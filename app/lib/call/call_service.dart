import 'dart:async';
import 'dart:io';
import 'dart:math';

import 'package:flutter/material.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

import '../relay_service.dart';
import '../src/rust/api/client.dart' as core;
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
    _sub = relay.signals.listen(_onSignal);
    notifier.onAction = (a) {
      if (a == 'mute') toggleMute();
      if (a == 'hangup') phase == CallPhase.incoming ? decline() : hangup();
    };
  }

  final RelayService relay;
  final GlobalKey<NavigatorState> navigatorKey;
  final MediaSettings settings;
  final notifier = CallNotifier();
  late final StreamSubscription<IncomingSignal> _sub;

  static const _ringTimeout = Duration(seconds: 45);
  static const _reconnectDeadline = Duration(seconds: 25);

  CallPhase phase = CallPhase.idle;
  String peerName = '';
  bool muted = false;
  bool speaker = false;
  bool cameraOn = false;
  bool screenOn = false;
  bool peerVideo = false; // the peer sends camera or screen
  bool reconnecting = false;
  String? endReason;
  DateTime? connectedAt;
  CallStats? stats;
  bool showStats = false;

  /// Whether the call was started as a video call (UI hint for the incoming screen).
  bool videoCall = false;

  final localRenderer = RTCVideoRenderer();
  final remoteRenderer = RTCVideoRenderer();
  bool _renderersReady = false;

  String? _callId;
  bool _isCaller = false;
  String? _peerDevice; // the device we are actually talking to
  final _ringing = <String>{}; // outgoing: devices the offer went to
  Map<String, dynamic>? _offer; // incoming: the offer to answer
  RTCPeerConnection? _pc;
  RTCRtpSender? _audioSender;
  RTCRtpSender? _videoSender;
  MediaStreamTrack? _audioTrack;
  MediaStreamTrack? _cameraTrack;
  MediaStreamTrack? _screenTrack;
  MediaStream? _localView;
  MediaStream? _remoteView;
  bool _remoteSet = false;
  final _localIce = <RTCIceCandidate>[]; // outgoing: gathered before the answer
  final _remoteIce = <RTCIceCandidate>[]; // arrived before the remote description
  Timer? _ringTimer;
  Timer? _statsTimer;
  Timer? _restartTimer;
  Timer? _deadline;
  CallStatsSampler? _sampler;
  bool _screenShown = false;

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
      final sdp = tuneOpus(offer.sdp!);
      await _pc!.setLocalDescription(RTCSessionDescription(sdp, 'offer'));
      for (final d in contact.devices) {
        _ringing.add(d);
        relay.signal(d, {'type': 'offer', 'call': _callId, 'sdp': sdp, 'video': video});
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
        }
      }
      await _remoteDescriptionSet();
      final answer = await _pc!.createAnswer();
      final sdp = tuneOpus(answer.sdp!);
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

  Future<void> toggleSpeaker() async {
    speaker = !speaker;
    if (Platform.isAndroid) await Helper.setSpeakerphoneOn(speaker);
    notifyListeners();
  }

  Future<void> toggleCamera() async {
    if (_pc == null) return;
    if (cameraOn) {
      final t = _cameraTrack;
      _cameraTrack = null;
      cameraOn = false;
      if (!screenOn) await _videoSender?.replaceTrack(null);
      await t?.stop();
    } else {
      _cameraTrack = await _openCamera();
      if (_cameraTrack == null) return;
      cameraOn = true;
      if (!screenOn) await _videoSender?.replaceTrack(_cameraTrack);
      await notifier.update(camera: true);
    }
    await _refreshLocalView();
    _sendMediaState();
    notifyListeners();
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
      final stream = await navigator.mediaDevices.getDisplayMedia({
        'audio': false,
        'video': sourceId == null
            ? true
            : {
                'deviceId': {'exact': sourceId},
                'mandatory': {'frameRate': 15.0},
              },
      });
      _screenTrack = stream.getVideoTracks().first;
      _screenTrack!.onEnded = stopScreenShare;
      screenOn = true;
      await _videoSender?.replaceTrack(_screenTrack);
      await _refreshLocalView();
      _sendMediaState();
      notifyListeners();
    } catch (e) {
      _toast('не удалось показать экран: $e');
    }
  }

  Future<void> stopScreenShare() async {
    if (!screenOn) return;
    final t = _screenTrack;
    _screenTrack = null;
    screenOn = false;
    await _videoSender?.replaceTrack(cameraOn ? _cameraTrack : null);
    await t?.stop();
    if (Platform.isAndroid) await notifier.update(screen: false);
    await _refreshLocalView();
    _sendMediaState();
    notifyListeners();
  }

  Future<void> setAudioMode(AudioMode mode) async {
    settings.audioMode = mode;
    await _reopenMicrophone();
    notifyListeners();
  }

  Future<void> setMicrophone(String? deviceId) async {
    settings.micId = deviceId;
    await _reopenMicrophone();
  }

  Future<void> setCamera(String? deviceId) async {
    settings.cameraId = deviceId;
    if (!cameraOn) return;
    final old = _cameraTrack;
    _cameraTrack = await _openCamera();
    if (!screenOn) await _videoSender?.replaceTrack(_cameraTrack);
    await old?.stop();
    await _refreshLocalView();
  }

  Future<void> setOutput(String? deviceId) async {
    settings.outputId = deviceId;
    if (deviceId != null && isDesktop) await Helper.selectAudioOutput(deviceId);
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
      if (call == _callId) return; // duplicate
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
      phase = CallPhase.incoming;
      _ringTimer = Timer(_ringTimeout, () => _end('пропущенный вызов'));
      _showScreen();
      notifyListeners();
      return;
    }
    if (call != _callId) return;
    if (_peerDevice != null && s.fromDevice != _peerDevice && type != 'answer' && type != 'hangup') return;

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
        await _pc!.setRemoteDescription(RTCSessionDescription(d['sdp'] as String, 'offer'));
        final answer = await _pc!.createAnswer();
        final sdp = tuneOpus(answer.sdp!);
        await _pc!.setLocalDescription(RTCSessionDescription(sdp, 'answer'));
        relay.signal(_peerDevice!, {'type': 'reanswer', 'call': call, 'sdp': sdp});
      case 'reanswer':
        if (!_isCaller || _pc == null) return;
        await _pc!.setRemoteDescription(RTCSessionDescription(d['sdp'] as String, 'answer'));
      case 'ice':
        final c = RTCIceCandidate(d['candidate'] as String?, d['mid'] as String?, d['mline'] as int?);
        if (_remoteSet) {
          await _pc?.addCandidate(c);
        } else {
          _remoteIce.add(c);
        }
      case 'media':
        peerVideo = d['video'] == true || d['screen'] == true;
        notifyListeners();
      case 'hangup':
        final reason = d['reason'];
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

  void _sendMediaState() {
    if (_peerDevice == null) return;
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

  Future<MediaStreamTrack?> _openCamera() async {
    try {
      final id = settings.cameraId;
      final s = await navigator.mediaDevices.getUserMedia({
        'audio': false,
        'video': {
          if (id != null && isDesktop) 'deviceId': id else 'facingMode': 'user',
          'width': 1280,
          'height': 720,
          'frameRate': 30,
        },
      });
      return s.getVideoTracks().first;
    } catch (e) {
      _toast('камера недоступна: $e');
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
    final s = await navigator.mediaDevices.getUserMedia({
      'audio': AudioProfile.constraints(mode, isDesktop ? settings.micId : null),
      'video': false,
    });
    final t = s.getAudioTracks().first;
    t.enabled = !muted;
    return t;
  }

  Future<void> _reopenMicrophone() async {
    if (_pc == null) return;
    final old = _audioTrack;
    _audioTrack = await _openMicrophone();
    await _audioSender?.replaceTrack(_audioTrack);
    await old?.stop();
    await _applyAudioBitrate();
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

  Future<void> _refreshLocalView() async {
    final view = _localView ??= await createLocalMediaStream('local');
    for (final t in view.getVideoTracks()) {
      await view.removeTrack(t);
    }
    final t = screenOn ? _screenTrack : (cameraOn ? _cameraTrack : null);
    if (t != null) await view.addTrack(t);
    localRenderer.srcObject = t == null ? null : view;
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
      _cameraTrack = await _openCamera();
      cameraOn = _cameraTrack != null;
    }
    final outputId = settings.outputId;
    if (outputId != null && isDesktop) await Helper.selectAudioOutput(outputId);

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
      if (_cameraTrack != null) await _videoSender!.replaceTrack(_cameraTrack);
    }
    await _refreshLocalView();

    _pc!.onIceCandidate = (c) {
      if (c.candidate == null) return;
      if (_peerDevice == null) {
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
    speaker = camera;
    if (Platform.isAndroid) await Helper.setSpeakerphoneOn(speaker);
  }

  void _startStats() {
    _statsTimer?.cancel();
    _sampler = CallStatsSampler();
    _statsTimer = Timer.periodic(const Duration(seconds: 2), (_) async {
      final pc = _pc;
      if (pc == null) return;
      try {
        stats = await _sampler!.sample(pc);
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

  /// Network changed or broke. The caller restarts ICE; both sides give up after the deadline.
  void _connectionLost({required bool soft}) {
    if (phase != CallPhase.active && phase != CallPhase.connecting) return;
    if (!reconnecting) {
      reconnecting = true;
      _updateNotification();
      notifyListeners();
    }
    _deadline ??= Timer(_reconnectDeadline, () {
      if (_peerDevice != null) {
        relay.signal(_peerDevice!, {'type': 'hangup', 'call': _callId, 'reason': 'failed'});
      }
      _end('соединение потеряно');
    });
    if (!_isCaller) return;
    _restartTimer?.cancel();
    // "disconnected" often heals by itself; "failed" never does.
    _restartTimer = Timer(soft ? const Duration(seconds: 2) : Duration.zero, _restartIce);
  }

  Future<void> _restartIce() async {
    final pc = _pc;
    if (pc == null || _peerDevice == null) return;
    try {
      final offer = await pc.createOffer({'iceRestart': true});
      final sdp = tuneOpus(offer.sdp!);
      await pc.setLocalDescription(RTCSessionDescription(sdp, 'offer'));
      relay.signal(_peerDevice!, {'type': 'reoffer', 'call': _callId, 'sdp': sdp});
    } catch (_) {}
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
    _deadline?.cancel();
    _deadline = null;
    endReason = reason;
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
    _statsTimer?.cancel();
    _statsTimer = null;
    stats = null;
    final pc = _pc;
    final tracks = [_audioTrack, _cameraTrack, _screenTrack];
    _pc = null;
    _audioSender = null;
    _videoSender = null;
    _audioTrack = null;
    _cameraTrack = null;
    _screenTrack = null;
    localRenderer.srcObject = null;
    remoteRenderer.srcObject = null;
    _remoteView = null;
    for (final t in tracks) {
      await t?.stop();
    }
    await pc?.close();
    if (Platform.isAndroid) await Helper.setAndroidAudioConfiguration(AndroidAudioConfiguration.communication);
  }

  void _reset() {
    _ringTimer?.cancel();
    _callId = null;
    _peerDevice = null;
    _ringing.clear();
    _offer = null;
    _remoteSet = false;
    _localIce.clear();
    _remoteIce.clear();
    muted = false;
    cameraOn = false;
    screenOn = false;
    peerVideo = false;
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

  void _toast(String text) {
    final ctx = navigatorKey.currentContext;
    if (ctx == null) return;
    ScaffoldMessenger.maybeOf(ctx)?.showSnackBar(SnackBar(content: Text(text)));
  }

  void _showScreen() {
    if (_screenShown) return;
    final nav = navigatorKey.currentState;
    if (nav == null) return;
    _screenShown = true;
    nav.push(MaterialPageRoute(builder: (_) => CallScreen(calls: this))).then((_) => _screenShown = false);
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
