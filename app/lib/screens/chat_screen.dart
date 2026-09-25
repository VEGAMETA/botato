import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:image_picker/image_picker.dart';

import '../call/call_notifier.dart';
import '../call/call_service.dart';
import '../relay_service.dart';
import '../src/rust/api/client.dart' as core;
import '../emoji.dart';
import '../image_prep.dart';
import '../theme.dart';
import 'photo_view.dart';

/// Message status codes from the Rust store.
abstract final class MsgStatus {
  static const sending = 0, sent = 1, delivered = 2, failed = 3;
}

class ChatScreen extends StatefulWidget {
  const ChatScreen({super.key, required this.contact, required this.relay, required this.calls, this.shared});

  final core.ContactInfo contact;
  final RelayService relay;
  final CallService calls;

  /// Shared from another app: text goes into the input, images are sent after a confirmation.
  final SharedContent? shared;

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
      if (!mounted) return;
      _reload();
      if (widget.shared case final s?) _takeShared(s);
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
                    IconButton(
                      tooltip: 'Своё эмодзи',
                      icon: const Icon(Icons.add_reaction_outlined),
                      onPressed: () => Navigator.pop(ctx, 'custom'),
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
      case 'custom':
        final e = await showDialog<String>(context: context, builder: (_) => const _EmojiDialog());
        if (e != null && e.isNotEmpty) _react(m, e);
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
    final canForAll = !m.deleted;
    final choice = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Удалить сообщение?'),
        content: const Text('Удалить у всех или только на этом устройстве?'),
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
      if (!mounted) return;
      if (to.userId == _peer) {
        _reload();
      } else {
        Navigator.of(context).pushReplacement(
          MaterialPageRoute(
            builder: (_) => ChatScreen(contact: to, relay: widget.relay, calls: widget.calls),
          ),
        );
      }
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
      // The original file: with size/quality options the picker would turn PNGs into JPEGs.
      file = await ImagePicker().pickImage(source: source, requestFullMetadata: false);
    } catch (e) {
      _toast('Не удалось открыть фото: $e');
      return;
    }
    if (file == null) return;
    await _sendPhoto(await file.readAsBytes());
  }

  Future<void> _takeShared(SharedContent s) async {
    final text = s.text?.trim() ?? '';
    if (s.files.isEmpty) {
      _input.text = text;
      _focus.requestFocus();
      return;
    }
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(s.files.length == 1 ? 'Отправить фото?' : 'Отправить фото: ${s.files.length}?'),
        content: text.isEmpty ? null : Text('Подпись: $text', maxLines: 4, overflow: TextOverflow.ellipsis),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('Отмена')),
          FilledButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('Отправить')),
        ],
      ),
    );
    if (ok != true || !mounted) return;
    for (final (i, path) in s.files.indexed) {
      try {
        await _sendPhoto(await File(path).readAsBytes(), caption: i == 0 ? text : '');
      } catch (e) {
        _toast('$e');
      }
      if (!mounted) return;
    }
  }

  Future<void> _sendPhoto(Uint8List bytes, {String? caption}) async {
    final fromInput = caption == null;
    caption ??= _input.text.trim();
    setState(() => _sendingPhoto = true);
    try {
      // PNG (screenshots) goes as is and may stay PNG. On phones any other image is first
      // shrunk by the OS decoder (HEIC works, 100 MP photos don't blow the memory); that PNG
      // is only an intermediate and becomes a JPEG.
      var keepPng = true;
      if (Platform.isAndroid && !isPng(bytes)) {
        bytes = await downscaleToPng(bytes);
        keepPng = false;
      }
      await core.clientSendPhoto(
        peerUser: _peer,
        image: bytes,
        caption: caption,
        replyTo: _replyTo?.msgId,
        keepPng: keepPng,
      );
      if (fromInput && caption.isNotEmpty && _input.text.trim() == caption) _input.clear();
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
                          hintText: 'Сообщение',
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
    final bigEmoji = m.deleted || m.photo != null || m.reply != null || m.forwardedFrom != null ? 0 : emojiOnly(m.text);
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
            padding: bigEmoji > 0 ? const EdgeInsets.fromLTRB(4, 0, 4, 2) : const EdgeInsets.fromLTRB(12, 8, 10, 6),
            decoration: BoxDecoration(
              color: bigEmoji > 0
                  ? Colors.transparent
                  : mine
                  ? BotatoColors.potatoShade.withValues(alpha: 0.55)
                  : scheme.surfaceContainerHigh,
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
                      child: Text(
                        bigEmoji > 0 ? m.text.trim() : m.text,
                        style: TextStyle(
                          fontSize: switch (bigEmoji) {
                            0 => 15,
                            1 => 56,
                            _ => 40,
                          },
                        ),
                      ),
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

/// Any emoji as a reaction: type or pick it from the system emoji keyboard.
class _EmojiDialog extends StatefulWidget {
  const _EmojiDialog();

  @override
  State<_EmojiDialog> createState() => _EmojiDialogState();
}

class _EmojiDialogState extends State<_EmojiDialog> {
  final _field = TextEditingController();

  @override
  void dispose() {
    _field.dispose();
    super.dispose();
  }

  /// One user-perceived character (flags, skin tones and ZWJ sequences stay whole).
  String get _emoji => _field.text.trim().characters.take(1).toString();

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Своя реакция'),
      content: TextField(
        controller: _field,
        autofocus: true,
        textAlign: TextAlign.center,
        style: const TextStyle(fontSize: 32),
        decoration: const InputDecoration(hintText: '🙂', helperText: 'Одно эмодзи; на ПК: Win + .'),
        onChanged: (_) => setState(() {}),
        onSubmitted: (_) => Navigator.pop(context, _emoji),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(context), child: const Text('Отмена')),
        FilledButton(
          onPressed: _emoji.isEmpty ? null : () => Navigator.pop(context, _emoji),
          child: const Text('Поставить'),
        ),
      ],
    );
  }
}
