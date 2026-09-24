import 'dart:async';
import 'dart:io';
import 'dart:math';

import 'package:flutter/material.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

import '../relay_service.dart';
import '../src/rust/api/client.dart' as core;
import 'call_screen.dart';
import 'sdp.dart';

enum CallPhase { idle, outgoing, incoming, connecting, active, ended }

/// One call at a time. Signaling goes through E2E envelopes (see protocol/README.md §6.1);
/// media is WebRTC, direct when possible, via our TURN otherwise.
class CallService extends ChangeNotifier {
  CallService(this.relay, this.navigatorKey) {
    _sub = relay.signals.listen(_onSignal);
  }

  final RelayService relay;
  final GlobalKey<NavigatorState> navigatorKey;
  late final StreamSubscription<IncomingSignal> _sub;

  static const _ringTimeout = Duration(seconds: 45);

  CallPhase phase = CallPhase.idle;
  String peerName = '';
  bool video = false;
  bool muted = false;
  bool speaker = false;
  String? endReason;
  DateTime? connectedAt;

  final localRenderer = RTCVideoRenderer();
  final remoteRenderer = RTCVideoRenderer();
  bool _renderersReady = false;

  String? _callId;
  String? _peerDevice; // the device we are actually talking to
  final _ringing = <String>{}; // outgoing: devices the offer went to
  Map<String, dynamic>? _offer; // incoming: the offer to answer
  RTCPeerConnection? _pc;
  MediaStream? _local;
  bool _remoteSet = false;
  final _localIce = <RTCIceCandidate>[]; // outgoing: gathered before the answer
  final _remoteIce = <RTCIceCandidate>[]; // arrived before the remote description
  Timer? _ringTimer;
  bool _screenShown = false;

  bool get busy => phase != CallPhase.idle;

  static String _newCallId() {
    final r = Random.secure();
    return List.generate(16, (_) => r.nextInt(256).toRadixString(16).padLeft(2, '0')).join();
  }

  // ---- public actions

  Future<void> startCall(core.ContactInfo contact, {bool video = false}) async {
    if (busy || contact.devices.isEmpty) return;
    _reset();
    _callId = _newCallId();
    this.video = video;
    peerName = contact.name;
    phase = CallPhase.outgoing;
    _showScreen();
    notifyListeners();
    try {
      await _preparePeer();
      final offer = await _pc!.createOffer();
      final sdp = tuneOpus(offer.sdp!);
      await _pc!.setLocalDescription(RTCSessionDescription(sdp, 'offer'));
      for (final d in contact.devices) {
        _ringing.add(d);
        relay.signal(d, {'type': 'offer', 'call': _callId, 'sdp': sdp, 'video': video});
      }
      _ringTimer = Timer(_ringTimeout, () => hangup(reason: 'no_answer'));
    } catch (e) {
      _fail('не удалось начать звонок: $e');
    }
  }

  Future<void> accept() async {
    if (phase != CallPhase.incoming || _offer == null) return;
    _ringTimer?.cancel();
    phase = CallPhase.connecting;
    notifyListeners();
    try {
      await _preparePeer();
      await _pc!.setRemoteDescription(RTCSessionDescription(_offer!['sdp'] as String, 'offer'));
      await _remoteDescriptionSet();
      final answer = await _pc!.createAnswer();
      final sdp = tuneOpus(answer.sdp!);
      await _pc!.setLocalDescription(RTCSessionDescription(sdp, 'answer'));
      relay.signal(_peerDevice!, {'type': 'answer', 'call': _callId, 'sdp': sdp});
    } catch (e) {
      _fail('не удалось принять звонок: $e');
    }
  }

  void decline() {
    if (phase != CallPhase.incoming) return;
    relay.signal(_peerDevice!, {'type': 'hangup', 'call': _callId, 'reason': 'declined'});
    _end('отклонён');
  }

  /// Local hangup (button, timeout).
  void hangup({String reason = 'hangup'}) {
    if (!busy || phase == CallPhase.ended) return;
    final targets = _peerDevice != null ? {_peerDevice!} : _ringing;
    for (final d in targets) {
      relay.signal(d, {'type': 'hangup', 'call': _callId, 'reason': reason == 'no_answer' ? 'hangup' : reason});
    }
    _end(reason == 'no_answer' ? 'нет ответа' : 'завершён');
  }

  void toggleMute() {
    muted = !muted;
    for (final t in _local?.getAudioTracks() ?? <MediaStreamTrack>[]) {
      t.enabled = !muted;
    }
    notifyListeners();
  }

