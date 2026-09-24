import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:path_provider/path_provider.dart';

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
  ConnState state = ConnState.connecting;
  String detail = '';
  List<core.ContactInfo> contacts = [];

  final _signals = StreamController<IncomingSignal>.broadcast();
  Stream<IncomingSignal> get signals => _signals.stream;

  final _turn = StreamController<TurnCreds>.broadcast();
  StreamSubscription<core.ClientEvent>? _sub;

  String? get myDevice => core.clientMyDevice();

  Future<void> start(Uint8List secret, String cert) async {
    final dir = await getApplicationSupportDirectory();
    await _sub?.cancel();
    _sub = core.clientStart(dataDir: dir.path, secret: secret, cert: cert).listen(_onEvent);
    contacts = core.clientContacts();
    notifyListeners();
  }

  void _onEvent(core.ClientEvent e) {
    switch (e) {
      case core.ClientEvent_Connecting(:final server):
        state = ConnState.connecting;
        detail = server;
      case core.ClientEvent_Online(:final server):
        state = ConnState.online;
        detail = server;
      case core.ClientEvent_Offline(:final reason):
        state = ConnState.offline;
        detail = reason;
      case core.ClientEvent_ContactsChanged():
        contacts = core.clientContacts();
      case core.ClientEvent_Signal(:final fromUser, :final fromDevice, :final payload):
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
      case core.ClientEvent_Error(:final message):
        detail = message;
    }
    notifyListeners();
  }

  /// Sends a call-signal JSON object to one device (sealed and signed in Rust).
  void signal(String toDevice, Map<String, dynamic> data) {
    core.clientSignal(toDevice: toDevice, payload: jsonEncode(data));
  }

  void refreshContacts() => core.clientRefreshDirectory();

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
    core.clientStop();
    super.dispose();
  }
}
