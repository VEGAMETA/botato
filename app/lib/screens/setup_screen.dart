import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:qr_flutter/qr_flutter.dart';

import '../identity_store.dart';
import '../src/rust/api/identity.dart' as core;

/// First launch: show the enrollment request, then accept the admin's certificate.
class SetupScreen extends StatefulWidget {
  const SetupScreen({super.key, required this.store, required this.secret, required this.onCert});

  final IdentityStore store;
  final Uint8List secret;
  final void Function(String cert, core.CertInfo info) onCert;

  @override
  State<SetupScreen> createState() => _SetupScreenState();
}

class _SetupScreenState extends State<SetupScreen> {
  final _name = TextEditingController();
  final _cert = TextEditingController();
  late final core.DeviceInfo _device = core.deviceInfo(secret: widget.secret);
  String? _request;
  String? _error;

  @override
  void initState() {
    super.initState();
    widget.store.requestedName().then((n) {
      if (!mounted) return;
      _name.text = n;
      _rebuildRequest();
    });
  }

  void _rebuildRequest() {
    setState(() => _request = core.enrollmentRequest(secret: widget.secret, name: _name.text));
  }

  Future<void> _import() async {
    try {
      final (text, info) = core.certImport(secret: widget.secret, text: _cert.text);
      await widget.store.saveCert(text);
      widget.onCert(text, info);
    } catch (e) {
      setState(() => _error = e.toString());
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      appBar: AppBar(title: const Text('Подключение устройства')),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          const Text('1. Как вас подписать?'),
          TextField(
            controller: _name,
            maxLength: 64,
            decoration: const InputDecoration(hintText: 'Имя'),
            onChanged: (v) {
              widget.store.saveRequestedName(v);
              _rebuildRequest();
            },
          ),
          const SizedBox(height: 8),
          const Text('2. Отправьте запрос админу (QR или текст):'),
          const SizedBox(height: 8),
          if (_request != null) ...[
            Center(
              child: Container(
                color: Colors.white,
                padding: const EdgeInsets.all(8),
                child: QrImageView(data: _request!, size: 260),
              ),
            ),
            const SizedBox(height: 8),
            OutlinedButton.icon(
              icon: const Icon(Icons.copy),
              label: const Text('Скопировать запрос'),
              onPressed: () => Clipboard.setData(ClipboardData(text: _request!)),
            ),
          ],
          const SizedBox(height: 16),
          const Text('3. Сверьте с админом голосом этот отпечаток:'),
          const SizedBox(height: 8),
          SelectableText(
            _device.fingerprint,
            textAlign: TextAlign.center,
            style: theme.textTheme.headlineSmall?.copyWith(fontFamily: 'monospace', letterSpacing: 2),
          ),
          const SizedBox(height: 24),
          const Text('4. Вставьте сертификат от админа:'),
          TextField(
            controller: _cert,
            minLines: 2,
            maxLines: 5,
            decoration: const InputDecoration(hintText: 'botato-cert1:…'),
          ),
          if (_error != null)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Text(_error!, style: TextStyle(color: theme.colorScheme.error)),
            ),
          const SizedBox(height: 8),
          FilledButton(onPressed: _import, child: const Text('Импортировать')),
        ],
      ),
    );
  }
}
