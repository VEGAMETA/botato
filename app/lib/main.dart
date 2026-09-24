import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';
import 'package:window_manager/window_manager.dart';

import 'call/call_service.dart';
import 'call/media_settings.dart';
import 'identity_store.dart';
import 'relay_service.dart';
import 'screens/home_screen.dart';
import 'screens/setup_screen.dart';
import 'src/rust/api/client.dart' as client;
import 'src/rust/api/identity.dart' as core;
import 'src/rust/frb_generated.dart';
import 'theme.dart';

/// Everything that must run without a visible screen: on Android a push can start the
/// app headless (no activity), and it must still connect, ring and notify.
class Services {
  Services._(this.store, this.media, this.secret, this.relay, this.calls, this.navigator);

  final IdentityStore store;
  final MediaSettings media;
  final Uint8List secret;
  final RelayService relay;
  final CallService calls;
  final GlobalKey<NavigatorState> navigator;

  String? cert;
  core.CertInfo? info;

  static Future<Services> create() async {
    final media = await MediaSettings.load();
    if (Platform.isAndroid) {
      // The phone's own AEC/NS is chosen when WebRTC starts and can't change mid-call.
      await WebRTC.initialize(options: {'androidUseHardwareAudioProcessing': media.hwAudioProcessing});
    }
    final store = IdentityStore();
    final secret = await store.deviceSecret();
    final relay = RelayService(media);
    final navigator = GlobalKey<NavigatorState>();
    final calls = CallService(relay, navigator, media);
    final s = Services._(store, media, secret, relay, calls, navigator);
    relay.onNewMessage = (peer, name, text) => calls.notifier.showMessage(peer, name, text);
    calls.notifier.onPushToken = (token) => client.clientSetPushToken(token: token);
    calls.notifier.onWake = relay.reconnect;

    final saved = await store.cert();
    if (saved != null) {
      try {
        final (normalized, info) = core.certImport(secret: secret, text: saved);
        await s.startWith(normalized, info);
      } catch (_) {
        // Expired or otherwise invalid: go through setup again.
      }
    }
    return s;
  }

  Future<void> startWith(String cert, core.CertInfo info) async {
    this.cert = cert;
    this.info = info;
    await relay.start(secret, cert, await store.dbKey());
    final token = await calls.notifier.pushToken();
    if (token != null) client.clientSetPushToken(token: token);
  }
}

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  if (Platform.isWindows || Platform.isLinux || Platform.isMacOS) await windowManager.ensureInitialized();
  await RustLib.init();
  final services = await Services.create();
  runApp(BotatoApp(services: services));
}

class BotatoApp extends StatefulWidget {
  const BotatoApp({super.key, required this.services});

  final Services services;

  @override
  State<BotatoApp> createState() => _BotatoAppState();
}

class _BotatoAppState extends State<BotatoApp> with WidgetsBindingObserver {
  Services get s => widget.services;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    // The UI may attach to a call that started while the app ran headless (from a push).
    WidgetsBinding.instance.addPostFrameCallback((_) => s.calls.showScreenIfNeeded());
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    s.relay.foreground = state == AppLifecycleState.resumed;
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final info = s.info;
    return MaterialApp(
      title: 'botato',
      navigatorKey: s.navigator,
      theme: botatoTheme(Brightness.light),
      darkTheme: botatoTheme(Brightness.dark),
      themeMode: ThemeMode.dark,
      home: info == null
          ? SetupScreen(
              store: s.store,
              secret: s.secret,
              onCert: (cert, info) async {
                await s.startWith(cert, info);
                setState(() {});
              },
            )
          : HomeScreen(info: info, relay: s.relay, calls: s.calls),
    );
  }
}
