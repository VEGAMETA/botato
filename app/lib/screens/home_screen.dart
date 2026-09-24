import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

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

  Future<void> _forget(BuildContext context, core.ContactInfo c) async {
    var deleteHistory = false;
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (ctx, set) => AlertDialog(
          title: Text('Забыть «${c.name}»?'),
          content: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text('Контакт пропадёт из списка. Если он напишет или позвонит — вернётся сам.'),
              CheckboxListTile(
                contentPadding: EdgeInsets.zero,
                value: deleteHistory,
                onChanged: (v) => set(() => deleteHistory = v ?? false),
                title: const Text('Удалить переписку на этом устройстве'),
              ),
            ],
          ),
          actions: [
            TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('Отмена')),
            FilledButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('Забыть')),
          ],
        ),
      ),
    );
    if (ok == true) widget.relay.forget(c.userId, deleteHistory: deleteHistory);
  }

  Future<void> _addContact(BuildContext context) async {
    widget.relay.refreshContacts();
    final added = await showDialog<List<core.ContactInfo>>(
      context: context,
      builder: (_) => _AddContactDialog(relay: widget.relay),
    );
    if (added == null || added.isEmpty) return;
    _toast(added.length == 1 ? 'Добавлен: ${added.first.name}' : 'Добавлено: ${added.length}');
  }

  Future<void> _showMe(BuildContext context) async {
    final device = core.clientMyDevice();
    Widget row(String label, String value) => ListTile(
      contentPadding: EdgeInsets.zero,
      title: Text(label, style: const TextStyle(color: BotatoColors.smoke, fontSize: 13)),
      subtitle: SelectableText(value, style: const TextStyle(fontFamily: 'monospace', fontSize: 15)),
      trailing: IconButton(
        tooltip: 'Скопировать',
        icon: const Icon(Icons.copy, size: 20),
        onPressed: () {
          Clipboard.setData(ClipboardData(text: value));
          _toast('Скопировано');
        },
      ),
    );
    await showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(widget.info.name),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            // Why "no connection": the last error from the relay client, copyable.
            if (widget.relay.state != ConnState.online && widget.relay.detail.isNotEmpty)
              row(widget.relay.state == ConnState.connecting ? 'Подключение к' : 'Нет связи', widget.relay.detail),
            row('ID пользователя', widget.info.userId),
            if (device != null) row('ID этого устройства', device),
          ],
        ),
        actions: [TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('Закрыть'))],
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
        final others = r.visibleContacts
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
                  child: GestureDetector(
                    behavior: HitTestBehavior.opaque,
                    onTap: () => _showMe(context),
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
                ),
              ],
            ),
            actions: [
              IconButton(
                tooltip: 'Добавить контакт',
                icon: const Icon(Icons.person_add_alt),
                onPressed: () => _addContact(context),
              ),
              IconButton(
                tooltip: 'Звук и камера',
                icon: const Icon(Icons.tune),
                onPressed: () => showMediaSettings(context, widget.calls),
              ),
              IconButton(
                tooltip: 'Обновить подключение и статусы',
                icon: const Icon(Icons.refresh),
                onPressed: r.refresh,
              ),
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
                        peerOnline: r.isOnline(c),
                        onOpen: () => openChat(context, c),
                        onCall: () => widget.calls.startCall(c),
                        onVideo: () => widget.calls.startCall(c, video: true),
                        onPing: () => _ping(c),
                        onForget: () => _forget(context, c),
                        onRename: () => showDialog<void>(
                          context: context,
                          builder: (_) => _RenameDialog(relay: widget.relay, contact: c),
                        ),
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
    required this.peerOnline,
    required this.onOpen,
    required this.onCall,
    required this.onVideo,
    required this.onPing,
    required this.onForget,
    required this.onRename,
  });

  final core.ContactInfo contact;
  final core.ConversationInfo? conversation;
  final bool enabled;

  /// Some device of the contact is connected to the relay now.
  final bool peerOnline;
  final VoidCallback onOpen, onCall, onVideo, onPing, onForget, onRename;

  @override
  Widget build(BuildContext context) {
    final conv = conversation;
    final unread = conv?.unread.toInt() ?? 0;
    final subtitle = conv == null
        ? (peerOnline ? 'в сети' : 'нет сообщений')
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
                child: Stack(
                  clipBehavior: Clip.none,
                  children: [
                    Avatar(contact.name),
                    if (peerOnline)
                      Positioned(
                        right: -1,
                        bottom: -1,
                        child: Container(
                          width: 13,
                          height: 13,
                          decoration: BoxDecoration(
                            color: BotatoColors.ok,
                            shape: BoxShape.circle,
                            border: Border.all(color: Theme.of(context).colorScheme.surfaceContainerLow, width: 2),
                          ),
                        ),
                      ),
                  ],
                ),
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
                onSelected: (v) => switch (v) {
                  'ping' => onPing(),
                  'rename' => onRename(),
                  _ => onForget(),
                },
                itemBuilder: (_) => [
                  PopupMenuItem(value: 'ping', enabled: enabled, child: const Text('Проверить связь')),
                  const PopupMenuItem(value: 'rename', child: Text('Переименовать…')),
                  const PopupMenuItem(value: 'forget', child: Text('Забыть…')),
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
              online ? 'Добавьте контакт по имени: кнопка вверху справа' : 'Ждём связи с сервером…',
              textAlign: TextAlign.center,
              style: const TextStyle(color: BotatoColors.smoke),
            ),
          ],
        ),
      ),
    );
  }
}

