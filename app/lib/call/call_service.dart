import 'dart:async';
import 'dart:io';
import 'dart:math';

import 'package:flutter/material.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';
import 'package:window_manager/window_manager.dart';

import '../relay_service.dart';
import '../src/rust/api/client.dart' as core;
import '../desktop.dart' show Desktop;
import 'call_notifier.dart';
import 'call_screen.dart';
import 'call_stats.dart';
import 'media_settings.dart';
import 'sdp.dart';

enum CallPhase { idle, outgoing, incoming, connecting, active, ended }

/// One call at a time. Signaling goes through E2E envelopes (protocol/README.md §6.1);
/// media is WebRTC, direct when possible, via our TURN otherwise.
///
/// Both audio and video transceivers are negotiated in every call, so camera, screen
/// sharing, device switches and audio modes change tracks/parameters without renegotiation.
class CallService extends ChangeNotifier {
  CallService(this.relay, this.navigatorKey, this.settings) {
    // Debug builds log which video codecs this device can send.
    assert(() {
      availableVideoCodecs().then((c) => debugPrint('video codecs: $c'));
      return true;
    }());
    // Strictly one signal at a time: an ICE candidate must never be applied while the
    // (re)offer/answer it belongs to is still being set.
    _sub = relay.signals.listen((s) => _signalQueue = _signalQueue.then((_) => _onSignal(s)).catchError((_) {}));
    notifier.onAction = (a) {
      if (a == 'mute') toggleMute();
      if (a == 'hangup') phase == CallPhase.incoming ? decline() : hangup();
      // Answering from the notification never turns the camera on.
      if (a == 'accept') accept(video: false);
      if (a == 'decline') decline();
    };
  }

  final RelayService relay;
  final GlobalKey<NavigatorState> navigatorKey;
  final MediaSettings settings;
  final notifier = CallNotifier();
  late final StreamSubscription<IncomingSignal> _sub;

  static const _ringTimeout = Duration(seconds: 45);
  static const _reconnectDeadline = Duration(seconds: 40);

  CallPhase phase = CallPhase.idle;
  String peerName = '';
  bool muted = false;
  bool speaker = false;
  bool cameraOn = false;
  bool screenOn = false;
  bool peerVideo = false; // the peer sends camera or screen
  bool peerScreen = false; // ... and it is its screen
  int _idleVideoSamples = 0;
  int _busyVideoSamples = 0;
  DateTime _lastMediaAt = DateTime(0);
  bool reconnecting = false;
  Timer? _checkTimer;
  Timer? _relayKick;
  String? endReason;
  DateTime? connectedAt;
  CallStats? stats;
  bool showStats = false;

  @override
  void notifyListeners() {
    super.notifyListeners();
    final live = phase == CallPhase.outgoing || phase == CallPhase.connecting || phase == CallPhase.active;
    notifier.proximity(live && !speaker);
  }

  /// Whether the call was started as a video call (UI hint for the incoming screen).
  bool videoCall = false;

  final localRenderer = RTCVideoRenderer();
  final remoteRenderer = RTCVideoRenderer();
  bool _renderersReady = false;

  String? _callId;
  DateTime _lastHangupEcho = DateTime(0);

  /// Recently ended calls: the peer may still send signals for them (a lost hangup) even
  /// after we started another call.
  final _endedCalls = <String>[];
  bool _isCaller = false;
  String? _peerDevice; // the device we are actually talking to
  final _ringing = <String>{}; // outgoing: devices the offer went to
  Map<String, dynamic>? _offer; // incoming: the offer to answer
  RTCPeerConnection? _pc;
  RTCRtpSender? _audioSender;
  RTCRtpSender? _videoSender;
  MediaStreamTrack? _audioTrack;
  MediaStream? _cameraStream;
  MediaStream? _screenStream;
  MediaStream? _remoteView;
  bool _remoteSet = false;
  final _localIce = <RTCIceCandidate>[]; // outgoing: gathered before the answer

  /// During an ICE restart our new candidates wait until the reoffer/reanswer that carries
  /// their credentials has been sent: arriving first, the peer would test them against the
  /// old description and drop them.
  bool _holdLocalIce = false;

  /// Our candidates since the last ICE restart: re-sent with a repeated reoffer/reanswer,
  /// in case the first copies went into a relay socket that was already dead.
  final _restartCandidates = <RTCIceCandidate>[];

  void _resendRestartIce() {
    for (final c in _restartCandidates) {
      _sendIce(c);
    }
  }

  void _flushLocalIce() {
    _holdLocalIce = false;
    if (_peerDevice == null) return;
    for (final c in _localIce) {
      _sendIce(c);
    }
    _localIce.clear();
  }

  final _remoteIce = <RTCIceCandidate>[]; // arrived before the remote description
  Timer? _ringTimer;
  Timer? _statsTimer;
  Timer? _restartTimer;
  Future<void> _signalQueue = Future.value();
  String? _pendingReoffer; // caller: ICE-restart offer still waiting for its answer
  DateTime _lastRestart = DateTime(0);
  String? _lastReoffer; // callee: last reoffer applied, and our answer to it
  String? _lastReanswer;
  Timer? _deadline;
  CallStatsSampler? _sampler;
  bool _screenShown = false;

  /// The call screen is on top (otherwise the app shows a "back to the call" banner).
  bool get screenShown => _screenShown;

  /// User id of the other side, for "open the chat" from the call.
  String? peerUser;

  bool get busy => phase != CallPhase.idle;
  bool get isDesktop => Platform.isWindows || Platform.isLinux || Platform.isMacOS;

  static String _newCallId() {
    final r = Random.secure();
    return List.generate(16, (_) => r.nextInt(256).toRadixString(16).padLeft(2, '0')).join();
  }

