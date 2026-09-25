import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/widgets.dart';
import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';

import 'desktop.dart';
import 'src/rust/api/update.dart' as upd;
import 'l10n.dart';

/// Offers app updates published on the relay host. The release list and the files come from
/// an untrusted server; Rust checks the admin signature and the file's SHA-256 before anything
/// is handed to the OS installer (protocol/README.md §3b).
class UpdateService extends ChangeNotifier with WidgetsBindingObserver {
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

  /// The downloaded and verified file of [available] (kept: no second download when the
  /// user first had to allow installs, or taps "Update" again).
  String? _ready;

  /// Android sent the user to allow installs from botato: continue when they come back.
  bool _awaitingPermission = false;

  String get current => '${version.version} (${version.build})';

  void start() {
    WidgetsBinding.instance.addObserver(this);
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

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    // Back from the "install unknown apps" settings: carry on with the file we have.
    if (state == AppLifecycleState.resumed && _awaitingPermission && _ready != null) {
      _awaitingPermission = false;
      install().catchError((_) {});
    }
  }

  /// The system installer refused the update (Android).
  void installFailed(String message) {
    error = message;
    notifyListeners();
  }

  /// Downloads (unless already done), verifies and starts the installer. Throws with a
  /// readable message.
  Future<void> install() async {
    final info = available;
    if (info == null || progress != null) return;
    progress = 0;
    error = null;
    notifyListeners();
    try {
      final dir = Directory('${(await getTemporaryDirectory()).path}${Platform.pathSeparator}updates');
      final path = '${dir.path}${Platform.pathSeparator}${info.file}';
      // A file kept from before is checked again: it sat on disk meanwhile.
      final kept =
          _ready == path &&
          File(path).existsSync() &&
          await upd.updateVerify(path: path, size: info.size, sha256: info.sha256);
      if (!kept) {
        _ready = null;
        if (dir.existsSync()) dir.deleteSync(recursive: true);
        dir.createSync(recursive: true);
        await _download(Uri.parse(info.url), File(path), info.size.toInt());
        if (!await upd.updateVerify(path: path, size: info.size, sha256: info.sha256)) {
          File(path).deleteSync();
          throw tr(
            'файл не совпал с подписанным релизом — установка отменена',
            'the file doesn\'t match the signed release — installation cancelled',
          );
        }
        _ready = path;
      }
      if (Platform.isAndroid) {
        try {
          await _channel.invokeMethod('installApk', {'path': path});
        } on PlatformException catch (e) {
          if (e.code == 'permission') _awaitingPermission = true;
          rethrow;
        }
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
        if (got > size) throw tr('файл больше подписанного', 'the file is larger than the signed one');
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
    WidgetsBinding.instance.removeObserver(this);
    _timer?.cancel();
    super.dispose();
  }
}
