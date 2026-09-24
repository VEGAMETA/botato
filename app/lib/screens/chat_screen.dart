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

  String get _peer => widget.contact.userId;

  @override
  void initState() {
    super.initState();
    widget.relay.activeChat = _peer;
    _reload();
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
      widget.relay.sendText(_peer, text);
      _input.clear();
      _reload();
    } catch (e) {
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text('$e')));
    }
    _focus.requestFocus();
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
      await core.clientSendPhoto(peerUser: _peer, image: bytes, caption: caption);
      if (caption.isNotEmpty && _input.text.trim() == caption) _input.clear();
      _reload();
    } catch (e) {
      _toast('$e');
    } finally {
      if (mounted) setState(() => _sendingPhoto = false);
    }
  }

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
            Expanded(child: Text(widget.contact.name, overflow: TextOverflow.ellipsis)),
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
                          _Bubble(m),
                        ],
                      );
                    },
                  ),
          ),
          if (_sendingPhoto) const LinearProgressIndicator(minHeight: 2),
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
                      bindings: {const SingleActivator(LogicalKeyboardKey.enter): _send},
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

class _Bubble extends StatelessWidget {
  const _Bubble(this.m);

  final core.MessageInfo m;

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
    return Align(
      alignment: mine ? Alignment.centerRight : Alignment.centerLeft,
      child: ConstrainedBox(
        constraints: BoxConstraints(maxWidth: MediaQuery.sizeOf(context).width * 0.78),
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
              if (m.photo case final photo?) ...[
                PhotoContent(msgId: m.msgId, photo: photo),
                if (m.text.isNotEmpty) const SizedBox(height: 6),
              ],
              if (m.text.isNotEmpty)
                Align(
                  alignment: Alignment.centerLeft,
                  widthFactor: 1,
                  child: SelectableText(m.text, style: const TextStyle(fontSize: 15)),
                ),
              const SizedBox(height: 2),
              Row(
                mainAxisSize: MainAxisSize.min,
                children: [
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
    );
  }
}