  // ---- call control

  Future<void> startCall(core.ContactInfo contact, {bool video = false}) async {
    if (busy || contact.devices.isEmpty) return;
    _reset();
    peerUser = contact.userId;
    _callId = _newCallId();
    _isCaller = true;
    videoCall = video;
    peerName = contact.name;
    phase = CallPhase.outgoing;
    _showScreen();
    notifyListeners();
    try {
      await _preparePeer(camera: video);
      final offer = await _pc!.createOffer();
      final sdp = tuneSdp(offer.sdp!);
      await _pc!.setLocalDescription(RTCSessionDescription(sdp, 'offer'));
      for (final d in contact.devices) {
        _ringing.add(d);
        relay.signal(d, {'type': 'offer', 'call': _callId, 'sdp': sdp, 'video': video, 'vcodec': ?settings.videoCodec});
      }
      _ringTimer = Timer(_ringTimeout, () => hangup(reason: 'no_answer'));
      notifier.show(peerName, 'вызов…', muted: muted);
    } catch (e) {
      _fail('не удалось начать звонок: $e');
    }
  }

  Future<void> accept({bool? video}) async {
    if (phase != CallPhase.incoming || _offer == null) return;
    _ringTimer?.cancel();
    notifier.cancelIncoming();
    phase = CallPhase.connecting;
    notifyListeners();
    try {
      await _preparePeer(camera: video ?? videoCall);
      await _pc!.setRemoteDescription(RTCSessionDescription(_offer!['sdp'] as String, 'offer'));
      // Attach our tracks to the transceivers the offer created, both sendrecv.
      for (final t in await _pc!.getTransceivers()) {
        final kind = t.receiver.track?.kind;
        await t.setDirection(TransceiverDirection.SendRecv);
        if (kind == 'audio') {
          _audioSender = t.sender;
          await t.sender.replaceTrack(_audioTrack);
        } else if (kind == 'video') {
          _videoSender = t.sender;
          await t.sender.replaceTrack(_cameraTrack);
          await _preferCodec(t, _offer!['vcodec'] as String?);
        }
      }
      await _remoteDescriptionSet();
      final answer = await _pc!.createAnswer();
      final sdp = tuneSdp(answer.sdp!);
      await _pc!.setLocalDescription(RTCSessionDescription(sdp, 'answer'));
      relay.signal(_peerDevice!, {'type': 'answer', 'call': _callId, 'sdp': sdp});
      await _applyAudioBitrate();
      _sendMediaState();
      notifier.show(peerName, 'соединение…', muted: muted);
    } catch (e) {
      _fail('не удалось принять звонок: $e');
    }
  }

  void decline() {
    if (phase != CallPhase.incoming) return;
    relay.signal(_peerDevice!, {'type': 'hangup', 'call': _callId, 'reason': 'declined'});
    _end('отклонён');
  }

  /// Local hangup (button, notification, timeout).
  void hangup({String reason = 'hangup'}) {
    if (!busy || phase == CallPhase.ended) return;
    final targets = _peerDevice != null ? {_peerDevice!} : _ringing;
    _log('hangup sent ($reason, relay ${relay.state.name})');
    for (final d in targets) {
      relay.signal(d, {'type': 'hangup', 'call': _callId, 'reason': reason == 'no_answer' ? 'hangup' : reason});
    }
    _end(reason == 'no_answer' ? 'нет ответа' : 'завершён');
  }

  // ---- in-call controls

  void toggleMute() {
    muted = !muted;
    _audioTrack?.enabled = !muted;
    _updateNotification();
    notifyListeners();
  }

  // ---- audio output (Android): earpiece, loudspeaker, Bluetooth, wired headset

  /// Output ids from flutter_webrtc: `earpiece`, `speaker`, `bluetooth`, `wired-headset`.
  List<String> audioOutputs = const [];

  /// The output in use (as far as we routed it; the OS falls back by itself on unplug).
  String audioRoute = 'earpiece';

  static const _routeOrder = ['bluetooth', 'wired-headset', 'earpiece', 'speaker'];

  Future<List<String>> _outputs() async {
    try {
      final devices = await navigator.mediaDevices.enumerateDevices();
      final ids = {
        for (final d in devices)
          if (d.kind == 'audiooutput') d.deviceId,
      };
      return [
        for (final id in _routeOrder)
          if (ids.contains(id)) id,
      ];
    } catch (_) {
      return const ['earpiece', 'speaker'];
    }
  }

  /// Android picks outputs only in the voice mode: clean audio runs in media mode, where the
  /// OS routes by itself (Bluetooth or wired headset if connected, else the loudspeaker).
  bool get routable => settings.audioMode == AudioMode.voice;

  Future<void> setAudioRoute(String id) async {
    if (!Platform.isAndroid || !routable) return;
    audioRoute = id;
    speaker = id == 'speaker';
    // setSpeakerphoneOn also reorders the OS preference list (headsets first, then the chosen
    // built-in output), so a later plug/unplug lands somewhere sensible.
    await Helper.setSpeakerphoneOn(speaker);
    if (!speaker) await Helper.selectAudioOutput(id);
    notifyListeners();
  }

  /// Toggle kept for the loudspeaker button when no headset is around.
  Future<void> toggleSpeaker() => setAudioRoute(speaker ? 'earpiece' : 'speaker');

