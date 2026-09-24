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
import 'src/rust/api/identity.dart' as core;
import 'src/rust/frb_generated.dart';
import 'theme.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  if (Platform.isWindows || Platform.isLinux || Platform.isMacOS) await windowManager.ensureInitialized();
  await RustLib.init();
  final media = await MediaSettings.load();
  if (Platform.isAndroid) {
    // The phone's own AEC/NS is chosen when WebRTC starts and can't change mid-call.
    await WebRTC.initialize(options: {'androidUseHardwareAudioProcessing': media.hwAudioProcessing});
  }
  final store = IdentityStore();
  final secret = await store.deviceSecret();
  String? cert = await store.cert();
  core.CertInfo? info;
  if (cert != null) {
    try {
      final (normalized, i) = core.certImport(secret: secret, text: cert);
      cert = normalized;
      info = i;
    } catch (_) {
      // Expired or otherwise invalid: go through setup again.
      cert = null;
    }
  }
  runApp(BotatoApp(store: store, media: media, secret: secret, cert: cert, info: info));
}

class BotatoApp extends StatefulWidget {
  const BotatoApp({super.key, required this.store, required this.media, required this.secret, this.cert, this.info});

  final IdentityStore store;
  final MediaSettings media;
  final Uint8List secret;
  final String? cert;
  final core.CertInfo? info;

  @override
  State<BotatoApp> createState() => _BotatoAppState();
}

class _BotatoAppState extends State<BotatoApp> with WidgetsBindingObserver {
  late final _relay = RelayService(widget.media);
  final _navigator = GlobalKey<NavigatorState>();
  late final _calls = CallService(_relay, _navigator, widget.media);
  late core.CertInfo? _info = widget.info;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _relay.onNewMessage = (peer, name, text) => _calls.notifier.showMessage(peer, name, text);
    if (widget.cert != null) _startRelay(widget.cert!);
  }

  Future<void> _startRelay(String cert) async {
    await _relay.start(widget.secret, cert, await widget.store.dbKey());
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    _relay.foreground = state == AppLifecycleState.resumed;
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _calls.dispose();
    _relay.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'botato',
      navigatorKey: _navigator,
      theme: botatoTheme(Brightness.light),
      darkTheme: botatoTheme(Brightness.dark),
      themeMode: ThemeMode.dark,
      home: _info == null
          ? SetupScreen(
              store: widget.store,
              secret: widget.secret,
              onCert: (cert, info) {
                _startRelay(cert);
                setState(() => _info = info);
              },
            )
          : HomeScreen(info: _info!, relay: _relay, calls: _calls),
    );
  }
}
