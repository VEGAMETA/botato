import 'dart:async';

import 'package:flutter/material.dart';

import '../call/call_screen.dart';
import '../call/call_service.dart';
import '../relay_service.dart';
import '../src/rust/api/client.dart' as core;
import '../src/rust/api/identity.dart' as id;
import '../theme.dart';
import 'chat_screen.dart';

class HomeScreen extends StatefulWidget {
  const HomeScreen({super.key, required this.info, required this.relay, required this.calls});

  final id.CertInfo info;
  final RelayService relay;
  final CallService calls;

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  StreamSubscription<IncomingSignal>? _sub;
  final _pingSent = <String, DateTime>{};

  @override
  void initState() {
    super.initState();
    _sub = widget.relay.signals.listen(_onSignal);
    // Tap on a message notification.
    widget.calls.notifier.onOpenChat = (peerUser) {
      final c = widget.relay.contacts.where((c) => c.userId == peerUser).firstOrNull;
      if (c != null && mounted) openChat(context, c);
    };
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      final peer = await widget.calls.notifier.pendingChat();
      if (peer != null) widget.calls.notifier.onOpenChat?.call(peer);
    });
  }

  @override
  void dispose() {
    _sub?.cancel();
    super.dispose();
  }

  void _onSignal(IncomingSignal s) {
    final who = widget.relay.contactOfDevice(s.fromDevice)?.name ?? s.fromDevice;
    switch (s.data['type']) {
      case 'ping':
        widget.relay.signal(s.fromDevice, {'type': 'pong'});
        _toast('ping от $who');
      case 'pong':
        final sent = _pingSent.remove(s.fromDevice);
        final ms = sent == null ? '?' : DateTime.now().difference(sent).inMilliseconds;
        _toast('pong от $who: $ms мс');
    }
  }

  void _toast(String text) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
  }

  void openChat(BuildContext context, core.ContactInfo c) {
    Navigator.of(context).popUntil((r) => r.isFirst);
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => ChatScreen(contact: c, relay: widget.relay, calls: widget.calls),
      ),
    );
  }

  void _ping(core.ContactInfo c) {
    for (final d in c.devices) {
      _pingSent[d] = DateTime.now();
      widget.relay.signal(d, {'type': 'ping'});
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return ListenableBuilder(
      listenable: widget.relay,
      builder: (context, _) {
        final r = widget.relay;
        final online = r.state == ConnState.online;
        final (color, label) = switch (r.state) {
          ConnState.online => (BotatoColors.ok, 'в сети'),
          ConnState.connecting => (Colors.amber, 'подключение…'),
          ConnState.offline => (BotatoColors.danger, 'нет связи'),
        };
        final others = r.contacts.where((c) => !c.isMe).toList()
          // Recent conversations first, then alphabetically.
          ..sort((a, b) {
            final ta = r.conversations[a.userId]?.lastAt.toInt() ?? 0;
            final tb = r.conversations[b.userId]?.lastAt.toInt() ?? 0;
            return ta != tb ? tb.compareTo(ta) : a.name.compareTo(b.name);
          });
        return Scaffold(
          appBar: AppBar(
            titleSpacing: 12,
            title: Row(
              children: [
                Image.asset('assets/icon/potato.png', width: 34, height: 34),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(widget.info.name, style: theme.textTheme.titleMedium),
                      Tooltip(
                        message: r.detail,
                        child: Row(
                          children: [
                            Icon(Icons.circle, color: color, size: 8),
                            const SizedBox(width: 6),
                            Text(label, style: theme.textTheme.bodySmall?.copyWith(color: BotatoColors.smoke)),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
            actions: [
              IconButton(
                tooltip: 'Звук и камера',
                icon: const Icon(Icons.tune),
                onPressed: () => showMediaSettings(context, widget.calls),
              ),
              IconButton(tooltip: 'Обновить контакты', icon: const Icon(Icons.refresh), onPressed: r.refreshContacts),
            ],
          ),
          body: others.isEmpty
              ? _Empty(online: online)
              : ListView(
                  padding: const EdgeInsets.only(top: 8, bottom: 24),
                  children: [
                    for (final c in others)
                      _ContactCard(
                        contact: c,
                        conversation: r.conversations[c.userId],
                        enabled: online,
                        onOpen: () => openChat(context, c),
                        onCall: () => widget.calls.startCall(c),
                        onVideo: () => widget.calls.startCall(c, video: true),
                        onPing: () => _ping(c),
                      ),
                  ],
                ),
        );
      },
    );
  }
}

class _ContactCard extends StatelessWidget {
  const _ContactCard({
    required this.contact,
    required this.conversation,
    required this.enabled,
    required this.onOpen,
    required this.onCall,
    required this.onVideo,
    required this.onPing,
  });

  final core.ContactInfo contact;
  final core.ConversationInfo? conversation;
  final bool enabled;
  final VoidCallback onOpen, onCall, onVideo, onPing;

  @override
  Widget build(BuildContext context) {
    final conv = conversation;
    final unread = conv?.unread.toInt() ?? 0;
    final devices = contact.devices.length;
    final subtitle = conv == null
        ? (devices == 1 ? '1 устройство' : 'устройств: $devices')
        : '${conv.lastFromMe ? 'Вы: ' : ''}${conv.lastText.replaceAll('\n', ' ')}';
    final theme = Theme.of(context);
    return Card(
      clipBehavior: Clip.antiAlias,
      child: InkWell(
        onTap: onOpen,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(14, 10, 6, 10),
          child: Row(
            children: [
              Badge(
                isLabelVisible: unread > 0,
                label: Text('$unread'),
                backgroundColor: BotatoColors.ember,
                child: Avatar(contact.name),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(contact.name, style: theme.textTheme.titleMedium),
                    Text(
                      subtitle,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: theme.textTheme.bodySmall?.copyWith(
                        color: unread > 0 ? theme.colorScheme.onSurface : BotatoColors.smoke,
                        fontWeight: unread > 0 ? FontWeight.w600 : null,
                      ),
                    ),
                  ],
                ),
              ),
              IconButton.filledTonal(
                tooltip: 'Позвонить',
                icon: const Icon(Icons.call),
                onPressed: enabled ? onCall : null,
              ),
              const SizedBox(width: 6),
              IconButton.filledTonal(
                tooltip: 'Видеозвонок',
                icon: const Icon(Icons.videocam),
                onPressed: enabled ? onVideo : null,
              ),
              PopupMenuButton<String>(
                icon: const Icon(Icons.more_vert, color: BotatoColors.smoke),
                onSelected: (_) => onPing(),
                itemBuilder: (_) => [
                  PopupMenuItem(value: 'ping', enabled: enabled, child: const Text('Проверить связь')),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _Empty extends StatelessWidget {
  const _Empty({required this.online});

  final bool online;

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Opacity(opacity: 0.85, child: Image.asset('assets/icon/potato.png', width: 140)),
            const SizedBox(height: 16),
            Text('Пока никого', style: Theme.of(context).textTheme.titleLarge),
            const SizedBox(height: 6),
            Text(
              online ? 'Контакты появятся, когда подключатся к серверу' : 'Ждём связи с сервером…',
              textAlign: TextAlign.center,
              style: const TextStyle(color: BotatoColors.smoke),
            ),
          ],
        ),
      ),
    );
  }
}
