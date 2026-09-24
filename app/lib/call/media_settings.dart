import 'dart:convert';

import 'package:shared_preferences/shared_preferences.dart';

enum AudioMode { voice, clean }

/// Per-device media preferences (local only).
class MediaSettings {
  MediaSettings._(this._prefs);

  static Future<MediaSettings> load() async => MediaSettings._(await SharedPreferences.getInstance());

  final SharedPreferences _prefs;

  String? get micId => _prefs.getString('mic');
  set micId(String? v) => _set('mic', v);

  String? get cameraId => _prefs.getString('camera');
  set cameraId(String? v) => _set('camera', v);

  String? get outputId => _prefs.getString('output');
  set outputId(String? v) => _set('output', v);

  AudioMode get audioMode => _prefs.getString('audio_mode') == 'clean' ? AudioMode.clean : AudioMode.voice;
  set audioMode(AudioMode m) => _prefs.setString('audio_mode', m.name);

  /// Contacts the user added (user ids); null before the first run of this scheme. Local only.
  Set<String>? get addedContacts => _prefs.getStringList('added_contacts')?.toSet();
  set addedContacts(Set<String>? v) => _prefs.setStringList('added_contacts', (v ?? const {}).toList());

  /// Local display names for other users (user id -> name). Never leave the device.
  Map<String, String> get contactAliases {
    try {
      return Map<String, String>.from(jsonDecode(_prefs.getString('contact_aliases') ?? '{}') as Map);
    } catch (_) {
      return {};
    }
  }

  set contactAliases(Map<String, String> v) => _prefs.setString('contact_aliases', jsonEncode(v));

  /// Forgotten contacts under the old scheme (everyone visible unless hidden); migration only.
  Set<String> takeLegacyHidden() {
    final v = (_prefs.getStringList('hidden_contacts') ?? const []).toSet();
    _prefs.remove('hidden_contacts');
    return v;
  }

  /// Screen sharing frame rate: 30 or 60.
  int get screenFps => _prefs.getInt('screen_fps') ?? 30;
  set screenFps(int v) => _prefs.setInt('screen_fps', v);

  /// Preferred video codec (mime subtype, e.g. "H264"); null = WebRTC's default order.
  /// The caller's choice is sent with the offer and used by both sides.
  String? get videoCodec => _prefs.getString('video_codec');
  set videoCodec(String? v) => _set('video_codec', v);

  /// Android: the phone's own echo/noise processing (applied at app start).
  bool get hwAudioProcessing => _prefs.getBool('hw_audio_processing') ?? true;
  set hwAudioProcessing(bool v) => _prefs.setBool('hw_audio_processing', v);

  void _set(String key, String? v) => v == null ? _prefs.remove(key) : _prefs.setString(key, v);
}

/// Audio capture and send settings per mode (CLAUDE.md §8).
class AudioProfile {
  static Map<String, dynamic> constraints(AudioMode mode, String? deviceId) {
    final processing = mode == AudioMode.voice;
    return {
      'echoCancellation': processing,
      'noiseSuppression': processing,
      'autoGainControl': processing,
      'channelCount': processing ? 1 : 2,
      // Older libwebrtc builds (Android) read the goog* names.
      'optional': [
        {'googEchoCancellation': processing},
        {'googNoiseSuppression': processing},
        {'googAutoGainControl': processing},
        {'googHighpassFilter': processing},
        // Input device (desktop). In audio constraints flutter_webrtc treats `deviceId` as the output.
        if (deviceId != null) {'sourceId': deviceId},
      ],
    };
  }

  /// Opus target, capped under the `maxaveragebitrate` negotiated in SDP.
  static int bitrate(AudioMode mode) => mode == AudioMode.voice ? 48000 : 128000;
}