  /// At call start and whenever devices change: a new headset takes over, a vanished one
  /// gives way to the earpiece (or the loudspeaker in a video call).
  Future<void> _refreshAudioRoute({bool video = false, bool initial = false}) async {
    if (!Platform.isAndroid) return;
    final before = audioOutputs;
    audioOutputs = await _outputs();
    final headset = audioOutputs.where((o) => o == 'bluetooth' || o == 'wired-headset').firstOrNull;
    if (!routable) {
      audioRoute = headset ?? 'speaker';
      speaker = headset == null;
      notifyListeners();
      return;
    }
    final appeared = headset != null && !before.contains(headset);
    if (initial || appeared) {
      await setAudioRoute(headset ?? (video ? 'speaker' : 'earpiece'));
    } else if (!audioOutputs.contains(audioRoute)) {
      await setAudioRoute(headset ?? 'earpiece');
    } else {
      notifyListeners();
    }
  }

  Future<void> toggleCamera() async {
    if (_pc == null) return;
    if (cameraOn) {
      final s = _cameraStream;
      _cameraStream = null;
      cameraOn = false;
      if (!screenOn) await _videoSender?.replaceTrack(null);
      await _disposeStream(s);
    } else {
      _cameraStream = await _openCamera();
      if (_cameraStream == null) return;
      cameraOn = true;
      if (!screenOn) await _videoSender?.replaceTrack(_cameraTrack);
      if (!screenOn) await _applyVideoParams();
      await notifier.update(camera: true);
    }
    _refreshLocalView();
    _sendMediaState();
  }

  /// Front/back camera on phones.
  Future<void> switchCamera() async {
    final t = _cameraTrack;
    if (t != null) await Helper.switchCamera(t);
  }

  /// Desktop: [sourceId] from `desktopCapturer.getSources`. Android: null (system picker).
  Future<void> startScreenShare(String? sourceId) async {
    if (_pc == null || screenOn) return;
    try {
      if (Platform.isAndroid) {
        if (!await Helper.requestCapturePermission()) return;
        await notifier.update(screen: true); // MediaProjection needs the foreground service type
      }
      _screenStream = await navigator.mediaDevices.getDisplayMedia({
        'audio': false,
        'video': sourceId == null
            ? true
            : {
                'deviceId': {'exact': sourceId},
                'mandatory': {'frameRate': settings.screenFps.toDouble()},
              },
      });
    } catch (e) {
      if (Platform.isAndroid) await notifier.update(screen: false);
      _toast('не удалось показать экран: $e');
      return;
    }
    screenOn = true;
    _screenTrack?.onEnded = stopScreenShare;
    await _videoSender?.replaceTrack(_screenTrack);
    await _applyVideoParams();
    _refreshLocalView();
    _sendMediaState();
  }

  Future<void> stopScreenShare() async {
    if (!screenOn) return;
    final s = _screenStream;
    _screenStream = null;
    screenOn = false;
    await _videoSender?.replaceTrack(cameraOn ? _cameraTrack : null);
    await _applyVideoParams();
    await _disposeStream(s);
    if (Platform.isAndroid) await notifier.update(screen: false);
    _refreshLocalView();
    _sendMediaState();
  }

  Future<void> setAudioMode(AudioMode mode) async {
    settings.audioMode = mode;
    await _reopenMicrophone();
    if (Platform.isAndroid && _pc != null) {
      await notifier.audioMode(communication: mode == AudioMode.voice);
      await _refreshAudioRoute(video: cameraOn, initial: true);
    }
    notifyListeners();
  }

  Future<void> setMicrophone(String? deviceId) async {
    settings.micId = deviceId;
    await _reopenMicrophone();
  }

  Future<void> setCamera(String? deviceId) async {
    settings.cameraId = deviceId;
    if (!cameraOn) return;
    final old = _cameraStream;
    final fresh = await _openCamera();
    if (fresh == null) return;
    _cameraStream = fresh;
    if (!screenOn) await _videoSender?.replaceTrack(_cameraTrack);
    await _disposeStream(old);
    _refreshLocalView();
  }

  Future<void> setOutput(String? deviceId) async {
    settings.outputId = deviceId;
    await _applyOutput();
  }

  void toggleStats() {
    showStats = !showStats;
    notifyListeners();
  }

  // ---- signaling

