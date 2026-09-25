import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../call/call_screen.dart';
import '../call/call_notifier.dart';
import '../call/call_service.dart';
import '../error_log.dart';
import '../relay_service.dart';
import '../update_service.dart';
import '../src/rust/api/client.dart' as core;
import '../src/rust/api/identity.dart' as id;
import '../theme.dart';
import 'chat_screen.dart';

class HomeScreen extends StatefulWidget {
  const HomeScreen({super.key, required this.info, required this.relay, required this.calls, required this.updates});

  final id.CertInfo info;
  final RelayService relay;
  final CallService calls;
  final UpdateService updates;

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  StreamSubscription<IncomingSignal>? _sub;
  final _pingSent = <String, DateTime>{};

  // Search: contacts by shown name, certificate name or id; messages in all chats.
  bool _searching = false;
  final _query = TextEditingController();
  List<core.SearchHit> _msgHits = [];
  Timer? _searchDebounce;

  void _search(String q) {
    setState(() {}); // contacts filter at once
    _searchDebounce?.cancel();
    _searchDebounce = Timer(const Duration(milliseconds: 250), () async {
      final hits = q.trim().isEmpty ? <core.SearchHit>[] : await core.clientSearch(query: q, limit: 100);
      if (mounted && _query.text == q) setState(() => _msgHits = hits);
    });
  }

  void _closeSearch() {
    _searchDebounce?.cancel();
    setState(() {
      _searching = false;
      _msgHits = [];
      _query.clear();
    });
  }

  List<core.ContactInfo> _matchingContacts(String q) {
    final s = q.trim().toLowerCase();
    if (s.isEmpty) return const [];
    final r = widget.relay;
    return [
      for (final c in r.contacts)
        if (!c.isMe &&
            (c.name.toLowerCase().contains(s) ||
                r.certName(c.userId).toLowerCase().contains(s) ||
                c.userId.contains(s)))
          c,
    ];
  }

  Widget _searchResults() {
    final q = _query.text.trim();
    if (q.isEmpty) return const SizedBox.shrink();
    final r = widget.relay;
    final contacts = _matchingContacts(q);
    if (contacts.isEmpty && _msgHits.isEmpty) {
      return const Center(
        child: Text('Ничего не найдено', style: TextStyle(color: BotatoColors.smoke)),
      );
    }
    return ListView(
      children: [
        if (contacts.isNotEmpty) const _SectionLabel('Контакты'),
        for (final c in contacts)
          ListTile(
            leading: Avatar(c.name),
            title: Text(c.name),
            subtitle: Text(
              [
                if (r.certName(c.userId) != c.name) r.certName(c.userId),
                if (!r.isAdded(c.userId)) 'не в списке — нажмите, чтобы открыть чат',
              ].join(' · '),
              style: const TextStyle(color: BotatoColors.smoke, fontSize: 12),
            ),
            onTap: () {
              r.add(c.userId);
              _closeSearch();
              openChat(context, c);
            },
          ),
        if (_msgHits.isNotEmpty) const _SectionLabel('Сообщения'),
        for (final h in _msgHits)
          if (r.contacts.where((c) => c.userId == h.peerUser).firstOrNull case final c?)
            ListTile(
              leading: Avatar(c.name, size: 36),
              title: Text(c.name),
              subtitle: Text(
                '${h.fromMe ? 'Вы: ' : ''}${h.text.replaceAll('\n', ' ')}',
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
              ),
              onTap: () {
                _closeSearch();
                openChat(context, c, jumpTo: h.msgId);
              },
            ),
      ],
    );
  }

