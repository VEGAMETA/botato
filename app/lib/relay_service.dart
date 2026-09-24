import 'dart:async';
import 'dart:convert';

import 'package:connectivity_plus/connectivity_plus.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart' show PlatformInt64Util;
import 'package:path_provider/path_provider.dart';

import 'call/media_settings.dart';
import 'src/rust/api/client.dart' as core;

enum ConnState { connecting, online, offline }

/// An authenticated call signal (already decrypted and verified in Rust).
class IncomingSignal {
  IncomingSignal(this.fromUser, this.fromDevice, this.data);
  final String fromUser;
  final String fromDevice;
  final Map<String, dynamic> data;
}

class TurnCreds {
  TurnCreds(this.urls, this.username, this.credential);
  final List<String> urls;
  final String username;
  final String credential;
}

/// Owns the Rust relay client and exposes its state to the UI.
class RelayService extends ChangeNotifier {
  RelayService(this.settings);

  final MediaSettings settings;

  /// Contacts shown in the list: only those the user added (or who called or wrote).
  List<core.ContactInfo> get visibleContacts {
    final added = settings.addedContacts ?? const {};
    return contacts.where((c) => !c.isMe && added.contains(c.userId)).toList();
  }

  bool isAdded(String userId) => settings.addedContacts?.contains(userId) ?? false;

  /// Known users (directory or local cache) whose name or user id matches [query] exactly,
  /// ignoring case. Only people who have ever connected with a certificate can be found.
  List<core.ContactInfo> findContacts(String query) {
    final q = query.trim().toLowerCase();
    if (q.isEmpty) return const [];
    return contacts.where((c) => !c.isMe && (c.name.toLowerCase() == q || c.userId == q)).toList();
  }

  void add(String userId) {
    final a = settings.addedContacts ?? {};
    if (!a.add(userId)) return;
    settings.addedContacts = a;
    notifyListeners();
  }

  void forget(String userId, {required bool deleteHistory}) {
    settings.addedContacts = (settings.addedContacts ?? {})..remove(userId);
    if (deleteHistory) {
      core.clientDeleteHistory(peerUser: userId);
      _loadConversations();
    }
    notifyListeners();
  }

  /// First run of the added-contacts scheme: keep whoever we already talked to.
  void _migrateContacts() {
    if (settings.addedContacts != null) return;
    final hidden = settings.takeLegacyHidden();
    settings.addedContacts = conversations.keys.where((u) => !hidden.contains(u)).toSet();
  }

  ConnState state = ConnState.connecting;
  String detail = '';
  List<core.ContactInfo> contacts = [];

  /// Last message and unread count per peer user.
  Map<String, core.ConversationInfo> conversations = {};

  /// Peer user whose chat is on screen (no notifications for it while in foreground).
  String? activeChat;
  bool foreground = true;

  /// Called for a new incoming message that the user isn't looking at.
  void Function(String peerUser, String name, String text)? onNewMessage;

  final _messageUpdates = StreamController<String>.broadcast();

  /// Peer users whose history changed.
  Stream<String> get messageUpdates => _messageUpdates.stream;

  final _signals = StreamController<IncomingSignal>.broadcast();
  Stream<IncomingSignal> get signals => _signals.stream;

  final _turn = StreamController<TurnCreds>.broadcast();
  StreamSubscription<core.ClientEvent>? _sub;
  StreamSubscription<List<ConnectivityResult>>? _net;
  List<ConnectivityResult>? _lastNet;

  String? get myDevice => core.clientMyDevice();

  Future<void> start(Uint8List secret, String cert, Uint8List dbKey) async {
    final dir = await getApplicationSupportDirectory();
    await _sub?.cancel();
    _sub = core.clientStart(dataDir: dir.path, secret: secret, cert: cert, dbKey: dbKey).listen(_onEvent);
    // Wi-Fi <-> LTE, VPN on/off: the old socket may be silently dead, dial again right away.
    _net ??= Connectivity().onConnectivityChanged.listen((r) {
      if (_lastNet != null && !listEquals(r, _lastNet)) reconnect();
      _lastNet = r;
    });
    contacts = core.clientContacts();
    _loadConversations();
    _migrateContacts();
    notifyListeners();
  }

  void _loadConversations() {
    conversations = {for (final c in core.clientConversations()) c.peerUser: c};
  }

  void _onEvent(core.ClientEvent e) {
    switch (e) {
      case core.ClientEvent_Connecting(:final server):
        state = ConnState.connecting;
        detail = server;
      case core.ClientEvent_Online(:final server):
        state = ConnState.online;
        detail = server;
        contacts = core.clientContacts();
      case core.ClientEvent_Offline(:final reason):
        state = ConnState.offline;
        detail = reason;
      case core.ClientEvent_ContactsChanged():
        contacts = core.clientContacts();
      case core.ClientEvent_Signal(:final fromUser, :final fromDevice, :final payload):
        add(fromUser); // whoever calls us shows up in the list
        try {
          final data = jsonDecode(payload) as Map<String, dynamic>;
          _signals.add(IncomingSignal(fromUser, fromDevice, data));
        } catch (_) {}
        return;
      case core.ClientEvent_Signaled():
        return;
      case core.ClientEvent_Turn(:final urls, :final username, :final credential):
        _turn.add(TurnCreds(urls, username, credential));
        return;
      case core.ClientEvent_MessagesChanged(:final peerUser):
        _loadConversations();
        _messageUpdates.add(peerUser);
      case core.ClientEvent_NewMessage(:final peerUser, :final text):
        add(peerUser); // whoever writes to us shows up in the list
        if (!foreground || activeChat != peerUser) {
          final name = contacts.where((c) => c.userId == peerUser).firstOrNull?.name ?? 'сообщение';
          onNewMessage?.call(peerUser, name, text);
        }
        return;
      case core.ClientEvent_Error(:final message):
        detail = message;
    }
    notifyListeners();
  }

  /// Sends a call-signal JSON object to one device (sealed and signed in Rust).
  void signal(String toDevice, Map<String, dynamic> data) {
    core.clientSignal(toDevice: toDevice, payload: jsonEncode(data));
  }

  /// Throws with a readable message if the text can't be queued.
  void sendText(String peerUser, String text) => core.clientSendText(peerUser: peerUser, text: text);

  List<core.MessageInfo> messages(String peerUser, {int limit = 200}) =>
      core.clientMessages(peerUser: peerUser, limit: PlatformInt64Util.from(limit));

  void markRead(String peerUser) {
    core.clientMarkRead(peerUser: peerUser);
    _loadConversations();
    notifyListeners();
  }

  void refreshContacts() => core.clientRefreshDirectory();

  void reconnect() => core.clientReconnect();

  /// Fresh TURN credentials from the relay (2 h TTL).
  Future<TurnCreds?> turnCreds() async {
    final next = _turn.stream.first.timeout(const Duration(seconds: 5));
    core.clientRequestTurn();
    try {
      return await next;
    } catch (_) {
      return null;
    }
  }

  List<String> get stunServers => core.clientIceConfig().stun;

  core.ContactInfo? contactOfDevice(String device) {
    for (final c in contacts) {
      if (c.devices.contains(device)) return c;
    }
    return null;
  }

  @override
  void dispose() {
    _sub?.cancel();
    _net?.cancel();
    core.clientStop();
    super.dispose();
  }
}