  Future<void> _onSignal(IncomingSignal s) async {
    final d = s.data;
    final type = d['type'];
    final call = d['call'];
    if (call is! String) return;

    if (type == 'offer') {
      if (call == _callId || _endedCalls.contains(call)) return; // duplicate
      if (busy && reconnecting && s.fromDevice == _peerDevice) {
        // The device we are "reconnecting" with calls anew: for it the old call is over (its
        // hangup got lost). Close ours and take the new one instead of answering busy.
        _log('new offer from the peer while reconnecting: the old call is over');
        _end('завершён');
      }
      if (phase == CallPhase.ended) phase = CallPhase.idle; // a quick redial is not busy
      if (busy) {
        relay.signal(s.fromDevice, {'type': 'hangup', 'call': call, 'reason': 'busy'});
        return;
      }
      _reset();
      _callId = call;
      _isCaller = false;
      _peerDevice = s.fromDevice;
      _offer = d;
      videoCall = d['video'] == true;
      peerVideo = videoCall;
      peerName = relay.contactOfDevice(s.fromDevice)?.name ?? 'неизвестный';
      peerUser = s.fromUser;
      phase = CallPhase.incoming;
      _ringTimer = Timer(_ringTimeout, () => _end('пропущенный вызов'));
      _showScreen();
      notifier.showIncoming(peerName, video: videoCall);
      _bringToFront();
      notifyListeners();
      return;
    }
    final endedCall =
        _endedCalls.contains(call) && (call != _callId || phase == CallPhase.ended || phase == CallPhase.idle);
    if (!endedCall && call != _callId) return;
    final otherDevice = _peerDevice != null && s.fromDevice != _peerDevice;
    if (!endedCall && otherDevice && type != 'answer' && type != 'hangup') return;
    if (endedCall || phase == CallPhase.ended || phase == CallPhase.idle) {
      // The peer still talks about a call we ended: our hangup was lost (typically while its
      // relay connection was being replaced). Tell it again instead of letting it reconnect.
      const echoes = {'reoffer', 'reanswer', 'ice', 'media', 'check'};
      final now = DateTime.now();
      if (echoes.contains(type) && now.difference(_lastHangupEcho) > const Duration(seconds: 2)) {
        _lastHangupEcho = now;
        _log('signal for the ended call, hangup again');
        relay.signal(s.fromDevice, {'type': 'hangup', 'call': call, 'reason': 'hangup'});
      }
      return;
    }

    switch (type) {
      case 'answer':
        if (phase != CallPhase.outgoing) {
          // Another device of the callee answered too late.
          if (s.fromDevice != _peerDevice) {
            relay.signal(s.fromDevice, {'type': 'hangup', 'call': call, 'reason': 'answered_elsewhere'});
          }
          return;
        }
        _ringTimer?.cancel();
        _peerDevice = s.fromDevice;
        for (final other in _ringing.where((x) => x != s.fromDevice)) {
          relay.signal(other, {'type': 'hangup', 'call': call, 'reason': 'answered_elsewhere'});
        }
        phase = CallPhase.connecting;
        notifyListeners();
        await _pc!.setRemoteDescription(RTCSessionDescription(d['sdp'] as String, 'answer'));
        await _remoteDescriptionSet();
        for (final c in _localIce) {
          _sendIce(c);
        }
        _localIce.clear();
        await _applyAudioBitrate();
        _sendMediaState();
      case 'reoffer':
        if (_isCaller || _pc == null) return;
        final offerSdp = d['sdp'] as String;
        _log(offerSdp == _lastReoffer ? 'reoffer (repeat), answering again' : 'reoffer, answering');
        // A re-sent copy of an offer we already answered: just answer again.
        if (offerSdp != _lastReoffer) {
          _lastReoffer = offerSdp;
          _remoteSet = false; // buffer candidates until the new description is in
          _holdLocalIce = true;
          _restartCandidates.clear();
          await _pc!.setRemoteDescription(RTCSessionDescription(offerSdp, 'offer'));
          final answer = await _pc!.createAnswer();
          _lastReanswer = tuneSdp(answer.sdp!);
          await _pc!.setLocalDescription(RTCSessionDescription(_lastReanswer!, 'answer'));
          await _remoteDescriptionSet();
        }
        final repeat = !_holdLocalIce;
        relay.signal(_peerDevice!, {'type': 'reanswer', 'call': call, 'sdp': _lastReanswer});
        if (repeat) {
          _resendRestartIce(); // the caller may have missed them
        } else {
          _flushLocalIce();
        }
      case 'reanswer':
        // Only the answer to the offer we are waiting for; duplicates are ignored.
        if (!_isCaller || _pc == null || _pendingReoffer == null) return;
        _log('reanswer applied');
        _pendingReoffer = null;
        _remoteSet = false;
        await _pc!.setRemoteDescription(RTCSessionDescription(d['sdp'] as String, 'answer'));
        await _remoteDescriptionSet();
      case 'ice':
        final c = RTCIceCandidate(d['candidate'] as String?, d['mid'] as String?, d['mline'] as int?);
        _log('remote candidate ${_candKind(c.candidate)}${_remoteSet ? '' : ' (buffered)'}');
        if (_remoteSet) {
          await _pc?.addCandidate(c);
        } else {
          _remoteIce.add(c);
        }
      case 'media':
        _log('media received: video ${d['video']}, screen ${d['screen']}');
        _lastMediaAt = DateTime.now();
        peerScreen = d['screen'] == true;
        _setPeerVideo(d['video'] == true || d['screen'] == true);
        notifyListeners();
      case 'hangup':
        final reason = d['reason'];
        _log('hangup received ($reason)');
        if (phase == CallPhase.outgoing && _peerDevice == null) {
          _ringing.remove(s.fromDevice);
          if (_ringing.isEmpty) _end(reason == 'busy' ? 'занято' : 'отклонён');
        } else if (s.fromDevice == _peerDevice) {
          _end(switch (reason) {
            'answered_elsewhere' => 'отвечено на другом устройстве',
            'failed' => 'соединение не удалось',
            _ => 'собеседник завершил звонок',
          });
        }
    }
  }

  void _sendIce(RTCIceCandidate c) {
    relay.signal(_peerDevice!, {
      'type': 'ice',
      'call': _callId,
      'candidate': c.candidate,
      'mid': c.sdpMid,
      'mline': c.sdpMLineIndex,
    });
  }

  void _setPeerVideo(bool on) {
    if (on && !peerVideo) {
      // Re-attach the renderer: on Windows it sometimes misses the first frames of a track
      // that was empty until now (replaceTrack on the peer's side).
      remoteRenderer.srcObject = _remoteView;
    }
    peerVideo = on;
    _idleVideoSamples = 0;
    _busyVideoSamples = 0;
  }

  /// The `media` signal is the fast path, but it can be lost with a relay reconnect: frames
  /// actually arriving show the video anyway, and a long silence hides a stale last frame.
  void _videoFromStats(CallStats s) {
    // Decoded frames only: bandwidth probing sends padding on the video stream even when the
    // peer has no track, so the bitrate alone would show an empty window.
    final receiving = s.videoInFps >= 1;
    // A fresh media signal wins: after "off" the last frames still trickle in for a second or
    // two, and showing them again froze the picture until the silence timeout.
    final signalQuiet = DateTime.now().difference(_lastMediaAt) > const Duration(seconds: 6);
    if (receiving && !peerVideo) {
      if (++_busyVideoSamples >= 3 && signalQuiet) {
        _log('video frames keep arriving without a media signal, showing');
        _setPeerVideo(true);
      }
    } else if (!receiving && peerVideo && phase == CallPhase.active && !reconnecting) {
      if (++_idleVideoSamples >= 5) {
        _log('no video frames for a while, hiding');
        peerVideo = false;
        peerScreen = false;
        _idleVideoSamples = 0;
      }
    } else {
      _idleVideoSamples = 0;
      if (!receiving) _busyVideoSamples = 0;
    }
  }

