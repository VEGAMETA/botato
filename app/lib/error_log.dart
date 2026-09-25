import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:path_provider/path_provider.dart';

/// The app's error journal: `<app support dir>/errors.log`, written by Dart (uncaught errors),
/// Kotlin (uncaught exceptions) and Rust (panics). Stays on the device; shown and copied from
/// the profile dialog to diagnose crashes. Errors only, no message contents.
class ErrorLog {
  static const _max = 256 * 1024;
  static File? _file;

  static Future<void> init() async {
    try {
      _file = File('${(await getApplicationSupportDirectory()).path}${Platform.pathSeparator}errors.log');
    } catch (_) {
      return;
    }
    final previous = FlutterError.onError;
    FlutterError.onError = (d) {
      write('flutter', '${d.exceptionAsString()}\n${d.stack ?? ''}');
      previous?.call(d);
    };
    PlatformDispatcher.instance.onError = (e, st) {
      write('dart', '$e\n$st');
      return false; // keep the default handling (logging)
    };
  }

  static void write(String source, String text) {
    final f = _file;
    if (f == null) return;
    try {
      final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
      final lines = text.split('\n').take(40).join('\n');
      f.writeAsStringSync('$now [$source] $lines\n', mode: FileMode.append, flush: true);
      if (f.lengthSync() > _max) {
        final keep = f.readAsStringSync();
        f.writeAsStringSync(keep.substring(keep.length - _max ~/ 2));
      }
    } catch (_) {}
  }

  /// Newest last; unix timestamps turned into local time.
  static String read() {
    final f = _file;
    if (f == null || !f.existsSync()) return '';
    return f.readAsStringSync().replaceAllMapped(RegExp(r'^(\d{9,11}) ', multiLine: true), (m) {
      final t = DateTime.fromMillisecondsSinceEpoch(int.parse(m[1]!) * 1000);
      String two(int v) => v.toString().padLeft(2, '0');
      return '${t.year}-${two(t.month)}-${two(t.day)} ${two(t.hour)}:${two(t.minute)}:${two(t.second)} ';
    });
  }

  static void clear() {
    try {
      _file?.writeAsStringSync('');
    } catch (_) {}
  }
}
