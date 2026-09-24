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
        if (call.method == 'openChat') onOpenChat?.call(call.arguments as String);
        if (call.method == 'pushToken') onPushToken?.call(call.arguments as String);
        if (call.method == 'wake') onWake?.call();
      });
    }
  }

  static const _channel = MethodChannel('botato/call');

  /// 'mute', 'hangup', 'accept' or 'decline' from the notifications.
  void Function(String action)? onAction;

  /// New FCM token (Android), to register with the relay.
  void Function(String token)? onPushToken;

  /// A push arrived while the app was running: the relay socket may be stale.
  void Function()? onWake;

  /// Current FCM token, or null (no Google services, not Android).
  Future<String?> pushToken() async {
    if (!Platform.isAndroid) return null;
    try {
      return await _channel.invokeMethod<String>('pushToken');
    } catch (_) {
      return null;
    }
  }

  /// Tap on a message notification: the peer user id.
  void Function(String peerUser)? onOpenChat;

  /// New-message notification (Android). The text stays on the device.
  Future<void> showMessage(String peerUser, String name, String text) =>
      _invoke('message', {'peer': peerUser, 'name': name, 'text': text});

  Future<void> clearMessages(String peerUser) => _invoke('messageClear', {'peer': peerUser});

  /// Chat whose notification launched the app (cold start), if any.
  Future<String?> pendingChat() async {
    if (!Platform.isAndroid) return null;
    try {
      return await _channel.invokeMethod<String>('pendingChat');
    } on MissingPluginException {
      return null;
    }
  }

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

  /// Ringing notification with accept / decline (full-screen over the lock screen).
  Future<void> showIncoming(String peer, {required bool video}) => _invoke('incoming', {'peer': peer, 'video': video});

  Future<void> cancelIncoming() => _invoke('incomingCancel', null);

  bool _proximity = false;

  /// Screen off at the ear (Android): wanted during a call when the loudspeaker is off;
  /// the native side also requires the earpiece to be the active route.
  Future<void> proximity(bool on) async {
    if (_proximity == on) return;
    _proximity = on;
    await _invoke('proximity', on);
  }

  Future<void> hide() {
    _camera = false;
    _screen = false;
    _proximity = false;
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