  int _diagTick = 0;
  String _videoTrouble = '';

  /// Call log: video state every 10 s while any video flows, and at once when incoming video
  /// stalls (data arrives but no frames get decoded, or frames stop while the peer sends).
  void _videoDiag(CallStats s) {
    final involved = cameraOn || screenOn || peerVideo || s.videoInFps > 0 || s.videoOutFps > 0;
    if (!involved) return;
    String trouble = '';
    if (peerVideo && s.videoInKbps > 50 && s.videoInFramesReceived < 1) {
      trouble = 'incoming video: data but no complete frames';
    } else if (peerVideo && s.videoInFramesReceived >= 1 && s.videoInFps < 1) {
      trouble = 'incoming video: frames received but not decoded (decoder stalled)';
    } else if (peerVideo && s.videoInKbps < 5) {
      trouble = 'incoming video: peer should send, nothing arrives';
    } else if ((cameraOn || screenOn) && s.videoOutFps < 1) {
      trouble = 'outgoing video: track attached, encoder produces nothing';
    }
    if (trouble != _videoTrouble) {
      _log(trouble.isEmpty ? 'video ok again: ${s.diag()}' : '$trouble: ${s.diag()}');
      _videoTrouble = trouble;
    } else if (++_diagTick % 5 == 0) {
      _log(s.diag());
    }
  }

  void _sendMediaState() {
    if (_peerDevice == null) return;
    _log('media sent: video $cameraOn, screen $screenOn');
    relay.signal(_peerDevice!, {'type': 'media', 'call': _callId, 'video': cameraOn, 'screen': screenOn});
  }

  Future<void> _remoteDescriptionSet() async {
    _remoteSet = true;
    for (final c in _remoteIce) {
      await _pc!.addCandidate(c);
    }
    _remoteIce.clear();
  }

  // ---- media

  MediaStreamTrack? get _cameraTrack => _cameraStream?.getVideoTracks().firstOrNull;
  MediaStreamTrack? get _screenTrack => _screenStream?.getVideoTracks().firstOrNull;

  static Future<void> _disposeStream(MediaStream? s) async {
    if (s == null) return;
    for (final t in s.getTracks()) {
      await t.stop();
    }
    await s.dispose();
  }

  /// Desktop device ids go in `optional: [{sourceId}]` (flutter_webrtc on Windows reads
  /// `deviceId` in audio constraints as the *output* device). A saved device that is gone
  /// is forgotten and the default is used, so a stale setting never breaks a call.
  Future<MediaStream?> _openCamera() async {
    Map<String, dynamic> constraints(String? id) => {
      'audio': false,
      'video': {
        if (id != null && isDesktop)
          'optional': [
            {'sourceId': id},
          ]
        else
          'facingMode': 'user',
        'width': 1280,
        'height': 720,
        'frameRate': 30,
      },
    };
    final id = settings.cameraId;
    try {
      return await navigator.mediaDevices.getUserMedia(constraints(id));
    } catch (_) {
      if (id != null && isDesktop) {
        settings.cameraId = null;
        try {
          return await navigator.mediaDevices.getUserMedia(constraints(null));
        } catch (_) {}
      }
      _toast('камера недоступна');
      return null;
    }
  }

  Future<MediaStreamTrack> _openMicrophone() async {
    final mode = settings.audioMode;
    if (Platform.isAndroid) {
      // Voice: VOICE_COMMUNICATION source with the phone's hardware AEC/NS.
      // Clean: plain media source, no system processing.
      await Helper.setAndroidAudioConfiguration(
        mode == AudioMode.voice ? AndroidAudioConfiguration.communication : AndroidAudioConfiguration.media,
      );
    }
    final id = isDesktop ? settings.micId : null;
    MediaStream s;
    try {
      s = await navigator.mediaDevices.getUserMedia({'audio': AudioProfile.constraints(mode, id), 'video': false});
    } catch (_) {
      if (id == null) rethrow;
      settings.micId = null;
      s = await navigator.mediaDevices.getUserMedia({'audio': AudioProfile.constraints(mode, null), 'video': false});
    }
    final t = s.getAudioTracks().first;
    t.enabled = !muted;
    return t;
  }

  Future<void> _applyOutput() async {
    final id = settings.outputId;
    if (id == null || !isDesktop) return;
    try {
      await Helper.selectAudioOutput(id);
    } catch (_) {
      settings.outputId = null;
    }
  }

  Future<void> _reopenMicrophone() async {
    if (_pc == null) return;
    final old = _audioTrack;
    _audioTrack = await _openMicrophone();
    await _audioSender?.replaceTrack(_audioTrack);
    await old?.stop();
    await _applyAudioBitrate();
  }

  /// Screen: smooth motion first (games), high bitrate. Camera: WebRTC's balanced default.
  Future<void> _applyVideoParams() async {
    final s = _videoSender;
    if (s == null) return;
    try {
      final p = s.parameters;
      final enc = p.encodings;
      if (enc == null || enc.isEmpty) return;
      final fps = settings.screenFps;
      if (screenOn) {
        enc.first.maxBitrate = fps >= 60 ? 12000000 : 8000000;
        enc.first.maxFramerate = fps;
        enc.first.scaleResolutionDownBy = 1.0;
        p.degradationPreference = RTCDegradationPreference.MAINTAIN_FRAMERATE;
      } else {
        enc.first.maxBitrate = 2500000;
        enc.first.maxFramerate = 30;
        p.degradationPreference = RTCDegradationPreference.BALANCED;
      }
      await s.setParameters(p);
    } catch (_) {
      // Not supported on this platform: WebRTC defaults stay.
    }
  }

