import 'package:flutter/material.dart';

import '../src/rust/api/identity.dart' as core;

class HomeScreen extends StatelessWidget {
  const HomeScreen({super.key, required this.info, required this.device});

  final core.CertInfo info;
  final core.DeviceInfo device;

  @override
  Widget build(BuildContext context) {
    final expires = DateTime.fromMillisecondsSinceEpoch(info.expiresAt.toInt() * 1000);
    return Scaffold(
      appBar: AppBar(title: Text(info.name)),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          ListTile(title: const Text('Пользователь'), subtitle: Text(info.userId)),
          ListTile(title: const Text('Устройство'), subtitle: Text(device.deviceId)),
          ListTile(title: const Text('Отпечаток'), subtitle: Text(device.fingerprint)),
          ListTile(
            title: const Text('Сертификат'),
            subtitle: Text('устройств: ${info.devices}, действует до ${expires.toLocal().toString().substring(0, 10)}'),
          ),
        ],
      ),
    );
  }
}
