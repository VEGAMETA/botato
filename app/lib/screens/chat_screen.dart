import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:image_picker/image_picker.dart';

import '../call/call_service.dart';
import '../relay_service.dart';
import '../src/rust/api/client.dart' as core;
import '../theme.dart';
import 'photo_view.dart';

/// Message status codes from the Rust store.
abstract final class MsgStatus {
  static const sending = 0, sent = 1, delivered = 2, failed = 3;
}

class ChatScreen extends StatefulWidget {
  const ChatScreen({super.key, required this.contact, required this.relay, required this.calls});

  final core.ContactInfo contact;
  final RelayService relay;
  final CallService calls;

  @override
  State<ChatScreen> createState() => _ChatScreenState();
}

class _ChatScreenState extends State<ChatScreen> {
  final _input = TextEditingController();
  final _focus = FocusNode();
  List<core.MessageInfo> _messages = [];
  StreamSubscription<String>? _sub;
  bool _sendingPhoto = false;

  /// The message being answered, or edited, above the input.
  core.MessageInfo? _replyTo;
  core.MessageInfo? _editing;

  String get _me => widget.relay.contacts.where((c) => c.isMe).firstOrNull?.userId ?? '';
  String get _myName => widget.relay.contacts.where((c) => c.isMe).firstOrNull?.name ?? '';

  String _nameOf(String userId) {
    if (userId == _me) return 'Вы';
    return widget.relay.contacts.where((c) => c.userId == userId).firstOrNull?.name ?? widget.contact.name;
  }

  String get _peer => widget.contact.userId;

