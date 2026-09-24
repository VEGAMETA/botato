import 'dart:io';

import 'package:flutter/services.dart';

/// Android ongoing-call foreground service + notification (mute / hang up actions).
/// The service also keeps the microphone, camera and screen capture alive in the
/// background. No-op on other platforms.
class CallNotifier {
  CallNotifier() {
    if (Platform.isAndroid) {
      _channel.setMethodCallHandler((call) async {
        if (call.method == 'action') onAction?.call(call.arguments as String);
      });
    }
  }

  static const _channel = MethodChannel('botato/call');

  /// 'mute' or 'hangup' from the notification buttons.
  void Function(String action)? onAction;

  bool _camera = false;
  bool _screen = false;

  Future<void> show(String peer, String status, {required bool muted, bool incoming = false}) => _invoke('show', {
    'peer': peer,
    'status': status,
    'muted': muted,
    'incoming': incoming,
    'camera': _camera,
    'screen': _screen,
  });

  /// Foreground service types must include camera / mediaProjection while those are used.
  Future<void> update({bool? camera, bool? screen}) async {
    _camera = camera ?? _camera;
    _screen = screen ?? _screen;
    await _invoke('types', {'camera': _camera, 'screen': _screen});
  }

  Future<void> hide() {
    _camera = false;
    _screen = false;
    return _invoke('hide', null);
  }

  Future<void> _invoke(String method, Object? args) async {
    if (!Platform.isAndroid) return;
    try {
      await _channel.invokeMethod(method, args);
    } on MissingPluginException {
      // Service not built into this binary.
    }
  }
}
