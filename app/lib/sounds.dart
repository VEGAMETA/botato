import 'dart:async';
import 'dart:ffi';
import 'dart:io';

import 'package:audioplayers/audioplayers.dart';
import 'package:ffi/ffi.dart';
import 'package:flutter/services.dart';

import 'call/call_service.dart';
import 'desktop.dart';

/// The app's own sounds (tools/gen-sounds.py). Android plays in-call tones natively as voice
/// communication, so they follow the call's route; the incoming ring and the message sound
/// there belong to the notification channels. Windows plays them with the system PlaySound
/// (audioplayers crashed the app: it fires its events from a native thread). Linux and macOS
/// use audioplayers.
class Sounds {
  Sounds._();

  static const _channel = MethodChannel('botato/call');
  static AudioPlayer? _loopPlayer;
  static String? _loopName;

  static Future<void> play(String name) async {
    if (Platform.isAndroid) return _invoke('sound', {'name': name, 'loop': false});
    if (Platform.isWindows) return _WinSound.play(name, resumeLoop: _loopName);
    if (!isDesktop) return;
    final p = AudioPlayer();
    // Disposed a moment after completion, not inside the completion event itself.
    p.onPlayerComplete.first.then((_) => Future.delayed(const Duration(seconds: 1), p.dispose));
    await _guard(() => p.play(AssetSource('sounds/$name.wav'), volume: 0.8));
  }

  // State changes happen before any await: CallService notifies several times in a row, and
  // a second loop() must not start a second player that nobody will stop.
  static Future<void> loop(String name) async {
    if (_loopName == name) return;
    final old = _loopPlayer;
    _loopPlayer = null;
    _loopName = name;
    if (Platform.isAndroid) return _invoke('sound', {'name': name, 'loop': true});
    if (Platform.isWindows) return _WinSound.loop(name);
    if (!isDesktop) return;
    final p = _loopPlayer = AudioPlayer();
    if (old != null) await _guard(() => old.dispose());
    await _guard(() async {
      await p.setReleaseMode(ReleaseMode.loop);
      if (_loopPlayer != p) return; // stopped or replaced meanwhile
      await p.play(AssetSource('sounds/$name.wav'), volume: 0.8);
    });
    // Stopped while starting: play() may have begun after dispose was requested.
    if (_loopPlayer != p) await _guard(() => p.dispose());
  }

  static Future<void> stopLoop() async {
    if (_loopName == null) return;
    _loopName = null;
    final p = _loopPlayer;
    _loopPlayer = null;
    if (Platform.isAndroid) return _invoke('stopSound', null);
    if (Platform.isWindows) return _WinSound.stop();
    if (p != null) await _guard(() => p.dispose());
  }

  static Future<void> _invoke(String method, Object? args) async {
    try {
      await _channel.invokeMethod(method, args);
    } on MissingPluginException {
      // tests
    } on PlatformException {
      // a missing sound must never break a call
    }
  }

  static Future<void> _guard(Future<void> Function() f) async {
    try {
      await f();
    } catch (_) {}
  }
}

/// winmm PlaySoundW: asynchronous, no callbacks. It plays one sound per process, so a
/// one-shot interrupts a loop; the loop is started again when the one-shot is over.
class _WinSound {
  static const _async = 0x1, _noDefault = 0x2, _loopFlag = 0x8, _fileName = 0x20000;
  static final _playSound = DynamicLibrary.open('winmm.dll')
      .lookupFunction<Int32 Function(Pointer<Utf16>, IntPtr, Uint32), int Function(Pointer<Utf16>, int, int)>(
        'PlaySoundW',
      );
  static Pointer<Utf16> _path = nullptr; // kept alive while the sound may still read it
  static String? _loop;
  static Timer? _resume;

  static File _file(String name) {
    final dir = File(Platform.resolvedExecutable).parent.path;
    return File([dir, 'data', 'flutter_assets', 'assets', 'sounds', '$name.wav'].join(Platform.pathSeparator));
  }

  static void _start(String name, {required bool loop}) {
    final file = _file(name);
    if (!file.existsSync()) return;
    final old = _path;
    _path = file.path.toNativeUtf16();
    try {
      _playSound(_path, 0, _fileName | _async | _noDefault | (loop ? _loopFlag : 0));
    } catch (_) {}
    if (old != nullptr) calloc.free(old);
  }

  static void play(String name, {String? resumeLoop}) {
    _start(name, loop: false);
    _resume?.cancel();
    if (_loop == null) return;
    // 16-bit mono 24 kHz WAV with a 44-byte header (gen-sounds.py).
    final ms = ((_file(name).lengthSync() - 44) / 48).round();
    _resume = Timer(Duration(milliseconds: ms + 50), () {
      if (_loop case final l?) _start(l, loop: true);
    });
  }

  static void loop(String name) {
    _resume?.cancel();
    _loop = name;
    _start(name, loop: true);
  }

  static void stop() {
    _resume?.cancel();
    _loop = null;
    try {
      _playSound(nullptr, 0, 0);
    } catch (_) {}
  }
}

/// Call tones derived from the call state: ringback while our call rings, the ring for an
/// incoming call (desktop; Android rings from the notification), a soft blip while
/// reconnecting, a falling tone when a call we were in ends, a chime when screen sharing starts.
class CallSounds {
  CallSounds(this.calls) {
    calls.addListener(_update);
  }

  final CallService calls;
  CallPhase _phase = CallPhase.idle;
  bool _screen = false;
  bool _peerScreen = false;

  void _update() {
    final c = calls;
    final quiet = isDesktop && Desktop.instance.dnd;
    if (c.phase == CallPhase.ended &&
        (_phase == CallPhase.outgoing || _phase == CallPhase.connecting || _phase == CallPhase.active)) {
      Sounds.play('hangup');
    }
    if ((c.screenOn && !_screen) || (c.peerScreen && !_peerScreen)) Sounds.play('screen');
    final live = c.phase == CallPhase.active || c.phase == CallPhase.connecting;
    final want = switch (c.phase) {
      CallPhase.outgoing => 'ringback',
      CallPhase.incoming when isDesktop && !quiet => 'ring',
      _ when live && c.reconnecting => 'reconnect',
      _ => null,
    };
    want == null ? Sounds.stopLoop() : Sounds.loop(want);
    _phase = c.phase;
    _screen = c.screenOn;
    _peerScreen = c.peerScreen;
  }
}