  /// Puts [mime] (e.g. "H264") first in the transceiver's codec list, if this device has it.
  /// The rest stay as fallbacks, so a peer without it still gets video.
  static Future<void> _preferCodec(RTCRtpTransceiver t, String? mime) async {
    if (mime == null) return;
    try {
      final caps = await getRtpSenderCapabilities('video');
      final codecs = caps.codecs ?? [];
      final want = [
        for (final c in codecs)
          if (c.mimeType.toLowerCase() == 'video/${mime.toLowerCase()}') c,
      ];
      if (want.isEmpty) return;
      await t.setCodecPreferences([...want, ...codecs.where((c) => !want.contains(c))]);
    } catch (_) {}
  }

  /// Video codecs this device can send (mime subtypes), for the settings.
  static Future<List<String>> availableVideoCodecs() async {
    try {
      final caps = await getRtpSenderCapabilities('video');
      final names = <String>{};
      for (final c in caps.codecs ?? <RTCRtpCodecCapability>[]) {
        final n = c.mimeType.split('/').last;
        if (!const {'rtx', 'red', 'ulpfec', 'flexfec-03'}.contains(n.toLowerCase())) names.add(n);
      }
      return names.toList();
    } catch (_) {
      return const [];
    }
  }

  Future<void> _applyAudioBitrate() async {
    final s = _audioSender;
    if (s == null) return;
    final p = s.parameters;
    final enc = p.encodings;
    if (enc == null || enc.isEmpty) return;
    enc.first.maxBitrate = AudioProfile.bitrate(settings.audioMode);
    await s.setParameters(p);
  }

  /// The preview shows the capture stream itself (no track juggling on native streams).
  void _refreshLocalView() {
    localRenderer.srcObject = screenOn ? _screenStream : (cameraOn ? _cameraStream : null);
    notifyListeners();
  }

  Future<void> _preparePeer({required bool camera}) async {
    if (!_renderersReady) {
      await localRenderer.initialize();
      await remoteRenderer.initialize();
      _renderersReady = true;
    }
    // On Android flutter_webrtc asks for microphone/camera permission itself here.
    try {
      _audioTrack = await _openMicrophone();
    } catch (_) {
      throw 'нет доступа к микрофону';
    }
    if (camera) {
      _cameraStream = await _openCamera();
      cameraOn = _cameraStream != null;
    }
    await _applyOutput();

    final ice = <Map<String, dynamic>>[
      for (final s in relay.stunServers) {'urls': s},
    ];
    final turn = await relay.turnCreds();
    if (turn != null) {
      ice.add({'urls': turn.urls, 'username': turn.username, 'credential': turn.credential});
    }
    _pc = await createPeerConnection({
      'iceServers': ice,
      'iceTransportPolicy': 'all',
      'bundlePolicy': 'max-bundle',
      'sdpSemantics': 'unified-plan',
    });
    if (_isCaller) {
      final stream = await createLocalMediaStream('botato');
      final a = await _pc!.addTransceiver(
        track: _audioTrack!,
        kind: RTCRtpMediaType.RTCRtpMediaTypeAudio,
        init: RTCRtpTransceiverInit(direction: TransceiverDirection.SendRecv, streams: [stream]),
      );
      // The video transceiver exists even in voice calls; camera/screen are attached later.
      final v = await _pc!.addTransceiver(
        kind: RTCRtpMediaType.RTCRtpMediaTypeVideo,
        init: RTCRtpTransceiverInit(direction: TransceiverDirection.SendRecv, streams: [stream]),
      );
      _audioSender = a.sender;
      _videoSender = v.sender;
      await _preferCodec(v, settings.videoCodec);
      if (_cameraTrack != null) await _videoSender!.replaceTrack(_cameraTrack);
    }
    _refreshLocalView();

    _pc!.onIceCandidate = (c) {
      if (c.candidate == null) return;
      _restartCandidates.add(c);
      _log('local candidate ${_candKind(c.candidate)}${_peerDevice == null || _holdLocalIce ? ' (held)' : ''}');
      if (_peerDevice == null || _holdLocalIce) {
        _localIce.add(c);
      } else {
        _sendIce(c);
      }
    };
    _pc!.onTrack = (e) async {
      if (e.track.kind != 'video') return;
      if (e.streams.isNotEmpty) {
        _remoteView = e.streams.first;
      } else {
        _remoteView ??= await createLocalMediaStream('remote');
        await _remoteView!.addTrack(e.track);
      }
      remoteRenderer.srcObject = _remoteView;
      notifyListeners();
    };
    // Some platforms (flutter_webrtc on Windows) report only ICE state, so watch both.
    _pc!.onConnectionState = (s) {
      _log('pc ${s.name.replaceFirst('RTCPeerConnectionState', '')}');
      switch (s) {
        case RTCPeerConnectionState.RTCPeerConnectionStateConnected:
          _connected();
        case RTCPeerConnectionState.RTCPeerConnectionStateDisconnected:
          _connectionLost(soft: true);
        case RTCPeerConnectionState.RTCPeerConnectionStateFailed:
          _connectionLost(soft: false);
        default:
      }
    };
    _pc!.onIceConnectionState = (s) {
      _log('ice ${s.name.replaceFirst('RTCIceConnectionState', '')}');
      switch (s) {
        case RTCIceConnectionState.RTCIceConnectionStateConnected:
        case RTCIceConnectionState.RTCIceConnectionStateCompleted:
          _connected();
        case RTCIceConnectionState.RTCIceConnectionStateDisconnected:
          _connectionLost(soft: true);
        case RTCIceConnectionState.RTCIceConnectionStateFailed:
          _connectionLost(soft: false);
        default:
      }
    };
    _startStats();
    navigator.mediaDevices.ondevicechange = (_) => _refreshAudioRoute(video: cameraOn);
    await _refreshAudioRoute(video: camera, initial: true);
  }

