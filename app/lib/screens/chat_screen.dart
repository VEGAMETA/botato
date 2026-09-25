import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:image_picker/image_picker.dart';
import 'package:pasteboard/pasteboard.dart';

import '../call/call_notifier.dart';
import '../call/call_service.dart';
import '../relay_service.dart';
import '../src/rust/api/client.dart' as core;
import '../emoji.dart';
import '../image_prep.dart';
import '../link_text.dart';
import '../voice.dart';
import '../theme.dart';
import 'photo_view.dart';
import '../l10n.dart';

/// Message status codes from the Rust store.
abstract final class MsgStatus {
  static const sending = 0, sent = 1, delivered = 2, failed = 3;
}

class ChatScreen extends StatefulWidget {
  const ChatScreen({
    super.key,
    required this.contact,
    required this.relay,
    required this.calls,
    this.shared,
    this.jumpTo,
  });

  /// Message to scroll to and highlight on open (a search hit).
  final String? jumpTo;

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

  final _recorder = VoiceRecorder();

  Future<void> _startVoice() async {
    if (widget.calls.busy) {
      _toast(tr('Во время звонка голосовые недоступны', 'Voice messages are unavailable during a call'));
      return;
    }
    await VoicePlayer.instance.stop();
    try {
      await _recorder.start();
    } catch (e) {
      _toast('$e');
    }
  }

  Future<void> _sendVoice() async {
    final v = await _recorder.stop();
    if (v == null) {
      _toast(tr('Слишком коротко', 'Too short'));
      return;
    }
    try {
      await core.clientSendVoice(
        peerUser: _peer,
        audio: v.audio,
        durationMs: v.duration.inMilliseconds,
        waveform: v.wave,
        replyTo: _replyTo?.msgId,
      );
      if (mounted) setState(() => _replyTo = null);
      _reload();
    } catch (e) {
      _toast('$e');
    }
  }

  /// How many of the newest messages are loaded (grows to reach a search hit).
  int _limit = 200;
  final _scroll = ScrollController();

  // Search in this chat: hits replace the message list while the query is non-empty.
  bool _searching = false;
  final _searchField = TextEditingController();
  List<core.SearchHit> _hits = [];
  Timer? _searchDebounce;

  /// A search hit being shown: scrolled to and highlighted for a moment.
  String? _highlight;
  final _highlightKey = GlobalKey();

  void _search(String q) {
    _searchDebounce?.cancel();
    _searchDebounce = Timer(const Duration(milliseconds: 250), () async {
      final hits = q.trim().isEmpty
          ? <core.SearchHit>[]
          : await core.clientSearch(peerUser: _peer, query: q, limit: 200);
      if (mounted && _searchField.text == q) setState(() => _hits = hits);
    });
  }

  void _closeSearch() {
    _searchDebounce?.cancel();
    setState(() {
      _searching = false;
      _hits = [];
      _searchField.clear();
    });
  }

  /// Loads back far enough to contain [msgId], scrolls to it and highlights it.
  Future<void> _jumpTo(String msgId) async {
    while (!_messages.any((m) => m.msgId == msgId) && _limit < 100000) {
      _limit *= 4;
      _messages = widget.relay.messages(_peer, limit: _limit);
      if (_messages.length < _limit) break; // everything is loaded
    }
    final idx = _messages.indexWhere((m) => m.msgId == msgId);
    if (idx < 0 || !mounted) return;
    setState(() => _highlight = msgId);
    // The list is reversed: item 0 is the newest. Jump near it, then let the frame place it.
    final fromBottom = _messages.length - 1 - idx;
    for (var attempt = 0; attempt < 6; attempt++) {
      await WidgetsBinding.instance.endOfFrame;
      final ctx = _highlightKey.currentContext;
      if (ctx != null && ctx.mounted) {
        await Scrollable.ensureVisible(ctx, alignment: 0.5, duration: const Duration(milliseconds: 250));
        break;
      }
      if (_scroll.hasClients) {
        final target = (fromBottom * 72.0).clamp(0.0, _scroll.position.maxScrollExtent);
        _scroll.jumpTo(target);
      }
    }
    Future.delayed(const Duration(seconds: 2), () {
      if (mounted && _highlight == msgId) setState(() => _highlight = null);
    });
  }