  Future<void> toggleSpeaker() async {
    speaker = !speaker;
    if (Platform.isAndroid) await Helper.setSpeakerphoneOn(speaker);
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
      _peerDevice = s.fromDevice;
      _offer = d;
      video = d['video'] == true;
      peerName = relay.contactOfDevice(s.fromDevice)?.name ?? 'неизвестный';
      phase = CallPhase.incoming;
      _ringTimer = Timer(_ringTimeout, () => _end('пропущенный вызов'));
      _showScreen();
      notifyListeners();
      return;
    }
    if (call != _callId) return;

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
      case 'ice':
        if (_peerDevice != null && s.fromDevice != _peerDevice) return;
        final c = RTCIceCandidate(d['candidate'] as String?, d['mid'] as String?, d['mline'] as int?);
        if (_remoteSet) {
          await _pc?.addCandidate(c);
        } else {
          _remoteIce.add(c);
        }
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

  Future<void> _remoteDescriptionSet() async {
    _remoteSet = true;
    for (final c in _remoteIce) {
      await _pc!.addCandidate(c);
    }
    _remoteIce.clear();
  }

  // ---- media

  Future<void> _preparePeer() async {
    if (!_renderersReady) {
      await localRenderer.initialize();
      await remoteRenderer.initialize();
      _renderersReady = true;
    }
    // On Android flutter_webrtc asks for microphone/camera permission itself here.
    try {
      _local = await navigator.mediaDevices.getUserMedia({
        'audio': {'echoCancellation': true, 'noiseSuppression': true, 'autoGainControl': true},
        'video': video ? {'facingMode': 'user', 'width': 1280, 'height': 720, 'frameRate': 30} : false,
      });
    } catch (_) {
      throw 'нет доступа к микрофону или камере';
    }
    if (video) localRenderer.srcObject = _local;

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
    for (final t in _local!.getTracks()) {
      await _pc!.addTrack(t, _local!);
    }
    _pc!.onIceCandidate = (c) {
      if (c.candidate == null) return;
      if (_peerDevice == null) {
        _localIce.add(c);
      } else {
        _sendIce(c);
      }
    };
    _pc!.onTrack = (e) {
      if (e.streams.isNotEmpty) remoteRenderer.srcObject = e.streams.first;
    };
    // Some platforms (flutter_webrtc on Windows) report only ICE state, so watch both.
    _pc!.onConnectionState = (s) {
      switch (s) {
        case RTCPeerConnectionState.RTCPeerConnectionStateConnected:
          _connected();
        case RTCPeerConnectionState.RTCPeerConnectionStateFailed:
          _connectionFailed();
        default:
      }
    };
    _pc!.onIceConnectionState = (s) {
      switch (s) {
        case RTCIceConnectionState.RTCIceConnectionStateConnected:
        case RTCIceConnectionState.RTCIceConnectionStateCompleted:
          _connected();
        case RTCIceConnectionState.RTCIceConnectionStateFailed:
          _connectionFailed();
        default:
      }
    };
    speaker = video;
    if (Platform.isAndroid) await Helper.setSpeakerphoneOn(speaker);
  }

  void _connected() {
    if (phase != CallPhase.connecting && phase != CallPhase.outgoing) return;
    phase = CallPhase.active;
    connectedAt = DateTime.now();
    notifyListeners();
  }

  void _connectionFailed() {
    if (phase == CallPhase.ended || phase == CallPhase.idle) return;
    if (_peerDevice != null) {
      relay.signal(_peerDevice!, {'type': 'hangup', 'call': _callId, 'reason': 'failed'});
    }
    _end('соединение не удалось');
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
    endReason = reason;
    phase = CallPhase.ended;
    notifyListeners();
    _teardown();
    Timer(const Duration(seconds: 2), () {
      if (phase != CallPhase.ended) return;
      phase = CallPhase.idle;
      notifyListeners();
    });
  }

  Future<void> _teardown() async {
    final pc = _pc;
    final local = _local;
    _pc = null;
    _local = null;
    localRenderer.srcObject = null;
    remoteRenderer.srcObject = null;
    for (final t in local?.getTracks() ?? <MediaStreamTrack>[]) {
      await t.stop();
    }
    await local?.dispose();
    await pc?.close();
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
    endReason = null;
    connectedAt = null;
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
    if (_renderersReady) {
      localRenderer.dispose();
      remoteRenderer.dispose();
    }
    super.dispose();
  }
}