  void _startStats() {
    _statsTimer?.cancel();
    _sampler = CallStatsSampler();
    _statsTimer = Timer.periodic(const Duration(seconds: 2), (_) async {
      final pc = _pc;
      if (pc == null) return;
      try {
        stats = await _sampler!.sample(pc);
        _videoFromStats(stats!);
        _videoDiag(stats!);
        notifyListeners();
      } catch (_) {}
    });
  }

  // ---- connection state and ICE restart

  void _connected() {
    _restartTimer?.cancel();
    _deadline?.cancel();
    _deadline = null;
    if (reconnecting) {
      final lost = _lostAt;
      _log('media back${lost == null ? '' : ' after ${DateTime.now().difference(lost).inMilliseconds} ms'}');
      reconnecting = false;
      _updateNotification();
      notifyListeners();
    }
    if (phase != CallPhase.connecting && phase != CallPhase.outgoing) return;
    phase = CallPhase.active;
    connectedAt = DateTime.now();
    _updateNotification();
    notifyListeners();
  }

  /// Call diagnostics for `flutter logs` / logcat: events only, never SDP, keys or addresses.
  void _log(String event) {
    final t = DateTime.now();
    final ts =
        '${t.minute.toString().padLeft(2, '0')}:${t.second.toString().padLeft(2, '0')}.'
        '${(t.millisecond ~/ 100)}';
    debugPrint('[call $ts] $event');
  }

  DateTime? _lostAt;

  /// "relay udp" from a candidate line; never the address.
  static String _candKind(String? line) {
    if (line == null) return '?';
    final typ = RegExp(r' typ (\w+)').firstMatch(line)?.group(1) ?? '?';
    final proto = RegExp(r'^candidate:\S+ \d+ (\w+)', caseSensitive: false).firstMatch(line)?.group(1) ?? '?';
    return '$typ ${proto.toLowerCase()}';
  }

  /// What ICE had to work with when it failed: candidate types on both sides and pair states.
  Future<void> _logIceSummary() async {
    final pc = _pc;
    if (pc == null) return;
    try {
      final local = <String, int>{}, remote = <String, int>{}, pairs = <String, int>{};
      for (final r in await pc.getStats()) {
        final v = r.values;
        String k(Map m) => '${m['candidateType']}/${m['protocol']}';
        switch (r.type) {
          case 'local-candidate':
            local.update(k(v), (n) => n + 1, ifAbsent: () => 1);
          case 'remote-candidate':
            remote.update(k(v), (n) => n + 1, ifAbsent: () => 1);
          case 'candidate-pair':
            pairs.update('${v['state']}', (n) => n + 1, ifAbsent: () => 1);
        }
      }
      _log('ICE summary: local $local, remote $remote, pairs $pairs');
    } catch (e) {
      _log('ICE summary failed: $e');
    }
  }

  /// Network changed or broke. The caller restarts ICE; both sides give up after the deadline.
  void _connectionLost({required bool soft}) {
    if (phase != CallPhase.active && phase != CallPhase.connecting) return;
    _log('connection lost (${soft ? 'disconnected' : 'failed'}), ${_isCaller ? 'caller' : 'callee'}');
    if (!soft) _logIceSummary();
    if (!reconnecting) {
      _lostAt = DateTime.now();
      reconnecting = true;
      // The relay socket probably died with the old network too; don't wait to notice. But
      // not at once on a mere "disconnected": the peer may just have hung up, and its hangup,
      // arriving right now, would be lost with the replaced connection.
      _relayKick?.cancel();
      _relayKick = Timer(soft ? const Duration(milliseconds: 1500) : Duration.zero, () {
        if (reconnecting) relay.reconnect();
      });
      _updateNotification();
      notifyListeners();
    }
    _deadline ??= Timer(_reconnectDeadline, () {
      _log('gave up after ${_reconnectDeadline.inSeconds} s');
      if (_peerDevice != null) {
        relay.signal(_peerDevice!, {'type': 'hangup', 'call': _callId, 'reason': 'failed'});
      }
      _end('соединение потеряно');
    });
    if (!_isCaller) {
      // The callee only waits for a reoffer; ask now and then whether the caller is still
      // there (an ended caller answers with hangup).
      _checkTimer ??= Timer.periodic(const Duration(seconds: 5), (t) {
        if (!reconnecting || _peerDevice == null) {
          t.cancel();
          _checkTimer = null;
          return;
        }
        relay.signal(_peerDevice!, {'type': 'check', 'call': _callId});
      });
      return;
    }
    if (_restartTimer?.isActive == true) return;
    // "disconnected" often heals by itself; "failed" never does.
    _restartTimer = Timer(soft ? const Duration(seconds: 1) : Duration.zero, () {
      _restartIce();
      _restartTimer = Timer.periodic(const Duration(seconds: 5), (t) {
        if (!reconnecting) return t.cancel();
        final pending = _pendingReoffer;
        if (pending != null) {
          // No answer yet (the relay may have been reconnecting): re-send the same offer,
          // don't start over; restarting again would abort candidate gathering.
          _log('no reanswer yet, re-sending the same reoffer (relay ${relay.state.name})');
          relay.signal(_peerDevice!, {'type': 'reoffer', 'call': _callId, 'sdp': pending});
          _resendRestartIce();
        } else if (DateTime.now().difference(_lastRestart) > const Duration(seconds: 15)) {
          _log('answered but still no media, fresh ICE restart');
          _restartIce(); // answered, but still no media: try a fresh restart
        }
      });
    });
  }

