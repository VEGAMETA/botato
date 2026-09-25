import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';

import 'desktop.dart';
import 'src/rust/api/update.dart' as upd;

/// Offers app updates published on the relay host. The release list and the files come from
/// an untrusted server; Rust checks the admin signature and the file's SHA-256 before anything
/// is handed to the OS installer (protocol/README.md §3b).
class UpdateService extends ChangeNotifier {
  static const _channel = MethodChannel('botato/call');
  static const _every = Duration(hours: 6);

  final version = upd.appVersion();

  /// A newer signed release for this platform, if any.
  upd.UpdateInfo? available;

  /// 0..1 while downloading.
  double? progress;
  String? error;
  bool checking = false;
  Timer? _timer;

  String get current => '${version.version} (${version.build})';

  void start() {
    check();
    _timer ??= Timer.periodic(_every, (_) => check());
  }

  /// Returns true if a check completed (with or without an update).
  Future<bool> check() async {
    if (checking) return false;
    checking = true;
    error = null;
    notifyListeners();
    try {
      for (final base in upd.updateBases()) {
        try {
          final bytes = await _get(Uri.parse('${base}release.bin'));
          available = upd.updateCheck(signed: bytes, base: base);
          return true;
        } catch (e) {
          error = '$e';
        }
      }
      return false;
    } finally {
      checking = false;
      notifyListeners();
    }
  }

  Future<Uint8List> _get(Uri url) async {
    final client = HttpClient()..connectionTimeout = const Duration(seconds: 15);
    try {
      final res = await (await client.getUrl(url)).close().timeout(const Duration(seconds: 20));
      if (res.statusCode != 200) throw 'HTTP ${res.statusCode}';
      final b = BytesBuilder(copy: false);
      await for (final chunk in res) {
        b.add(chunk);
      }
      return b.takeBytes();
    } finally {
      client.close();
    }
  }

  /// Downloads, verifies and starts the installer. Throws with a readable message.
  Future<void> install() async {
    final info = available;
    if (info == null || progress != null) return;
    progress = 0;
    error = null;
    notifyListeners();
    try {
      final dir = Directory('${(await getTemporaryDirectory()).path}${Platform.pathSeparator}updates');
      if (dir.existsSync()) dir.deleteSync(recursive: true);
      dir.createSync(recursive: true);
      final path = '${dir.path}${Platform.pathSeparator}${info.file}';
      await _download(Uri.parse(info.url), File(path), info.size.toInt());
      if (!await upd.updateVerify(path: path, size: info.size, sha256: info.sha256)) {
        File(path).deleteSync();
        throw 'файл не совпал с подписанным релизом — установка отменена';
      }
      if (Platform.isAndroid) {
        await _channel.invokeMethod('installApk', {'path': path});
      } else if (Platform.isWindows) {
        await Process.start(path, const [], mode: ProcessStartMode.detached);
        await Desktop.instance.quit(); // the installer replaces our files
      }
    } catch (e) {
      error = e is PlatformException ? (e.message ?? e.code) : '$e';
      rethrow;
    } finally {
      progress = null;
      notifyListeners();
    }
  }

  Future<void> _download(Uri url, File to, int size) async {
    final client = HttpClient()..connectionTimeout = const Duration(seconds: 15);
    final sink = to.openWrite();
    try {
      final res = await (await client.getUrl(url)).close();
      if (res.statusCode != 200) throw 'HTTP ${res.statusCode}';
      var got = 0;
      await for (final chunk in res) {
        sink.add(chunk);
        got += chunk.length;
        if (got > size) throw 'файл больше подписанного';
        progress = size == 0 ? null : got / size;
        notifyListeners();
      }
    } finally {
      await sink.close();
      client.close();
    }
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }
}