  /// The message being answered, or edited, above the input.
  core.MessageInfo? _replyTo;
  core.MessageInfo? _editing;

  String get _me => widget.relay.contacts.where((c) => c.isMe).firstOrNull?.userId ?? '';
  String get _myName => widget.relay.contacts.where((c) => c.isMe).firstOrNull?.name ?? '';

  String _nameOf(String userId) {
    if (userId == _me) return tr('Вы', 'You');
    return widget.relay.contacts.where((c) => c.userId == userId).firstOrNull?.name ?? widget.contact.name;
  }

  String get _peer => widget.contact.userId;

  @override
  void initState() {
    super.initState();
    widget.relay.activeChat = _peer;
    _messages = widget.relay.messages(_peer, limit: _limit);
    // markRead notifies listeners; not allowed while the route is being built.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      _reload();
      if (widget.shared case final s?) _takeShared(s);
      if (widget.jumpTo case final id?) _jumpTo(id);
    });
    _sub = widget.relay.messageUpdates.where((p) => p == _peer).listen((_) => _reload());
  }

  @override
  void dispose() {
    if (widget.relay.activeChat == _peer) widget.relay.activeChat = null;
    _sub?.cancel();
    _recorder.dispose();
    _input.dispose();
    _scroll.dispose();
    _searchField.dispose();
    _searchDebounce?.cancel();
    _focus.dispose();
    super.dispose();
  }

  void _reload() {
    setState(() => _messages = widget.relay.messages(_peer, limit: _limit));
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
                      tooltip: tr('Своё эмодзи', 'Custom emoji'),
                      icon: const Icon(Icons.add_reaction_outlined),
                      onPressed: () => Navigator.pop(ctx, 'custom'),
                    ),
                  ],
                ),
              ),
            if (!m.deleted) ...[
              ListTile(
                leading: const Icon(Icons.reply),
                title: Text(tr('Ответить', 'Reply')),
                onTap: () => Navigator.pop(ctx, 'reply'),
              ),
              if (m.text.isNotEmpty)
                ListTile(
                  leading: const Icon(Icons.copy),
                  title: Text(tr('Копировать', 'Copy')),
                  onTap: () => Navigator.pop(ctx, 'copy'),
                ),
              if (m.text.isNotEmpty)
                ListTile(
                  leading: const Icon(Icons.text_fields),
                  title: Text(tr('Выделить текст', 'Select text')),
                  onTap: () => Navigator.pop(ctx, 'select'),
                ),
              ListTile(
                leading: const Icon(Icons.forward),
                title: Text(tr('Переслать', 'Forward')),
                onTap: () => Navigator.pop(ctx, 'forward'),
              ),
              if (m.fromMe && (m.text.isNotEmpty || m.photo == null))
                ListTile(
                  leading: const Icon(Icons.edit_outlined),
                  title: Text(tr('Изменить', 'Edit')),
                  onTap: () => Navigator.pop(ctx, 'edit'),
                ),
            ],
            ListTile(
              leading: const Icon(Icons.delete_outline, color: BotatoColors.danger),
              title: Text(tr('Удалить', 'Delete'), style: TextStyle(color: BotatoColors.danger)),
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
        _toast(tr('Скопировано', 'Copied'));
      case 'select':
        await showDialog<void>(
          context: context,
          builder: (ctx) => AlertDialog(
            content: SingleChildScrollView(child: SelectableText(m.text, style: const TextStyle(fontSize: 16))),
            actions: [TextButton(onPressed: () => Navigator.pop(ctx), child: Text(tr('Закрыть', 'Close')))],
          ),
        );
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
        title: Text(tr('Удалить сообщение?', 'Delete the message?')),
        content: Text(
          tr('Удалить у всех или только на этом устройстве?', 'Delete for everyone or only on this device?'),
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx), child: Text(tr('Отмена', 'Cancel'))),
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: Text(tr('Только у меня', 'Only for me'))),
          if (canForAll)
            FilledButton(onPressed: () => Navigator.pop(ctx, true), child: Text(tr('У всех', 'For everyone'))),
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
        title: Text(tr('Переслать', 'Forward')),
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

  /// Ctrl+V: an image on the clipboard is offered for sending; text is pasted as usual.
  Future<void> _paste() async {
    final image = await _clipboardImage();
    if (image != null) return _confirmImage(image);
    final text = (await Clipboard.getData(Clipboard.kTextPlain))?.text;
    if (text == null || text.isEmpty) return;
    final v = _input.value;
    final sel = v.selection.isValid ? v.selection : TextSelection.collapsed(offset: v.text.length);
    _input.value = v.copyWith(
      text: v.text.replaceRange(sel.start, sel.end, text),
      selection: TextSelection.collapsed(offset: sel.start + text.length),
    );
  }

  static Future<Uint8List?> _clipboardImage() async {
    try {
      final b = await Pasteboard.image;
      return b == null || b.isEmpty ? null : await clipboardToPng(b);
    } catch (_) {
      return null;
    }
  }

  /// A pasted or keyboard-inserted image: preview, then send (the input text is the caption).
  Future<void> _confirmImage(Uint8List bytes) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(tr('Отправить картинку?', 'Send the image?')),
        content: ConstrainedBox(
          constraints: const BoxConstraints(maxHeight: 360, maxWidth: 420),
          child: Image.memory(bytes, fit: BoxFit.contain),
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: Text(tr('Отмена', 'Cancel'))),
          FilledButton(onPressed: () => Navigator.pop(ctx, true), child: Text(tr('Отправить', 'Send'))),
        ],
      ),
    );
    if (ok == true) await _sendPhoto(bytes);
  }

  Future<void> _pickPhoto() async {
    var source = ImageSource.gallery;
    final clipboard = await _clipboardImage();
    if (!mounted) return;
    if (Platform.isAndroid || Platform.isIOS || clipboard != null) {
      final picked = await showModalBottomSheet<Object>(
        context: context,
        builder: (ctx) => SafeArea(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (clipboard != null)
                ListTile(
                  leading: const Icon(Icons.content_paste),
                  title: Text(tr('Из буфера обмена', 'From clipboard')),
                  onTap: () => Navigator.pop(ctx, 'clipboard'),
                ),
              ListTile(
                leading: const Icon(Icons.photo_library_outlined),
                title: Text(Platform.isAndroid || Platform.isIOS ? tr('Галерея', 'Gallery') : tr('Файл', 'File')),
                onTap: () => Navigator.pop(ctx, ImageSource.gallery),
              ),
              if (Platform.isAndroid || Platform.isIOS)
                ListTile(
                  leading: const Icon(Icons.photo_camera_outlined),
                  title: Text(tr('Камера', 'Camera')),
                  onTap: () => Navigator.pop(ctx, ImageSource.camera),
                ),
            ],
          ),
        ),
      );
      if (picked == null) return;
      if (picked == 'clipboard') return _confirmImage(clipboard!);
      source = picked as ImageSource;
    }
    final XFile? file;
    try {
      // On phones the picker re-encodes to JPEG (HEIC and friends become decodable);
      // the final resize and metadata stripping happen in Rust on every platform.
      // The original file: with size/quality options the picker would turn PNGs into JPEGs.
      file = await ImagePicker().pickImage(source: source, requestFullMetadata: false);
    } catch (e) {
      _toast(tr('Не удалось открыть фото: $e', 'Couldn\'t open the photo: $e'));
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
        title: Text(
          s.files.length == 1
              ? tr('Отправить фото?', 'Send the photo?')
              : tr('Отправить фото: ${s.files.length}?', 'Send photos: ${s.files.length}?'),
        ),
        content: text.isEmpty
            ? null
            : Text(tr('Подпись: $text', 'Caption: $text'), maxLines: 4, overflow: TextOverflow.ellipsis),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: Text(tr('Отмена', 'Cancel'))),
          FilledButton(onPressed: () => Navigator.pop(ctx, true), child: Text(tr('Отправить', 'Send'))),
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

  Widget _searchScaffold() {
    final q = _searchField.text.trim();
    return PopScope(
      canPop: false,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) _closeSearch();
      },
      child: Scaffold(
        appBar: AppBar(
          leading: IconButton(icon: const Icon(Icons.arrow_back), onPressed: _closeSearch),
          title: TextField(
            controller: _searchField,
            autofocus: true,
            decoration: InputDecoration(hintText: tr('Поиск в чате', 'Search in chat'), border: InputBorder.none),
            onChanged: _search,
          ),
        ),
        body: q.isEmpty
            ? const SizedBox.shrink()
            : _hits.isEmpty
            ? Center(
                child: Text(tr('Ничего не найдено', 'Nothing found'), style: TextStyle(color: BotatoColors.smoke)),
              )
            : ListView.separated(
                itemCount: _hits.length,
                separatorBuilder: (_, _) => const Divider(height: 1),
                itemBuilder: (_, i) {
                  final h = _hits[i];
                  return ListTile(
                    title: Text(h.fromMe ? tr('Вы', 'You') : widget.contact.name, style: const TextStyle(fontSize: 13)),
                    subtitle: _Snippet(text: h.text, query: q),
                    trailing: Text(
                      _dateTime(h.sentAt.toInt()),
                      style: const TextStyle(color: BotatoColors.smoke, fontSize: 12),
                    ),
                    onTap: () {
                      _closeSearch();
                      _jumpTo(h.msgId);
                    },
                  );
                },
              ),
      ),
    );
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
    if (_searching) return _searchScaffold();
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
                        on ? tr('в сети', 'online') : tr('не в сети', 'offline'),
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
            tooltip: tr('Поиск в чате', 'Search in chat'),
            icon: const Icon(Icons.search),
            onPressed: () => setState(() => _searching = true),
          ),
          IconButton(
            tooltip: tr('Позвонить', 'Call'),
            icon: const Icon(Icons.call),
            onPressed: online ? () => widget.calls.startCall(widget.contact) : null,
          ),
          IconButton(
            tooltip: tr('Видеозвонок', 'Video call'),
            icon: const Icon(Icons.videocam),
            onPressed: online ? () => widget.calls.startCall(widget.contact, video: true) : null,
          ),
        ],
      ),
      body: Column(
        children: [
          Expanded(
            child: _messages.isEmpty
                ? Center(
                    child: Text(
                      tr('Сообщений пока нет', 'No messages yet'),
                      style: TextStyle(color: BotatoColors.smoke),
                    ),
                  )
                : ListView.builder(
                    controller: _scroll,
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
                          AnimatedContainer(
                            key: m.msgId == _highlight ? _highlightKey : null,
                            duration: const Duration(milliseconds: 300),
                            decoration: BoxDecoration(
                              color: m.msgId == _highlight
                                  ? BotatoColors.potato.withValues(alpha: 0.18)
                                  : Colors.transparent,
                              borderRadius: BorderRadius.circular(12),
                            ),
                            child: _Bubble(
                              m,
                              key: ValueKey(m.msgId),
                              gallery: _gallery,
                              me: _me,
                              nameOf: _nameOf,
                              onMenu: _menu,
                              onReact: _react,
                            ),
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
                        ? _Quote(name: tr('Редактирование', 'Editing'), text: _editing!.text, maxLines: 1)
                        : _Quote(
                            name: _nameOf(_replyTo!.fromMe ? _me : _peer),
                            text: _replyTo!.text.isEmpty && _replyTo!.photo != null
                                ? tr('📷 Фото', '📷 Photo')
                                : _replyTo!.text,
                            maxLines: 1,
                          ),
                  ),
                  IconButton(tooltip: tr('Отмена', 'Cancel'), icon: const Icon(Icons.close), onPressed: _cancelCompose),
                ],
              ),
            ),
          SafeArea(
            top: false,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(4, 4, 6, 8),
              child: ListenableBuilder(
                listenable: Listenable.merge([_recorder, _input]),
                builder: (context, _) => _recorder.recording
                    ? _RecordingBar(recorder: _recorder, onCancel: _recorder.cancel, onSend: _sendVoice)
                    : Row(
                        crossAxisAlignment: CrossAxisAlignment.end,
                        children: [
                          IconButton(
                            tooltip: tr('Фото', 'Photo'),
                            icon: const Icon(Icons.add_photo_alternate_outlined),
                            onPressed: _sendingPhoto ? null : _pickPhoto,
                          ),
                          Expanded(
                            child: CallbackShortcuts(
                              // Desktop: Enter sends, Shift+Enter makes a new line.
                              bindings: {
                                const SingleActivator(LogicalKeyboardKey.enter): _send,
                                const SingleActivator(LogicalKeyboardKey.escape): _cancelCompose,
                                const SingleActivator(LogicalKeyboardKey.keyV, control: true): _paste,
                              },
                              child: TextField(
                                controller: _input,
                                focusNode: _focus,
                                autofocus: true,
                                minLines: 1,
                                maxLines: 6,
                                textCapitalization: TextCapitalization.sentences,
                                // Android keyboards (Gboard, Samsung) can insert images from their clipboard.
                                contentInsertionConfiguration: ContentInsertionConfiguration(
                                  allowedMimeTypes: const ['image/png', 'image/jpeg', 'image/gif', 'image/webp'],
                                  onContentInserted: (c) {
                                    if (c.hasData) _confirmImage(c.data!);
                                  },
                                ),
                                decoration: InputDecoration(
                                  hintText: tr('Сообщение', 'Message'),
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
                          // Empty input: record a voice message instead (like Telegram).
                          if (_input.text.trim().isEmpty && _editing == null)
                            IconButton.filled(
                              tooltip: tr('Голосовое', 'Voice message'),
                              icon: const Icon(Icons.mic),
                              onPressed: _startVoice,
                            )
                          else
                            IconButton.filled(icon: const Icon(Icons.send), onPressed: _send),
                        ],
                      ),
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
    final months = [
      tr('января', 'January'),
      tr('февраля', 'February'),
      tr('марта', 'March'),
      tr('апреля', 'April'),
      tr('мая', 'May'),
      tr('июня', 'June'),
      tr('июля', 'July'),
      tr('августа', 'August'),
      tr('сентября', 'September'),
      tr('октября', 'October'),
      tr('ноября', 'November'),
      tr('декабря', 'December'),
    ];
    final text = _sameDay(at, now.millisecondsSinceEpoch ~/ 1000)
        ? tr('Сегодня', 'Today')
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
    // A photo without caption, like big emoji, goes without a bubble.
    final bare =
        bigEmoji > 0 || (m.photo != null && m.text.isEmpty && m.reply == null && m.forwardedFrom == null && !m.deleted);
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
            padding: bare ? const EdgeInsets.fromLTRB(4, 0, 4, 2) : const EdgeInsets.fromLTRB(12, 8, 10, 6),
            decoration: BoxDecoration(
              color: bare
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
                  Align(
                    alignment: Alignment.centerLeft,
                    widthFactor: 1,
                    child: Text(
                      tr('сообщение удалено', 'message deleted'),
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
                          tr('Переслано от $from', 'Forwarded from $from'),
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
                  if (m.voice case final v?) _VoiceBubble(msgId: m.msgId, voice: v),
                  if (m.photo case final photo?) ...[
                    PhotoContent(key: ValueKey(m.msgId), msgId: m.msgId, photo: photo, gallery: gallery),
                    if (m.text.isNotEmpty) const SizedBox(height: 6),
                  ],
                  if (m.text.isNotEmpty)
                    Align(
                      alignment: Alignment.centerLeft,
                      widthFactor: 1,
                      child: bigEmoji > 0
                          ? Text(m.text.trim(), style: TextStyle(fontSize: bigEmoji == 1 ? 56 : 40))
                          : LinkText(m.text, style: const TextStyle(fontSize: 15)),
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
                      Text(tr('изменено · ', 'edited · '), style: TextStyle(color: BotatoColors.smoke, fontSize: 11)),
                    Text(_hhmm(m.sentAt.toInt()), style: const TextStyle(color: BotatoColors.smoke, fontSize: 11)),
                    if (icon != null) ...[
                      const SizedBox(width: 4),
                      Tooltip(
                        message: switch (status) {
                          MsgStatus.sending => tr('отправляется', 'sending'),
                          MsgStatus.sent => tr('на сервере', 'on the server'),
                          MsgStatus.delivered => tr('доставлено', 'delivered'),
                          _ => tr('не доставлено за 24 ч', 'not delivered within 24 h'),
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
      title: Text(tr('Своя реакция', 'Custom reaction')),
      content: TextField(
        controller: _field,
        autofocus: true,
        textAlign: TextAlign.center,
        style: const TextStyle(fontSize: 32),
        decoration: InputDecoration(
          hintText: '🙂',
          helperText: tr('Одно эмодзи; на ПК: Win + .', 'One emoji; on PC: Win + .'),
        ),
        onChanged: (_) => setState(() {}),
        onSubmitted: (_) => Navigator.pop(context, _emoji),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(context), child: Text(tr('Отмена', 'Cancel'))),
        FilledButton(
          onPressed: _emoji.isEmpty ? null : () => Navigator.pop(context, _emoji),
          child: Text(tr('Поставить', 'Set')),
        ),
      ],
    );
  }
}

String _dateTime(int t) {
  final d = DateTime.fromMillisecondsSinceEpoch(t * 1000);
  final now = DateTime.now();
  String two(int v) => v.toString().padLeft(2, '0');
  final time = '${two(d.hour)}:${two(d.minute)}';
  if (d.year == now.year && d.month == now.month && d.day == now.day) return time;
  return '${two(d.day)}.${two(d.month)}${d.year == now.year ? '' : '.${d.year % 100}'}';
}

/// A search hit: the text around the first match, the match in bold.
class _Snippet extends StatelessWidget {
  const _Snippet({required this.text, required this.query});

  final String text;
  final String query;

  @override
  Widget build(BuildContext context) {
    final flat = text.replaceAll('\n', ' ');
    final at = flat.toLowerCase().indexOf(query.toLowerCase());
    if (at < 0) return Text(flat, maxLines: 2, overflow: TextOverflow.ellipsis);
    final start = at > 40 ? at - 30 : 0;
    return Text.rich(
      TextSpan(
        children: [
          TextSpan(text: '${start > 0 ? '…' : ''}${flat.substring(start, at)}'),
          TextSpan(
            text: flat.substring(at, at + query.length),
            style: const TextStyle(fontWeight: FontWeight.bold, color: BotatoColors.potato),
          ),
          TextSpan(text: flat.substring(at + query.length)),
        ],
      ),
      maxLines: 2,
      overflow: TextOverflow.ellipsis,
    );
  }
}

/// Replaces the input while recording: live level, timer, cancel, send.
class _RecordingBar extends StatelessWidget {
  const _RecordingBar({required this.recorder, required this.onCancel, required this.onSend});

  final VoiceRecorder recorder;
  final VoidCallback onCancel;
  final VoidCallback onSend;

  @override
  Widget build(BuildContext context) {
    final t = recorder.elapsed;
    final mm = t.inMinutes.toString().padLeft(2, '0');
    final ss = (t.inSeconds % 60).toString().padLeft(2, '0');
    return Row(
      children: [
        IconButton(tooltip: tr('Отменить', 'Cancel'), icon: const Icon(Icons.delete_outline), onPressed: onCancel),
        const SizedBox(width: 4),
        Container(
          width: 12 + 12 * recorder.level,
          height: 12 + 12 * recorder.level,
          decoration: const BoxDecoration(color: BotatoColors.danger, shape: BoxShape.circle),
        ),
        const SizedBox(width: 12),
        Text('$mm:$ss', style: const TextStyle(fontSize: 16, fontFeatures: [FontFeature.tabularFigures()])),
        const Expanded(child: SizedBox()),
        IconButton.filled(tooltip: tr('Отправить', 'Send'), icon: const Icon(Icons.send), onPressed: onSend),
      ],
    );
  }
}

/// Voice message in a bubble: play/pause, waveform with progress, duration.
class _VoiceBubble extends StatelessWidget {
  const _VoiceBubble({required this.msgId, required this.voice});

  final String msgId;
  final core.VoiceInfo voice;

  @override
  Widget build(BuildContext context) {
    final player = VoicePlayer.instance;
    return ListenableBuilder(
      listenable: player,
      builder: (context, _) {
        final total = Duration(milliseconds: voice.durationMs);
        final mine = player.playing == msgId;
        final pos = mine ? player.position : Duration.zero;
        final progress = total.inMilliseconds == 0 ? 0.0 : (pos.inMilliseconds / total.inMilliseconds).clamp(0.0, 1.0);
        final shown = mine ? pos : total;
        final label = '${shown.inMinutes}:${(shown.inSeconds % 60).toString().padLeft(2, '0')}';
        return SizedBox(
          width: 240,
          child: Row(
            children: [
              if (voice.gone)
                const Padding(
                  padding: EdgeInsets.all(8),
                  child: Icon(Icons.mic_off, color: BotatoColors.smoke),
                )
              else if (!voice.ready)
                const Padding(
                  padding: EdgeInsets.all(12),
                  child: SizedBox(width: 20, height: 20, child: CircularProgressIndicator(strokeWidth: 2)),
                )
              else
                IconButton.filledTonal(
                  icon: Icon(player.isPlaying(msgId) ? Icons.pause : Icons.play_arrow),
                  onPressed: () => player.toggle(msgId, () => core.clientPhoto(msgId: msgId)),
                ),
              const SizedBox(width: 6),
              Expanded(
                child: SizedBox(height: 28, child: CustomPaint(painter: _WavePainter(voice.waveform, progress))),
              ),
              const SizedBox(width: 8),
              Text(
                voice.gone ? tr('недоступно', 'unavailable') : label,
                style: const TextStyle(color: BotatoColors.smoke, fontSize: 12),
              ),
            ],
          ),
        );
      },
    );
  }
}

class _WavePainter extends CustomPainter {
  _WavePainter(this.wave, this.progress);

  final List<int> wave;
  final double progress;

  @override
  void paint(Canvas canvas, Size size) {
    final bars = wave.isEmpty ? List<int>.filled(24, 60) : wave;
    final w = size.width / bars.length;
    final played = Paint()..color = BotatoColors.potato;
    final rest = Paint()..color = BotatoColors.smoke.withValues(alpha: 0.6);
    for (var i = 0; i < bars.length; i++) {
      final h = size.height * (0.15 + 0.85 * bars[i] / 255);
      final r = RRect.fromRectAndRadius(
        Rect.fromLTWH(i * w + w * 0.2, (size.height - h) / 2, w * 0.6, h),
        const Radius.circular(2),
      );
      canvas.drawRRect(r, (i + 0.5) / bars.length <= progress ? played : rest);
    }
  }

  @override
  bool shouldRepaint(_WavePainter old) => old.progress != progress || old.wave != wave;
}
