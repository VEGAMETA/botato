import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';

import 'src/rust/api/identity.dart' as core;

/// Device secret and certificate in platform secure storage
/// (Android Keystore, Windows DPAPI). The secret never leaves the device.
class IdentityStore {
  static const _secretKey = 'device_secret_v1';
  static const _certKey = 'cert_v1';
  static const _nameKey = 'requested_name';

  final _storage = const FlutterSecureStorage();

  /// Loads the device secret, generating it on first launch.
  Future<Uint8List> deviceSecret() async {
    final stored = await _storage.read(key: _secretKey);
    if (stored != null) return base64Decode(stored);
    final fresh = core.deviceGenerate();
    await _storage.write(key: _secretKey, value: base64Encode(fresh));
    return fresh;
  }

  Future<String?> cert() => _storage.read(key: _certKey);

  Future<void> saveCert(String text) => _storage.write(key: _certKey, value: text);

  Future<String> requestedName() async => await _storage.read(key: _nameKey) ?? '';

  Future<void> saveRequestedName(String name) => _storage.write(key: _nameKey, value: name);
}
