import 'package:flutter/material.dart';

import 'src/rust/api/info.dart';
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  await RustLib.init();
  runApp(const BotatoApp());
}

class BotatoApp extends StatelessWidget {
  const BotatoApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'botato',
      home: Scaffold(
        appBar: AppBar(title: const Text('botato')),
        body: Center(child: Text(coreInfo())),
      ),
    );
  }
}
