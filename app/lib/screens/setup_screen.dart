import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:qr_flutter/qr_flutter.dart';

import '../identity_store.dart';
import '../src/rust/api/identity.dart' as core;
import '../theme.dart';
import '../l10n.dart';

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

  Widget _step(int n, String title, List<Widget> children) {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                CircleAvatar(
                  radius: 13,
                  backgroundColor: BotatoColors.potato.withValues(alpha: 0.2),
                  child: Text('$n', style: const TextStyle(color: BotatoColors.potato, fontSize: 13)),
                ),
                const SizedBox(width: 10),
                Expanded(child: Text(title, style: Theme.of(context).textTheme.titleSmall)),
              ],
            ),
            const SizedBox(height: 12),
            ...children,
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      body: SafeArea(
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 560),
            child: ListView(
              padding: const EdgeInsets.symmetric(vertical: 16),
              children: [
                Image.asset('assets/icon/potato.png', height: 120),
                const SizedBox(height: 8),
                Text('botato', textAlign: TextAlign.center, style: theme.textTheme.headlineMedium),
                const SizedBox(height: 4),
                Text(
                  tr('Подключение устройства', 'Device setup'),
                  textAlign: TextAlign.center,
                  style: TextStyle(color: BotatoColors.smoke),
                ),
                const SizedBox(height: 16),
                _step(1, tr('Как вас подписать?', 'Your name?'), [
                  TextField(
                    controller: _name,
                    maxLength: 64,
                    decoration: InputDecoration(hintText: tr('Имя', 'Name'), border: OutlineInputBorder()),
                    onChanged: (v) {
                      widget.store.saveRequestedName(v);
                      _rebuildRequest();
                    },
                  ),
                ]),
                _step(
                  2,
                  tr(
                    'Отправьте запрос админу — QR или текстом',
                    'Send the request to the admin — as a QR code or text',
                  ),
                  [
                    if (_request != null) ...[
                      Center(
                        child: Container(
                          decoration: BoxDecoration(color: Colors.white, borderRadius: BorderRadius.circular(16)),
                          padding: const EdgeInsets.all(12),
                          child: QrImageView(data: _request!, size: 240),
                        ),
                      ),
                      const SizedBox(height: 12),
                      OutlinedButton.icon(
                        icon: const Icon(Icons.copy),
                        label: Text(tr('Скопировать запрос', 'Copy the request')),
                        onPressed: () => Clipboard.setData(ClipboardData(text: _request!)),
                      ),
                    ],
                  ],
                ),
                _step(
                  3,
                  tr('Сверьте с админом голосом этот отпечаток', 'Compare this fingerprint with the admin by voice'),
                  [
                    SelectableText(
                      _device.fingerprint,
                      textAlign: TextAlign.center,
                      style: theme.textTheme.headlineSmall?.copyWith(
                        fontFamily: 'monospace',
                        letterSpacing: 2,
                        color: BotatoColors.potato,
                      ),
                    ),
                  ],
                ),
                _step(4, tr('Вставьте сертификат от админа', 'Paste the admin\'s certificate'), [
                  TextField(
                    controller: _cert,
                    minLines: 2,
                    maxLines: 5,
                    decoration: const InputDecoration(hintText: 'botato-cert1:…', border: OutlineInputBorder()),
                  ),
                  if (_error != null)
                    Padding(
                      padding: const EdgeInsets.only(top: 8),
                      child: Text(_error!, style: TextStyle(color: theme.colorScheme.error)),
                    ),
                  const SizedBox(height: 12),
                  FilledButton(onPressed: _import, child: Text(tr('Импортировать', 'Import'))),
                ]),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