  @override
  void initState() {
    super.initState();
    widget.relay.activeChat = _peer;
    _messages = widget.relay.messages(_peer);
    // markRead notifies listeners; not allowed while the route is being built.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _reload();
    });
    _sub = widget.relay.messageUpdates.where((p) => p == _peer).listen((_) => _reload());
  }

  @override
  void dispose() {
    if (widget.relay.activeChat == _peer) widget.relay.activeChat = null;
    _sub?.cancel();
    _input.dispose();
    _focus.dispose();
    super.dispose();
  }

  void _reload() {
    setState(() => _messages = widget.relay.messages(_peer));
    widget.relay.markRead(_peer);
    widget.calls.notifier.clearMessages(_peer);
  }

  void _send() {
    final text = _input.text.trim();
    if (text.isEmpty) return;
    try {
      final editing = _editing;
      if (editing != null) {
        core.clientEdit(peerUser: _peer, msgId: editing.msgId, text: text);
      } else {
        widget.relay.sendText(_peer, text, replyTo: _replyTo?.msgId);
      }
      _input.clear();
      setState(() {
        _replyTo = null;
        _editing = null;
      });
      _reload();
    } catch (e) {
      _toast('$e');
    }
    _focus.requestFocus();
  }

  void _cancelCompose() {
    if (_editing != null) _input.clear();
    setState(() {
      _replyTo = null;
      _editing = null;
    });
  }

  void _react(core.MessageInfo m, String emoji) {
    try {
      core.clientReact(peerUser: _peer, msgId: m.msgId, emoji: emoji);
      _reload();
    } catch (e) {
      _toast('$e');
    }
  }

  Future<void> _menu(core.MessageInfo m) async {
    final mine = m.reactions.where((r) => r.userId == _me).firstOrNull?.emoji;
    final action = await showModalBottomSheet<String>(
      context: context,
      builder: (ctx) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (!m.deleted)
              Padding(
                padding: const EdgeInsets.fromLTRB(8, 12, 8, 4),
                child: Wrap(
                  alignment: WrapAlignment.center,
                  spacing: 4,
                  children: [
                    for (final e in _quickReactions)
                      InkWell(
                        borderRadius: BorderRadius.circular(24),
                        onTap: () => Navigator.pop(ctx, 'react:$e'),
                        child: Container(
                          padding: const EdgeInsets.all(8),
                          decoration: BoxDecoration(
                            shape: BoxShape.circle,
                            color: e == mine ? BotatoColors.potato.withValues(alpha: 0.35) : null,
                          ),
                          child: Text(e, style: const TextStyle(fontSize: 26)),
                        ),
                      ),
                  ],
                ),
              ),
            if (!m.deleted) ...[
              ListTile(
                leading: const Icon(Icons.reply),
                title: const Text('Ответить'),
                onTap: () => Navigator.pop(ctx, 'reply'),
              ),
              if (m.text.isNotEmpty)
                ListTile(
                  leading: const Icon(Icons.copy),
                  title: const Text('Копировать'),
                  onTap: () => Navigator.pop(ctx, 'copy'),
                ),
              ListTile(
                leading: const Icon(Icons.forward),
                title: const Text('Переслать'),
                onTap: () => Navigator.pop(ctx, 'forward'),
              ),
              if (m.fromMe && (m.text.isNotEmpty || m.photo == null))
                ListTile(
                  leading: const Icon(Icons.edit_outlined),
                  title: const Text('Изменить'),
                  onTap: () => Navigator.pop(ctx, 'edit'),
                ),
            ],
            ListTile(
              leading: const Icon(Icons.delete_outline, color: BotatoColors.danger),
              title: const Text('Удалить', style: TextStyle(color: BotatoColors.danger)),
              onTap: () => Navigator.pop(ctx, 'delete'),
            ),
          ],
        ),
      ),
    );
    if (action == null || !mounted) return;
    switch (action) {
      case final a when a.startsWith('react:'):
        final e = a.substring(6);
        _react(m, e == mine ? '' : e);
      case 'reply':
        setState(() {
          _editing = null;
          _replyTo = m;
        });
        _focus.requestFocus();
      case 'copy':
        await Clipboard.setData(ClipboardData(text: m.text));
        _toast('Скопировано');
      case 'forward':
        await _forward(m);
      case 'edit':
        setState(() {
          _replyTo = null;
          _editing = m;
          _input.text = m.text;
        });
        _focus.requestFocus();
      case 'delete':
        await _delete(m);
    }
  }

  Future<void> _delete(core.MessageInfo m) async {
    final canForAll = m.fromMe && !m.deleted;
    final choice = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Удалить сообщение?'),
        content: Text(canForAll ? 'Удалить у всех или только на этом устройстве?' : 'Сообщение удалится только здесь.'),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('Отмена')),
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('Только у меня')),
          if (canForAll) FilledButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('У всех')),
        ],
      ),
    );
    if (choice == null) return;
    try {
      core.clientDelete(peerUser: _peer, msgId: m.msgId, forAll: choice);
      if (_editing?.msgId == m.msgId || _replyTo?.msgId == m.msgId) _cancelCompose();
      _reload();
    } catch (e) {
      _toast('$e');
    }
  }

  Future<void> _forward(core.MessageInfo m) async {
    final to = await showDialog<core.ContactInfo>(
      context: context,
      builder: (ctx) => SimpleDialog(
        title: const Text('Переслать'),
        children: [
          for (final c in widget.relay.visibleContacts)
            SimpleDialogOption(
              onPressed: () => Navigator.pop(ctx, c),
              child: Row(
                children: [
                  Avatar(c.name, size: 32),
                  const SizedBox(width: 12),
                  Expanded(child: Text(c.name)),
                ],
              ),
            ),
        ],
      ),
    );
    if (to == null) return;
    final from = m.forwardedFrom ?? (m.fromMe ? _myName : widget.contact.name);
    try {
      await core.clientForward(msgId: m.msgId, toPeer: to.userId, fromName: from);
      _toast('Переслано: ${to.name}');
      if (to.userId == _peer) _reload();
    } catch (e) {
      _toast('$e');
    }
  }

  Future<void> _pickPhoto() async {
    var source = ImageSource.gallery;
    if (Platform.isAndroid || Platform.isIOS) {
      final picked = await showModalBottomSheet<ImageSource>(
        context: context,
        builder: (ctx) => SafeArea(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              ListTile(
                leading: const Icon(Icons.photo_library_outlined),
                title: const Text('Галерея'),
                onTap: () => Navigator.pop(ctx, ImageSource.gallery),
              ),
              ListTile(
                leading: const Icon(Icons.photo_camera_outlined),
                title: const Text('Камера'),
                onTap: () => Navigator.pop(ctx, ImageSource.camera),
              ),
            ],
          ),
        ),
      );
      if (picked == null) return;
      source = picked;
    }
    final XFile? file;
    try {
      // On phones the picker re-encodes to JPEG (HEIC and friends become decodable);
      // the final resize and metadata stripping happen in Rust on every platform.
      file = await ImagePicker().pickImage(
        source: source,
        maxWidth: Platform.isAndroid ? 3200 : null,
        maxHeight: Platform.isAndroid ? 3200 : null,
        imageQuality: Platform.isAndroid ? 95 : null,
        requestFullMetadata: false,
      );
    } catch (e) {
      _toast('Не удалось открыть фото: $e');
      return;
    }
    if (file == null) return;
    await _sendPhoto(await file.readAsBytes());
  }

  Future<void> _sendPhoto(Uint8List bytes) async {
    final caption = _input.text.trim();
    setState(() => _sendingPhoto = true);
    try {
      await core.clientSendPhoto(peerUser: _peer, image: bytes, caption: caption, replyTo: _replyTo?.msgId);
      if (caption.isNotEmpty && _input.text.trim() == caption) _input.clear();
      if (mounted) setState(() => _replyTo = null);
      _reload();
    } catch (e) {
      _toast('$e');
    } finally {
      if (mounted) setState(() => _sendingPhoto = false);
    }
  }

  List<String> _gallery() => [
    for (final m in _messages)
      if (m.photo?.ready ?? false) m.msgId,
  ];

  void _toast(String text) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
  }

  @override
  Widget build(BuildContext context) {
    final online = widget.relay.state == ConnState.online;
    return Scaffold(
      appBar: AppBar(
        titleSpacing: 0,
        title: Row(
          children: [
            Avatar(widget.contact.name, size: 36),
            const SizedBox(width: 10),
            Expanded(
              child: ListenableBuilder(
                listenable: widget.relay,
                builder: (context, _) {
                  final on = widget.relay.isOnline(widget.contact);
                  return Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(widget.contact.name, overflow: TextOverflow.ellipsis),
                      Text(
                        on ? 'в сети' : 'не в сети',
                        style: TextStyle(fontSize: 12, color: on ? BotatoColors.ok : BotatoColors.smoke),
                      ),
                    ],
                  );
                },
              ),
            ),
          ],
        ),
        actions: [
          IconButton(
            tooltip: 'Позвонить',
            icon: const Icon(Icons.call),
            onPressed: online ? () => widget.calls.startCall(widget.contact) : null,
          ),
          IconButton(
            tooltip: 'Видеозвонок',
            icon: const Icon(Icons.videocam),
            onPressed: online ? () => widget.calls.startCall(widget.contact, video: true) : null,
          ),
        ],
      ),
      body: Column(
        children: [
          Expanded(
            child: _messages.isEmpty
                ? const Center(
                    child: Text('Сообщений пока нет', style: TextStyle(color: BotatoColors.smoke)),
                  )
                : ListView.builder(
                    reverse: true,
                    padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 12),
                    itemCount: _messages.length,
                    itemBuilder: (context, i) {
                      final m = _messages[_messages.length - 1 - i];
                      final prev = i + 1 < _messages.length ? _messages[_messages.length - 2 - i] : null;
                      return Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          if (prev == null || !_sameDay(prev.sentAt, m.sentAt)) _DayLabel(m.sentAt),
                          _Bubble(
                            m,
                            key: ValueKey(m.msgId),
                            gallery: _gallery,
                            me: _me,
                            nameOf: _nameOf,
                            onMenu: _menu,
                            onReact: _react,
                          ),
                        ],
                      );
                    },
                  ),
          ),
          if (_sendingPhoto) const LinearProgressIndicator(minHeight: 2),
          if (_replyTo != null || _editing != null)
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 6, 4, 0),
              child: Row(
                children: [
                  Icon(_editing != null ? Icons.edit_outlined : Icons.reply, color: BotatoColors.potato),
                  const SizedBox(width: 8),
                  Expanded(
                    child: _editing != null
                        ? _Quote(name: 'Редактирование', text: _editing!.text, maxLines: 1)
                        : _Quote(
                            name: _nameOf(_replyTo!.fromMe ? _me : _peer),
                            text: _replyTo!.text.isEmpty && _replyTo!.photo != null ? '📷 Фото' : _replyTo!.text,
                            maxLines: 1,
                          ),
                  ),
                  IconButton(tooltip: 'Отмена', icon: const Icon(Icons.close), onPressed: _cancelCompose),
                ],
              ),
            ),
          SafeArea(
            top: false,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(4, 4, 6, 8),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.end,
                children: [
                  IconButton(
                    tooltip: 'Фото',
                    icon: const Icon(Icons.add_photo_alternate_outlined),
                    onPressed: _sendingPhoto ? null : _pickPhoto,
                  ),
                  Expanded(
                    child: CallbackShortcuts(
                      // Desktop: Enter sends, Shift+Enter makes a new line.
                      bindings: {
                        const SingleActivator(LogicalKeyboardKey.enter): _send,
                        const SingleActivator(LogicalKeyboardKey.escape): _cancelCompose,
                      },
                      child: TextField(
                        controller: _input,
                        focusNode: _focus,
                        autofocus: true,
                        minLines: 1,
                        maxLines: 6,
                        textCapitalization: TextCapitalization.sentences,
                        decoration: InputDecoration(
                          hintText: 'Сообщение или подпись к фото',
                          filled: true,
                          fillColor: Theme.of(context).colorScheme.surfaceContainer,
                          contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
                          border: OutlineInputBorder(
                            borderRadius: BorderRadius.circular(24),
                            borderSide: BorderSide.none,
                          ),
                        ),
                      ),
                    ),
                  ),
                  const SizedBox(width: 6),
                  IconButton.filled(icon: const Icon(Icons.send), onPressed: _send),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

bool _sameDay(int a, int b) {
  final x = DateTime.fromMillisecondsSinceEpoch(a * 1000);
  final y = DateTime.fromMillisecondsSinceEpoch(b * 1000);
  return x.year == y.year && x.month == y.month && x.day == y.day;
}

String _hhmm(int t) {
  final d = DateTime.fromMillisecondsSinceEpoch(t * 1000);
  return '${d.hour.toString().padLeft(2, '0')}:${d.minute.toString().padLeft(2, '0')}';
}

class _DayLabel extends StatelessWidget {
  const _DayLabel(this.at);

  final int at;

  @override
  Widget build(BuildContext context) {
    final d = DateTime.fromMillisecondsSinceEpoch(at * 1000);
    final now = DateTime.now();
    const months = [
      'января',
      'февраля',
      'марта',
      'апреля',
      'мая',
      'июня',
      'июля',
      'августа',
      'сентября',
      'октября',
      'ноября',
      'декабря',
    ];
    final text = _sameDay(at, now.millisecondsSinceEpoch ~/ 1000)
        ? 'Сегодня'
        : '${d.day} ${months[d.month - 1]}${d.year == now.year ? '' : ' ${d.year}'}';
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 10),
      child: Center(
        child: Text(text, style: const TextStyle(color: BotatoColors.smoke, fontSize: 12)),
      ),
    );
  }
}

