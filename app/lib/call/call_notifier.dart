import 'dart:io';

import 'package:flutter/services.dart';

import '../desktop.dart';

/// Android ongoing-call foreground service + notification (mute / hang up actions).
/// The service also keeps the microphone, camera and screen capture alive in the
/// background. On desktop, message and incoming-call notifications go to the system
/// notification center; the rest is a no-op there.
class CallNotifier {
  CallNotifier() {
    if (Platform.isAndroid) {
      _channel.setMethodCallHandler((call) async {
        if (call.method == 'action') onAction?.call(call.arguments as String);
        if (call.method == 'openChat') onOpenChat?.call(call.arguments as String);
        if (call.method == 'pushToken') onPushToken?.call(call.arguments as String);
        if (call.method == 'wake') onWake?.call();
        if (call.method == 'share') {
          await _takeNativeShare(); // taken now: a later cold-start query must not repeat it
          final shared = SharedContent.fromMap(call.arguments as Map);
          // On a cold start the home screen may not be up yet: keep it until it asks.
          final handler = onShare;
          handler == null ? _queuedShare = shared : handler(shared);
        }
      });
    }
    if (isDesktop) {
      Desktop.instance.onCallAction = (a) => onAction?.call(a);
      Desktop.instance.onOpenChat = (p) => onOpenChat?.call(p);
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

  /// Content another app shared to botato (Android).
  void Function(SharedContent shared)? onShare;

  /// Content shared while the app was not running yet, if any.
  Future<SharedContent?> pendingShare() async {
    if (!Platform.isAndroid) return null;
    final queued = _queuedShare;
    _queuedShare = null;
    return queued ?? await _takeNativeShare();
  }

  SharedContent? _queuedShare;

  Future<SharedContent?> _takeNativeShare() async {
    try {
      final m = await _channel.invokeMethod<Map>('pendingShare');
      return m == null ? null : SharedContent.fromMap(m);
    } on MissingPluginException {
      return null;
    }
  }

  /// Android: true if the phone is unlocked, or the user unlocked it now (system prompt).
  /// A call shown over the lock screen must not become a way into the chats.
  Future<bool> ensureUnlocked() async {
    if (!Platform.isAndroid) return true;
    try {
      return await _channel.invokeMethod<bool>('unlock') ?? false;
    } catch (_) {
      return false;
    }
  }

  /// Android 12+: asks for the Bluetooth permission (call audio through headsets) if missing.
  Future<void> requestBluetooth() => _invoke('requestBluetooth', null);

  /// Android 14+: may incoming calls take the full screen on a locked phone?
  Future<bool> fullScreenAllowed() async {
    if (!Platform.isAndroid) return true;
    try {
      return await _channel.invokeMethod<bool>('fullScreenAllowed') ?? true;
    } catch (_) {
      return true;
    }
  }

  /// Opens the system page where the user allows full-screen notifications for botato.
  Future<void> openFullScreenSettings() => _invoke('openFullScreenSettings', null);

  /// Android: FLAG_SECURE on the app window (recent-apps preview hidden, no screenshots).
  Future<void> secureScreen(bool on) => _invoke('secureScreen', on);

  /// Android: the system share sheet for a file in the app's cache `share/` folder.
  static Future<void> shareFile(String path, String mime) async {
    try {
      await _channel.invokeMethod('shareFile', {'path': path, 'mime': mime});
    } on PlatformException catch (e) {
      throw e.message ?? e.code;
    }
  }

  /// Tap on a message notification: the peer user id.
  void Function(String peerUser)? onOpenChat;

  /// New-message notification (Android). The text stays on the device.
  Future<void> showMessage(String peerUser, String name, String text) => isDesktop
      ? Desktop.instance.showMessage(peerUser, name, text)
      : _invoke('message', {'peer': peerUser, 'name': name, 'text': text});

  Future<void> clearMessages(String peerUser) =>
      isDesktop ? Desktop.instance.clearMessages(peerUser) : _invoke('messageClear', {'peer': peerUser});

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
  Future<void> showIncoming(String peer, {required bool video}) => isDesktop
      ? Desktop.instance.showIncoming(peer, video: video)
      : _invoke('incoming', {'peer': peer, 'video': video});

  Future<void> cancelIncoming() => isDesktop ? Desktop.instance.cancelIncoming() : _invoke('incomingCancel', null);

  bool _proximity = false;

  /// Android: applies the call's audio mode right away (communication, or media for clean audio).
  Future<void> audioMode({required bool communication}) => _invoke('audioMode', communication);

  /// Screen off at the ear (Android): wanted during a call when the loudspeaker is off;
  /// the native side also requires the earpiece to be the active route.
  Future<void> proximity(bool on) async {
    if (_proximity == on) return;
    _proximity = on;
    await _invoke('proximity', on);
  }

  /// Android: saves an image to the gallery (Pictures/botato). Throws with a readable message.
  static Future<void> saveToGallery(String name, Uint8List data, String mime) async {
    try {
      await _channel.invokeMethod('saveImage', {'name': name, 'data': data, 'mime': mime});
    } on PlatformException catch (e) {
      throw e.message ?? e.code;
    }
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

/// What another app shared: text and/or image files copied into the app's cache.
class SharedContent {
  SharedContent(this.text, this.files);

  factory SharedContent.fromMap(Map m) =>
      SharedContent(m['text'] as String?, [for (final f in (m['files'] as List? ?? const [])) f as String]);

  final String? text;
  final List<String> files;

  bool get isEmpty => (text == null || text!.trim().isEmpty) && files.isEmpty;
}