  @override
  void initState() {
    super.initState();
    _sub = widget.relay.signals.listen(_onSignal);
    // Tap on a message notification.
    widget.calls.notifier.onOpenChat = (peerUser) async {
      final c = widget.relay.contacts.where((c) => c.userId == peerUser).firstOrNull;
      // During a call the app may be shown over the lock screen: chats only after unlocking.
      if (c == null || !await widget.calls.notifier.ensureUnlocked() || !mounted) return;
      openChat(context, c);
    };
    widget.calls.notifier.onShare = _handleShare;
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      final peer = await widget.calls.notifier.pendingChat();
      if (peer != null) widget.calls.notifier.onOpenChat?.call(peer);
      final shared = await widget.calls.notifier.pendingShare();
      if (shared != null) _handleShare(shared);
      await _checkFullScreen();
    });
  }

  @override
  void dispose() {
    _sub?.cancel();
    _query.dispose();
    _searchDebounce?.cancel();
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

  void openChat(BuildContext context, core.ContactInfo c, {SharedContent? shared, String? jumpTo}) {
    Navigator.of(context).popUntil((r) => r.isFirst);
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) =>
            ChatScreen(contact: c, relay: widget.relay, calls: widget.calls, shared: shared, jumpTo: jumpTo),
      ),
    );
  }

  /// "Share → botato" from another app: pick who gets it, then the chat takes over.
  Future<void> _handleShare(SharedContent shared) async {
    if (shared.isEmpty || !mounted) return;
    Navigator.of(context).popUntil((r) => r.isFirst);
    final contacts = widget.relay.visibleContacts;
    if (contacts.isEmpty) {
      _toast('Некому отправить: сначала добавьте контакт');
      return;
    }
    final to = await showDialog<core.ContactInfo>(
      context: context,
      builder: (ctx) => SimpleDialog(
        title: Text(shared.files.isEmpty ? 'Отправить текст' : 'Отправить фото: ${shared.files.length}'),
        children: [
          for (final c in contacts)
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
    if (to != null && mounted) openChat(context, to, shared: shared);
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

  /// Android 14+ ships full-screen notifications switched off for sideloaded apps: without them
  /// an incoming call on a locked phone is only a notification. Ask once per app start.
  Future<void> _checkFullScreen() async {
    final n = widget.calls.notifier;
    if (!mounted || await n.fullScreenAllowed()) return;
    if (!mounted) return;
    final open = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Звонки на весь экран'),
        content: const Text(
          'Сейчас на заблокированном телефоне входящий звонок приходит только уведомлением. '
          'Разрешите botato полноэкранные уведомления — тогда звонок откроется поверх экрана блокировки.',
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: const Text('Не сейчас')),
          FilledButton(onPressed: () => Navigator.pop(ctx, true), child: const Text('Разрешить')),
        ],
      ),
    );
    if (open == true) await n.openFullScreenSettings();
  }

  Future<void> _showErrorLog(BuildContext context) async {
    final text = ErrorLog.read();
    await showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Журнал ошибок'),
        content: SizedBox(
          width: 560,
          height: 360,
          child: text.isEmpty
              ? const Center(
                  child: Text('Ошибок нет', style: TextStyle(color: BotatoColors.smoke)),
                )
              : SingleChildScrollView(
                  reverse: true,
                  child: SelectableText(text, style: const TextStyle(fontFamily: 'monospace', fontSize: 11)),
                ),
        ),
        actions: [
          if (text.isNotEmpty)
            TextButton(
              onPressed: () {
                ErrorLog.clear();
                Navigator.pop(ctx);
              },
              child: const Text('Очистить'),
            ),
          if (text.isNotEmpty)
            TextButton(
              onPressed: () {
                Clipboard.setData(ClipboardData(text: text));
                _toast('Скопировано');
              },
              child: const Text('Скопировать'),
            ),
          TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('Закрыть')),
        ],
      ),
    );
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
            StatefulBuilder(
              builder: (ctx, set) {
                final s = widget.calls.settings;
                return Column(
                  children: [
                    SwitchListTile(
                      contentPadding: EdgeInsets.zero,
                      title: const Text('Текст в уведомлениях'),
                      subtitle: const Text('Выкл.: только «новое сообщение», без имени и текста'),
                      value: s.notifyPreview,
                      onChanged: (v) => set(() => s.notifyPreview = v),
                    ),
                    if (Platform.isAndroid)
                      SwitchListTile(
                        contentPadding: EdgeInsets.zero,
                        title: const Text('Защита экрана'),
                        subtitle: const Text('Скрыть чаты в «Недавних», запретить скриншоты'),
                        value: s.secureScreen,
                        onChanged: (v) => set(() {
                          s.secureScreen = v;
                          widget.calls.notifier.secureScreen(v);
                        }),
                      ),
                  ],
                );
              },
            ),
            if (Platform.isAndroid)
              FutureBuilder<bool>(
                future: widget.calls.notifier.fullScreenAllowed(),
                builder: (ctx, snap) => snap.data == false
                    ? ListTile(
                        contentPadding: EdgeInsets.zero,
                        leading: const Icon(Icons.warning_amber, color: Colors.orange),
                        title: const Text('Звонки на весь экран выключены'),
                        subtitle: const Text('На заблокированном телефоне звонок придёт только уведомлением'),
                        trailing: TextButton(
                          onPressed: widget.calls.notifier.openFullScreenSettings,
                          child: const Text('Разрешить'),
                        ),
                      )
                    : const SizedBox.shrink(),
              ),
            ListTile(
              contentPadding: EdgeInsets.zero,
              leading: const Icon(Icons.bug_report_outlined),
              title: const Text('Журнал ошибок'),
              onTap: () => _showErrorLog(context),
            ),
            ListTile(
              contentPadding: EdgeInsets.zero,
              title: const Text('Версия', style: TextStyle(color: BotatoColors.smoke, fontSize: 13)),
              subtitle: Text(widget.updates.current, style: const TextStyle(fontSize: 15)),
              trailing: TextButton(
                onPressed: () async {
                  final ok = await widget.updates.check();
                  final u = widget.updates;
                  _toast(
                    u.available != null
                        ? 'Доступна версия ${u.available!.version}'
                        : ok
                        ? 'Установлена последняя версия'
                        : 'Не удалось проверить: ${u.error ?? 'нет связи'}',
                  );
                },
                child: const Text('Проверить'),
              ),
            ),
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
          // Most recent first (last message or when added), then alphabetically.
          ..sort((a, b) {
            final ta = r.recency(a.userId), tb = r.recency(b.userId);
            return ta != tb ? tb.compareTo(ta) : a.name.compareTo(b.name);
          });
        if (_searching) {
          return PopScope(
            canPop: false,
            onPopInvokedWithResult: (didPop, _) {
              if (!didPop) _closeSearch();
            },
            child: Scaffold(
              appBar: AppBar(
                leading: IconButton(icon: const Icon(Icons.arrow_back), onPressed: _closeSearch),
                title: TextField(
                  controller: _query,
                  autofocus: true,
                  decoration: const InputDecoration(
                    hintText: 'Имя, ник, ID или текст сообщения',
                    border: InputBorder.none,
                  ),
                  onChanged: _search,
                ),
              ),
              body: _searchResults(),
            ),
          );
        }
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
                tooltip: 'Поиск',
                icon: const Icon(Icons.search),
                onPressed: () => setState(() => _searching = true),
              ),
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
          body: Column(
            children: [
              _UpdateBanner(updates: widget.updates, onError: _toast),
              Expanded(
                child: others.isEmpty
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
              ),
            ],
          ),
        );
      },
    );
  }
}

