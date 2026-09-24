import 'dart:typed_data';

import 'package:flutter/material.dart';

import 'identity_store.dart';
import 'screens/home_screen.dart';
import 'screens/setup_screen.dart';
import 'src/rust/api/identity.dart' as core;
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  final store = IdentityStore();
  final secret = await store.deviceSecret();
  core.CertInfo? info;
  final cert = await store.cert();
  if (cert != null) {
    try {
      info = core.certImport(secret: secret, text: cert).$2;
    } catch (_) {
      // Expired or otherwise invalid: go through setup again.
    }
  }
  runApp(BotatoApp(store: store, secret: secret, info: info));
}

class BotatoApp extends StatefulWidget {
  const BotatoApp({super.key, required this.store, required this.secret, this.info});

  final IdentityStore store;
  final Uint8List secret;
  final core.CertInfo? info;

  @override
  State<BotatoApp> createState() => _BotatoAppState();
}

class _BotatoAppState extends State<BotatoApp> {
  late core.CertInfo? _info = widget.info;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'botato',
      theme: ThemeData(colorSchemeSeed: Colors.deepPurple),
      darkTheme: ThemeData(colorSchemeSeed: Colors.deepPurple, brightness: Brightness.dark),
      home: _info == null
          ? SetupScreen(
              store: widget.store,
              secret: widget.secret,
              onCert: (_, info) => setState(() => _info = info),
            )
          : HomeScreen(info: _info!, device: core.deviceInfo(secret: widget.secret)),
    );
  }
}
