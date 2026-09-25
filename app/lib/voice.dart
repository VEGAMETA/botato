import 'dart:async';
import 'dart:io';
import 'dart:math';

import 'package:audioplayers/audioplayers.dart';
import 'package:flutter/foundation.dart';
import 'package:path_provider/path_provider.dart';
import 'package:record/record.dart';

/// Bars in a voice message's waveform (protocol allows up to 128).
const waveBars = 48;

/// Longest voice message; recording stops by itself there (protocol: 10 min).
const maxVoice = Duration(minutes: 10);

Future<Directory> _voiceDir() async {
  final dir = Directory('${(await getTemporaryDirectory()).path}${Platform.pathSeparator}voice');
  if (!dir.existsSync()) dir.createSync(recursive: true);
  return dir;
}

class RecordedVoice {
  RecordedVoice(this.audio, this.duration, this.wave);

  final Uint8List audio; // AAC-LC in M4A
  final Duration duration;
  final Uint8List wave;
}

/// Records one voice message at a time: AAC-LC, mono, 16 kHz, 32 kbit/s (~240 KB a minute).
class VoiceRecorder extends ChangeNotifier {
  final _rec = AudioRecorder();
  final _levels = <double>[]; // dBFS every 100 ms
  StreamSubscription<Amplitude>? _amp;
  Timer? _tick;
  final _clock = Stopwatch();
  String? _path;

  bool get recording => _path != null;
  Duration get elapsed => _clock.elapsed;

  /// Recent loudness 0..1 for a live indicator.
  double get level => _levels.isEmpty ? 0 : ((_levels.last + 50) / 50).clamp(0.0, 1.0);

  /// Throws a readable message when the microphone is unavailable.
  Future<void> start() async {
    if (recording) return;
    if (!await _rec.hasPermission()) throw 'нет доступа к микрофону';
    final path = '${(await _voiceDir()).path}${Platform.pathSeparator}rec-${DateTime.now().millisecondsSinceEpoch}.m4a';
    await _rec.start(
      const RecordConfig(encoder: AudioEncoder.aacLc, bitRate: 32000, sampleRate: 16000, numChannels: 1),
      path: path,
    );
    _path = path;
    _levels.clear();
    _clock
      ..reset()
      ..start();
    _amp = _rec.onAmplitudeChanged(const Duration(milliseconds: 100)).listen((a) => _levels.add(a.current));
    _tick = Timer.periodic(const Duration(milliseconds: 200), (_) {
      if (_clock.elapsed >= maxVoice) {
        _clock.stop();
      }
      notifyListeners();
    });
    notifyListeners();
  }

  /// Stops and returns the recording (null if too short to be useful).
  Future<RecordedVoice?> stop() async {
    final path = _path;
    if (path == null) return null;
    final duration = _clock.elapsed;
    await _finish();
    final file = File(path);
    try {
      if (duration < const Duration(milliseconds: 500) || !file.existsSync()) return null;
      return RecordedVoice(await file.readAsBytes(), duration, _wave());
    } finally {
      if (file.existsSync()) file.deleteSync();
    }
  }

  Future<void> cancel() async {
    final path = _path;
    await _finish();
    if (path != null && File(path).existsSync()) File(path).deleteSync();
  }

  Future<void> _finish() async {
    _tick?.cancel();
    await _amp?.cancel();
    _clock.stop();
    try {
      await _rec.stop();
    } catch (_) {}
    _path = null;
    notifyListeners();
  }

  /// dBFS samples squeezed into [waveBars] bytes (-50 dB and below = 0, 0 dB = 255).
  Uint8List _wave() {
    final out = Uint8List(waveBars);
    if (_levels.isEmpty) return out;
    for (var i = 0; i < waveBars; i++) {
      final from = i * _levels.length ~/ waveBars;
      final to = max(from + 1, (i + 1) * _levels.length ~/ waveBars);
      final peak = _levels.sublist(from, min(to, _levels.length)).fold<double>(-160, max);
      out[i] = (((peak + 50) / 50).clamp(0.0, 1.0) * 255).round();
    }
    return out;
  }

  @override
  void dispose() {
    cancel();
    _rec.dispose();
    super.dispose();
  }
}

/// Plays one voice message at a time. The decrypted audio lives in a temp file only while it
/// plays (the history keeps it encrypted).
class VoicePlayer extends ChangeNotifier {
  VoicePlayer._() {
    _player.onPositionChanged.listen((p) {
      position = p;
      notifyListeners();
    });
    _player.onPlayerComplete.listen((_) => stop());
  }

  static final instance = VoicePlayer._();

  final _player = AudioPlayer();
  String? playing; // msg id
  bool paused = false;
  Duration position = Duration.zero;
  File? _file;

  bool isPlaying(String msgId) => playing == msgId && !paused;

  Future<void> toggle(String msgId, Future<Uint8List?> Function() load) async {
    if (playing == msgId) {
      paused ? await _player.resume() : await _player.pause();
      paused = !paused;
      notifyListeners();
      return;
    }
    await stop();
    final data = await load();
    if (data == null) return;
    final file = File('${(await _voiceDir()).path}${Platform.pathSeparator}play-$msgId.m4a');
    await file.writeAsBytes(data, flush: true);
    _file = file;
    playing = msgId;
    paused = false;
    position = Duration.zero;
    notifyListeners();
    await _player.play(DeviceFileSource(file.path));
  }

  Future<void> stop() async {
    try {
      await _player.stop();
    } catch (_) {}
    playing = null;
    paused = false;
    position = Duration.zero;
    final f = _file;
    _file = null;
    if (f != null && f.existsSync()) {
      try {
        f.deleteSync();
      } catch (_) {}
    }
    notifyListeners();
  }
}