/// Owns its text field so the controller outlives the closing animation.
class _AddContactDialog extends StatefulWidget {
  const _AddContactDialog({required this.relay});

  final RelayService relay;

  @override
  State<_AddContactDialog> createState() => _AddContactDialogState();
}

class _AddContactDialogState extends State<_AddContactDialog> {
  final _field = TextEditingController();
  String? _error;

  @override
  void dispose() {
    _field.dispose();
    super.dispose();
  }

  void _submit() {
    final found = widget.relay.findContacts(_field.text);
    if (found.isEmpty) {
      setState(() => _error = 'Такого пользователя нет. Он должен хотя бы раз подключиться с сертификатом.');
      return;
    }
    for (final c in found) {
      widget.relay.add(c.userId);
    }
    Navigator.pop(context, found);
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Добавить контакт'),
      content: TextField(
        controller: _field,
        autofocus: true,
        decoration: InputDecoration(labelText: 'Имя или ID', errorText: _error, errorMaxLines: 3),
        onChanged: (_) {
          if (_error != null) setState(() => _error = null);
        },
        onSubmitted: (_) => _submit(),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(context), child: const Text('Отмена')),
        FilledButton(onPressed: _submit, child: const Text('Добавить')),
      ],
    );
  }
}

/// Local display name for a contact; only this device sees it.
class _RenameDialog extends StatefulWidget {
  const _RenameDialog({required this.relay, required this.contact});

  final RelayService relay;
  final core.ContactInfo contact;

  @override
  State<_RenameDialog> createState() => _RenameDialogState();
}

class _RenameDialogState extends State<_RenameDialog> {
  late final _field = TextEditingController(text: widget.contact.name);

  @override
  void dispose() {
    _field.dispose();
    super.dispose();
  }

  void _save(String? name) {
    widget.relay.rename(widget.contact.userId, name);
    Navigator.pop(context);
  }

  @override
  Widget build(BuildContext context) {
    final certName = widget.relay.certName(widget.contact.userId);
    return AlertDialog(
      title: const Text('Как показывать'),
      content: TextField(
        controller: _field,
        autofocus: true,
        decoration: InputDecoration(
          labelText: 'Имя',
          helperText: 'Видно только на этом устройстве. В сертификате: $certName',
          helperMaxLines: 2,
        ),
        onSubmitted: _save,
      ),
      actions: [
        if (widget.relay.hasAlias(widget.contact.userId))
          TextButton(onPressed: () => _save(null), child: const Text('Вернуть исходное')),
        TextButton(onPressed: () => Navigator.pop(context), child: const Text('Отмена')),
        FilledButton(onPressed: () => _save(_field.text), child: const Text('Сохранить')),
      ],
    );
  }
}
