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
import '../l10n.dart';

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
      return Center(
        child: Text(tr('Ничего не найдено', 'Nothing found'), style: TextStyle(color: BotatoColors.smoke)),
      );
    }
    return ListView(
      children: [
        if (contacts.isNotEmpty) _SectionLabel(tr('Контакты', 'Contacts')),
        for (final c in contacts)
          ListTile(
            leading: Avatar(c.name),
            title: Text(c.name),
            subtitle: Text(
              [
                if (r.certName(c.userId) != c.name) r.certName(c.userId),
                if (!r.isAdded(c.userId))
                  tr('не в списке — нажмите, чтобы открыть чат', 'not in the list — tap to open the chat'),
              ].join(' · '),
              style: const TextStyle(color: BotatoColors.smoke, fontSize: 12),
            ),
            onTap: () {
              r.add(c.userId);
              _closeSearch();
              openChat(context, c);
            },
          ),
        if (_msgHits.isNotEmpty) _SectionLabel(tr('Сообщения', 'Messages')),
        for (final h in _msgHits)
          if (r.contacts.where((c) => c.userId == h.peerUser).firstOrNull case final c?)
            ListTile(
              leading: Avatar(c.name, size: 36),
              title: Text(c.name),
              subtitle: Text(
                '${h.fromMe ? tr('Вы: ', 'You: ') : ''}${h.text.replaceAll('\n', ' ')}',
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
      await widget.calls.notifier.requestBluetooth();
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
        _toast(tr('ping от $who', 'ping from $who'));
      case 'pong':
        final sent = _pingSent.remove(s.fromDevice);
        final ms = sent == null ? '?' : DateTime.now().difference(sent).inMilliseconds;
        _toast(tr('pong от $who: $ms мс', 'pong from $who: $ms ms'));
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
      _toast(tr('Некому отправить: сначала добавьте контакт', 'Nobody to send to: add a contact first'));
      return;
    }
    final to = await showDialog<core.ContactInfo>(
      context: context,
      builder: (ctx) => SimpleDialog(
        title: Text(
          shared.files.isEmpty
              ? tr('Отправить текст', 'Send text')
              : tr('Отправить фото: ${shared.files.length}', 'Send photos: ${shared.files.length}'),
        ),
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
          title: Text(tr('Забыть «${c.name}»?', 'Forget “${c.name}”?')),
          content: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                tr(
                  'Контакт пропадёт из списка. Если он напишет или позвонит — вернётся сам.',
                  'The contact disappears from the list. If they write or call, they come back by themselves.',
                ),
              ),
              CheckboxListTile(
                contentPadding: EdgeInsets.zero,
                value: deleteHistory,
                onChanged: (v) => set(() => deleteHistory = v ?? false),
                title: Text(tr('Удалить переписку на этом устройстве', 'Delete the conversation on this device')),
              ),
            ],
          ),
          actions: [
            TextButton(onPressed: () => Navigator.pop(ctx, false), child: Text(tr('Отмена', 'Cancel'))),
            FilledButton(onPressed: () => Navigator.pop(ctx, true), child: Text(tr('Забыть', 'Forget'))),
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
    _toast(
      added.length == 1
          ? tr('Добавлен: ${added.first.name}', 'Added: ${added.first.name}')
          : tr('Добавлено: ${added.length}', 'Added: ${added.length}'),
    );
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
        title: Text(tr('Звонки на весь экран', 'Full-screen calls')),
        content: Text(
          tr(
            'Сейчас на заблокированном телефоне входящий звонок приходит только уведомлением. Разрешите botato полноэкранные уведомления — тогда звонок откроется поверх экрана блокировки.',
            'On a locked phone an incoming call now arrives only as a notification. Allow botato full-screen notifications, and the call will open over the lock screen.',
          ),
        ),
        actions: [
          TextButton(onPressed: () => Navigator.pop(ctx, false), child: Text(tr('Не сейчас', 'Not now'))),
          FilledButton(onPressed: () => Navigator.pop(ctx, true), child: Text(tr('Разрешить', 'Allow'))),
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
        title: Text(tr('Журнал ошибок', 'Error log')),
        content: SizedBox(
          width: 560,
          height: 360,
          child: text.isEmpty
              ? Center(
                  child: Text(tr('Ошибок нет', 'No errors'), style: TextStyle(color: BotatoColors.smoke)),
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
              child: Text(tr('Очистить', 'Clear')),
            ),
          if (text.isNotEmpty)
            TextButton(
              onPressed: () {
                Clipboard.setData(ClipboardData(text: text));
                _toast(tr('Скопировано', 'Copied'));
              },
              child: Text(tr('Скопировать', 'Copy')),
            ),
          TextButton(onPressed: () => Navigator.pop(ctx), child: Text(tr('Закрыть', 'Close'))),
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
        tooltip: tr('Скопировать', 'Copy'),
        icon: const Icon(Icons.copy, size: 20),
        onPressed: () {
          Clipboard.setData(ClipboardData(text: value));
          _toast(tr('Скопировано', 'Copied'));
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
              row(
                widget.relay.state == ConnState.connecting
                    ? tr('Подключение к', 'Connecting to')
                    : tr('Нет связи', 'No connection'),
                widget.relay.detail,
              ),
            row(tr('ID пользователя', 'User ID'), widget.info.userId),
            StatefulBuilder(
              builder: (ctx, set) {
                final s = widget.calls.settings;
                return Column(
                  children: [
                    ListTile(
                      contentPadding: EdgeInsets.zero,
                      title: const Text('Язык / Language'),
                      trailing: DropdownButton<AppLanguage>(
                        value: Lang.instance.setting,
                        underline: const SizedBox(),
                        items: [
                          DropdownMenuItem(value: AppLanguage.system, child: Text(tr('Системный', 'System'))),
                          const DropdownMenuItem(value: AppLanguage.ru, child: Text('Русский')),
                          const DropdownMenuItem(value: AppLanguage.en, child: Text('English')),
                        ],
                        onChanged: (l) async {
                          if (l == null) return;
                          await Lang.instance.set(l);
                          set(() {});
                        },
                      ),
                    ),
                    SwitchListTile(
                      contentPadding: EdgeInsets.zero,
                      title: Text(tr('Текст в уведомлениях', 'Text in notifications')),
                      subtitle: Text(
                        tr(
                          'Выкл.: только «новое сообщение», без имени и текста',
                          'Off: only “new message”, no name or text',
                        ),
                      ),
                      value: s.notifyPreview,
                      onChanged: (v) => set(() => s.notifyPreview = v),
                    ),
                    if (Platform.isAndroid)
                      SwitchListTile(
                        contentPadding: EdgeInsets.zero,
                        title: Text(tr('Защита экрана', 'Screen protection')),
                        subtitle: Text(
                          tr(
                            'Скрыть чаты в «Недавних», запретить скриншоты',
                            'Hide chats in Recents, block screenshots',
                          ),
                        ),
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
                        title: Text(tr('Звонки на весь экран выключены', 'Full-screen calls are off')),
                        subtitle: Text(
                          tr(
                            'На заблокированном телефоне звонок придёт только уведомлением',
                            'On a locked phone a call arrives only as a notification',
                          ),
                        ),
                        trailing: TextButton(
                          onPressed: widget.calls.notifier.openFullScreenSettings,
                          child: Text(tr('Разрешить', 'Allow')),
                        ),
                      )
                    : const SizedBox.shrink(),
              ),
            ListTile(
              contentPadding: EdgeInsets.zero,
              leading: const Icon(Icons.bug_report_outlined),
              title: Text(tr('Журнал ошибок', 'Error log')),
              onTap: () => _showErrorLog(context),
            ),
            ListTile(
              contentPadding: EdgeInsets.zero,
              title: Text(tr('Версия', 'Version'), style: TextStyle(color: BotatoColors.smoke, fontSize: 13)),
              subtitle: Text(widget.updates.current, style: const TextStyle(fontSize: 15)),
              trailing: TextButton(
                onPressed: () async {
                  final ok = await widget.updates.check();
                  final u = widget.updates;
                  _toast(
                    u.available != null
                        ? tr('Доступна версия ${u.available!.version}', 'Version ${u.available!.version} is available')
                        : ok
                        ? tr('Установлена последняя версия', 'The latest version is installed')
                        : '${tr('Не удалось проверить', "Couldn't check")}: ${u.error ?? tr('нет связи', 'no connection')}',
                  );
                },
                child: Text(tr('Проверить', 'Check')),
              ),
            ),
            if (device != null) row(tr('ID этого устройства', 'This device ID'), device),
          ],
        ),
        actions: [TextButton(onPressed: () => Navigator.pop(ctx), child: Text(tr('Закрыть', 'Close')))],
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
          ConnState.online => (BotatoColors.ok, tr('в сети', 'online')),
          ConnState.connecting => (Colors.amber, tr('подключение…', 'connecting…')),
          ConnState.offline => (BotatoColors.danger, tr('нет связи', 'no connection')),
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
                  decoration: InputDecoration(
                    hintText: tr('Имя, ник, ID или текст сообщения', 'Name, nickname, ID or message text'),
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
                tooltip: tr('Поиск', 'Search'),
                icon: const Icon(Icons.search),
                onPressed: () => setState(() => _searching = true),
              ),
              IconButton(
                tooltip: tr('Добавить контакт', 'Add contact'),
                icon: const Icon(Icons.person_add_alt),
                onPressed: () => _addContact(context),
              ),
              IconButton(
                tooltip: tr('Звук и камера', 'Audio and camera'),
                icon: const Icon(Icons.tune),
                onPressed: () => showMediaSettings(context, widget.calls),
              ),
              IconButton(
                tooltip: tr('Обновить подключение и статусы', 'Refresh connection and statuses'),
                icon: const Icon(Icons.refresh),
                onPressed: r.refresh,
              ),
            ],
          ),
          body: Column(
            children: [
              if (widget.relay.serverDown) const _ServerDownBanner(),
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
/// Our server has been unreachable for a minute: what still works.
class _ServerDownBanner extends StatelessWidget {
  const _ServerDownBanner();

  @override
  Widget build(BuildContext context) {
    return Container(
      width: double.infinity,
      margin: const EdgeInsets.fromLTRB(12, 8, 12, 0),
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
      decoration: BoxDecoration(
        color: BotatoColors.danger.withValues(alpha: 0.15),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        children: [
          const Icon(Icons.cloud_off, color: BotatoColors.danger),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              tr(
                'Сервер недоступен. Звонки идут через резервный канал, если собеседник в приложении; сообщения уйдут, когда связь вернётся.',
                'The server is unreachable. Calls go over the backup channel if the other side has the app open; messages will go out when it is back.',
              ),
            ),
          ),
        ],
      ),
    );
  }
}

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
                    Expanded(
                      child: Text(
                        tr(
                          'Доступна версия ${u.version}  (у вас ${updates.version.version})',
                          'Version ${u.version} is available  (you have ${updates.version.version})',
                        ),
                      ),
                    ),
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
                      child: Text(p == null ? tr('Обновить', 'Update') : '${(p * 100).round()}%'),
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
        ? (peerOnline ? tr('в сети', 'online') : tr('нет сообщений', 'no messages'))
        : '${conv.lastFromMe ? tr('Вы: ', 'You: ') : ''}${conv.lastText.replaceAll('\n', ' ')}';
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
                tooltip: tr('Позвонить', 'Call'),
                icon: const Icon(Icons.call),
                onPressed: enabled ? onCall : null,
              ),
              const SizedBox(width: 6),
              IconButton.filledTonal(
                tooltip: tr('Видеозвонок', 'Video call'),
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
                  PopupMenuItem(value: 'ping', enabled: enabled, child: Text(tr('Проверить связь', 'Test connection'))),
                  PopupMenuItem(value: 'rename', child: Text(tr('Переименовать…', 'Rename…'))),
                  PopupMenuItem(value: 'forget', child: Text(tr('Забыть…', 'Forget…'))),
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
            Text(tr('Пока никого', 'Nobody yet'), style: Theme.of(context).textTheme.titleLarge),
            const SizedBox(height: 6),
            Text(
              online
                  ? tr(
                      'Добавьте контакт по имени: кнопка вверху справа',
                      'Add a contact by name: the button at the top right',
                    )
                  : tr('Ждём связи с сервером…', 'Waiting for the server…'),
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
      setState(
        () => _error = tr(
          'Такого пользователя нет. Он должен хотя бы раз подключиться с сертификатом.',
          'No such user. They have to connect with their certificate at least once.',
        ),
      );
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
      title: Text(tr('Добавить контакт', 'Add contact')),
      content: TextField(
        controller: _field,
        autofocus: true,
        decoration: InputDecoration(labelText: tr('Имя или ID', 'Name or ID'), errorText: _error, errorMaxLines: 3),
        onChanged: (_) {
          if (_error != null) setState(() => _error = null);
        },
        onSubmitted: (_) => _submit(),
      ),
      actions: [
        TextButton(onPressed: () => Navigator.pop(context), child: Text(tr('Отмена', 'Cancel'))),
        FilledButton(onPressed: _submit, child: Text(tr('Добавить', 'Add'))),
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
      title: Text(tr('Как показывать', 'Display name')),
      content: TextField(
        controller: _field,
        autofocus: true,
        decoration: InputDecoration(
          labelText: tr('Имя', 'Name'),
          helperText: tr(
            'Видно только на этом устройстве. В сертификате: $certName',
            'Visible only on this device. In the certificate: $certName',
          ),
          helperMaxLines: 2,
        ),
        onSubmitted: _save,
      ),
      actions: [
        if (widget.relay.hasAlias(widget.contact.userId))
          TextButton(onPressed: () => _save(null), child: Text(tr('Вернуть исходное', 'Restore original'))),
        TextButton(onPressed: () => Navigator.pop(context), child: Text(tr('Отмена', 'Cancel'))),
        FilledButton(onPressed: () => _save(_field.text), child: Text(tr('Сохранить', 'Save'))),
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