/// "A new version is available" above the contact list, with download progress.
class _UpdateBanner extends StatelessWidget {
  const _UpdateBanner({required this.updates, required this.onError});

  final UpdateService updates;
  final void Function(String) onError;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: updates,
      builder: (context, _) {
        final u = updates.available;
        if (u == null) return const SizedBox.shrink();
        final p = updates.progress;
        return Card(
          margin: const EdgeInsets.fromLTRB(12, 8, 12, 0),
          color: BotatoColors.potatoShade.withValues(alpha: 0.5),
          child: Padding(
            padding: const EdgeInsets.fromLTRB(16, 10, 8, 10),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Row(
                  children: [
                    const Icon(Icons.system_update_alt),
                    const SizedBox(width: 12),
                    Expanded(child: Text('Доступна версия ${u.version}  (у вас ${updates.version.version})')),
                    FilledButton(
                      onPressed: p != null
                          ? null
                          : () async {
                              try {
                                await updates.install();
                              } catch (e) {
                                onError(updates.error ?? '$e');
                              }
                            },
                      child: Text(p == null ? 'Обновить' : '${(p * 100).round()}%'),
                    ),
                  ],
                ),
                if (p != null) ...[const SizedBox(height: 8), LinearProgressIndicator(value: p)],
              ],
            ),
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

class _SectionLabel extends StatelessWidget {
  const _SectionLabel(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(16, 12, 16, 4),
    child: Text(
      text,
      style: const TextStyle(color: BotatoColors.smoke, fontSize: 12, fontWeight: FontWeight.w600),
    ),
  );
}
