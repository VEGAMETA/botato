import 'dart:io';

import 'package:audioplayers/audioplayers.dart';
import 'package:flutter/services.dart';

import 'call/call_service.dart';
import 'desktop.dart';

/// The app's own sounds (tools/gen-sounds.py). Android plays in-call tones natively as voice
/// communication, so they follow the call's route; the incoming ring and the message sound
/// there belong to the notification channels. Desktop plays everything here.
class Sounds {
  Sounds._();

  static const _channel = MethodChannel('botato/call');
  static AudioPlayer? _loopPlayer;
  static String? _loopName;

  static Future<void> play(String name) async {
    if (Platform.isAndroid) return _invoke('sound', {'name': name, 'loop': false});
    if (!isDesktop) return;
    final p = AudioPlayer();
    p.onPlayerComplete.first.then((_) => p.dispose());
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