  Future<void> _restartIce() async {
    final pc = _pc;
    if (pc == null || _peerDevice == null) return;
    try {
      _lastRestart = DateTime.now();
      // flutter_webrtc (Android, Windows) reads offer options only from `mandatory`/`optional`:
      // a top-level `iceRestart` was silently ignored, so no new credentials or candidates.
      await pc.restartIce();
      final offer = await pc.createOffer({
        'mandatory': {'IceRestart': true},
      });
      final sdp = tuneSdp(offer.sdp!);
      _holdLocalIce = true;
      _restartCandidates.clear();
      _remoteSet = false; // the callee's new candidates wait for its reanswer
      await pc.setLocalDescription(RTCSessionDescription(sdp, 'offer'));
      _pendingReoffer = sdp;
      _log('ICE restart: reoffer sent (relay ${relay.state.name})');
      relay.signal(_peerDevice!, {'type': 'reoffer', 'call': _callId, 'sdp': sdp});
      _flushLocalIce();
    } catch (e) {
      _log('ICE restart failed: $e');
    }
  }

  // ---- lifecycle

  void _fail(String message) {
    for (final d in _peerDevice != null ? {_peerDevice!} : _ringing) {
      relay.signal(d, {'type': 'hangup', 'call': _callId, 'reason': 'failed'});
    }
    _end(message);
  }

  void _end(String reason) {
    if (phase == CallPhase.ended || phase == CallPhase.idle) return;
    _ringTimer?.cancel();
    _restartTimer?.cancel();
    _checkTimer?.cancel();
    _checkTimer = null;
    _relayKick?.cancel();
    _deadline?.cancel();
    _deadline = null;
    endReason = reason;
    if (_callId case final id?) {
      _endedCalls.add(id);
      if (_endedCalls.length > 8) _endedCalls.removeAt(0);
    }
    phase = CallPhase.ended;
    notifier.hide();
    notifyListeners();
    _teardown();
    Timer(const Duration(seconds: 2), () {
      if (phase != CallPhase.ended) return;
      phase = CallPhase.idle;
      notifyListeners();
    });
  }

  Future<void> _teardown() async {
    navigator.mediaDevices.ondevicechange = null;
    _statsTimer?.cancel();
    _statsTimer = null;
    stats = null;
    final pc = _pc;
    final audio = _audioTrack;
    final streams = [_cameraStream, _screenStream];
    _pc = null;
    _audioSender = null;
    _videoSender = null;
    _audioTrack = null;
    _cameraStream = null;
    _screenStream = null;
    localRenderer.srcObject = null;
    remoteRenderer.srcObject = null;
    _remoteView = null;
    await audio?.stop();
    for (final s in streams) {
      await _disposeStream(s);
    }
    await pc?.close();
    if (Platform.isAndroid) await Helper.setAndroidAudioConfiguration(AndroidAudioConfiguration.communication);
  }

  void _reset() {
    _ringTimer?.cancel();
    _pendingReoffer = null;
    _lastReoffer = null;
    _lastReanswer = null;
    _callId = null;
    _peerDevice = null;
    _ringing.clear();
    _offer = null;
    _remoteSet = false;
    _holdLocalIce = false;
    _restartCandidates.clear();
    _localIce.clear();
    _remoteIce.clear();
    muted = false;
    cameraOn = false;
    screenOn = false;
    peerVideo = false;
    peerScreen = false;
    _idleVideoSamples = 0;
    _busyVideoSamples = 0;
    _lastMediaAt = DateTime(0);
    _videoTrouble = '';
    _diagTick = 0;
    reconnecting = false;
    endReason = null;
    connectedAt = null;
  }

  void _updateNotification() {
    if (phase == CallPhase.idle || phase == CallPhase.ended) return;
    final status = reconnecting
        ? 'переподключение…'
        : switch (phase) {
            CallPhase.active => 'идёт звонок',
            CallPhase.outgoing => 'вызов…',
            _ => 'соединение…',
          };
    notifier.show(peerName, status, muted: muted);
  }

  /// Desktop: an incoming call raises and focuses the window (Windows only lets a
  /// background app take focus when it is briefly always-on-top).
  Future<void> _bringToFront() async {
    if (!isDesktop || Desktop.instance.dnd) return;
    try {
      if (await windowManager.isMinimized()) await windowManager.restore();
      await windowManager.show();
      await windowManager.setAlwaysOnTop(true);
      await windowManager.focus();
      await Future<void>.delayed(const Duration(seconds: 2));
      await windowManager.setAlwaysOnTop(false);
    } catch (_) {}
  }

  void _toast(String text) {
    final ctx = navigatorKey.currentContext;
    if (ctx == null) return;
    ScaffoldMessenger.maybeOf(ctx)?.showSnackBar(SnackBar(content: Text(text)));
  }

  /// Shows the call screen for a call that began before the UI existed.
  void showScreenIfNeeded() {
    if (busy && phase != CallPhase.ended) _showScreen();
  }

  /// Back to the call screen (from the banner).
  void showScreen() => _showScreen();

  void _showScreen() {
    if (_screenShown) return;
    final nav = navigatorKey.currentState;
    if (nav == null) return;
    _screenShown = true;
    notifyListeners();
    nav.push(MaterialPageRoute(builder: (_) => CallScreen(calls: this))).then((_) {
      _screenShown = false;
      notifyListeners();
    });
  }

  @override
  void dispose() {
    _sub.cancel();
    _teardown();
    notifier.hide();
    if (_renderersReady) {
      localRenderer.dispose();
      remoteRenderer.dispose();
    }
    super.dispose();
  }
}