/// Quick reactions offered in the message menu.
const _quickReactions = ['👍', '❤️', '😂', '😮', '😢', '🔥', '👎', '🎉'];

class _Bubble extends StatelessWidget {
  const _Bubble(
    this.m, {
    super.key,
    required this.gallery,
    required this.me,
    required this.nameOf,
    required this.onMenu,
    required this.onReact,
  });

  final core.MessageInfo m;
  final List<String> Function() gallery;

  /// Our user id: whose reactions are ours.
  final String me;
  final String Function(String userId) nameOf;
  final void Function(core.MessageInfo m) onMenu;
  final void Function(core.MessageInfo m, String emoji) onReact;

  @override
  Widget build(BuildContext context) {
    final mine = m.fromMe;
    final scheme = Theme.of(context).colorScheme;
    final status = m.status.toInt();
    final (IconData? icon, Color color) = !mine
        ? (null, BotatoColors.smoke)
        : switch (status) {
            MsgStatus.sending => (Icons.schedule, BotatoColors.smoke),
            MsgStatus.sent => (Icons.done, BotatoColors.smoke),
            MsgStatus.delivered => (Icons.done_all, BotatoColors.potato),
            _ => (Icons.error_outline, BotatoColors.danger),
          };
    // emoji -> (count, includes ours), in first-seen order.
    final counts = <String, (int, bool)>{};
    for (final r in m.reactions) {
      final (n, ours) = counts[r.emoji] ?? (0, false);
      counts[r.emoji] = (n + 1, ours || r.userId == me);
    }
    return Align(
      alignment: mine ? Alignment.centerRight : Alignment.centerLeft,
      child: ConstrainedBox(
        constraints: BoxConstraints(maxWidth: MediaQuery.sizeOf(context).width * 0.78),
        child: GestureDetector(
          onLongPress: () => onMenu(m),
          onSecondaryTap: () => onMenu(m),
          child: Container(
            margin: const EdgeInsets.symmetric(vertical: 2),
            padding: const EdgeInsets.fromLTRB(12, 8, 10, 6),
            decoration: BoxDecoration(
              color: mine ? BotatoColors.potatoShade.withValues(alpha: 0.55) : scheme.surfaceContainerHigh,
              borderRadius: BorderRadius.only(
                topLeft: const Radius.circular(16),
                topRight: const Radius.circular(16),
                bottomLeft: Radius.circular(mine ? 16 : 4),
                bottomRight: Radius.circular(mine ? 4 : 16),
              ),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.end,
              mainAxisSize: MainAxisSize.min,
              children: [
                if (m.deleted)
                  const Align(
                    alignment: Alignment.centerLeft,
                    widthFactor: 1,
                    child: Text(
                      'сообщение удалено',
                      style: TextStyle(fontStyle: FontStyle.italic, color: BotatoColors.smoke),
                    ),
                  )
                else ...[
                  if (m.forwardedFrom case final from?)
                    Align(
                      alignment: Alignment.centerLeft,
                      widthFactor: 1,
                      child: Padding(
                        padding: const EdgeInsets.only(bottom: 4),
                        child: Text(
                          'Переслано от $from',
                          style: const TextStyle(fontSize: 12, fontStyle: FontStyle.italic, color: BotatoColors.smoke),
                        ),
                      ),
                    ),
                  if (m.reply case final r?)
                    Align(
                      alignment: Alignment.centerLeft,
                      widthFactor: 1,
                      child: _Quote(name: nameOf(r.fromUser), text: r.text),
                    ),
                  if (m.photo case final photo?) ...[
                    PhotoContent(key: ValueKey(m.msgId), msgId: m.msgId, photo: photo, gallery: gallery),
                    if (m.text.isNotEmpty) const SizedBox(height: 6),
                  ],
                  if (m.text.isNotEmpty)
                    Align(
                      alignment: Alignment.centerLeft,
                      widthFactor: 1,
                      child: Text(m.text, style: const TextStyle(fontSize: 15)),
                    ),
                ],
                if (counts.isNotEmpty && !m.deleted)
                  Padding(
                    padding: const EdgeInsets.only(top: 6),
                    child: Wrap(
                      spacing: 4,
                      runSpacing: 4,
                      children: [
                        for (final MapEntry(key: emoji, value: (n, ours)) in counts.entries)
                          InkWell(
                            borderRadius: BorderRadius.circular(12),
                            onTap: () => onReact(m, ours ? '' : emoji),
                            child: Container(
                              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
                              decoration: BoxDecoration(
                                borderRadius: BorderRadius.circular(12),
                                color: ours ? BotatoColors.potato.withValues(alpha: 0.35) : Colors.black26,
                                border: ours ? Border.all(color: BotatoColors.potato) : null,
                              ),
                              child: Text(n > 1 ? '$emoji $n' : emoji, style: const TextStyle(fontSize: 14)),
                            ),
                          ),
                      ],
                    ),
                  ),
                const SizedBox(height: 2),
                Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    if (m.edited && !m.deleted)
                      const Text('изменено · ', style: TextStyle(color: BotatoColors.smoke, fontSize: 11)),
                    Text(_hhmm(m.sentAt.toInt()), style: const TextStyle(color: BotatoColors.smoke, fontSize: 11)),
                    if (icon != null) ...[
                      const SizedBox(width: 4),
                      Tooltip(
                        message: switch (status) {
                          MsgStatus.sending => 'отправляется',
                          MsgStatus.sent => 'на сервере',
                          MsgStatus.delivered => 'доставлено',
                          _ => 'не доставлено за 24 ч',
                        },
                        child: Icon(icon, size: 14, color: color),
                      ),
                    ],
                  ],
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// A quoted message: in a bubble (reply) and above the input (replying / editing).
class _Quote extends StatelessWidget {
  const _Quote({required this.name, required this.text, this.maxLines = 2});

  final String name;
  final String text;
  final int maxLines;

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.only(bottom: 6),
      padding: const EdgeInsets.fromLTRB(8, 4, 8, 4),
      decoration: BoxDecoration(
        color: Colors.black12,
        border: const Border(left: BorderSide(color: BotatoColors.potato, width: 3)),
        borderRadius: BorderRadius.circular(4),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            name,
            style: const TextStyle(color: BotatoColors.potato, fontSize: 12, fontWeight: FontWeight.w600),
          ),
          Text(
            text.isEmpty ? '…' : text,
            maxLines: maxLines,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(fontSize: 13, color: BotatoColors.smoke),
          ),
        ],
      ),
    );
  }
}
