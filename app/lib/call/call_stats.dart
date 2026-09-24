import 'package:flutter_webrtc/flutter_webrtc.dart';

/// A snapshot of what the call is actually doing, derived from getStats deltas.
class CallStats {
  String route = '…'; // direct / via TURN, with candidate types
  double? rttMs;
  String audioCodec = '';
  String videoCodec = '';
  double audioOutKbps = 0, audioInKbps = 0, videoOutKbps = 0, videoInKbps = 0;
  double audioLossPct = 0, videoLossPct = 0;
  double videoInFps = 0, videoOutFps = 0;
  String videoInSize = '', videoOutSize = '';

  /// Encoder / decoder in use (e.g. libvpx, OpenH264, MediaCodec, NVENC): hardware or not.
  String videoEncoder = '', videoDecoder = '';

  /// Why the sender lowered quality now: cpu, bandwidth or none.
  String videoLimit = '';

  @override
  String toString() {
    String f(double v) => v.toStringAsFixed(0);
    final lines = [
      'маршрут: $route${rttMs == null ? '' : ', RTT ${f(rttMs!)} мс'}',
      'аудио $audioCodec: ↑${f(audioOutKbps)} ↓${f(audioInKbps)} кбит/с, потери ${audioLossPct.toStringAsFixed(1)}%',
      if (videoCodec.isNotEmpty)
        'видео $videoCodec: ↑${f(videoOutKbps)} ${f(videoOutFps)}fps $videoOutSize'
            ' ↓${f(videoInKbps)} ${f(videoInFps)}fps $videoInSize, потери ${videoLossPct.toStringAsFixed(1)}%',
      if (videoEncoder.isNotEmpty || videoDecoder.isNotEmpty)
        'кодер ${videoEncoder.isEmpty ? '—' : videoEncoder}, декодер ${videoDecoder.isEmpty ? '—' : videoDecoder}'
            '${videoLimit.isEmpty || videoLimit == 'none' ? '' : ', упор: $videoLimit'}',
    ];
    return lines.join('\n');
  }
}

class CallStatsSampler {
  final _prev = <String, Map<String, dynamic>>{};
  DateTime? _prevAt;

  Future<CallStats> sample(RTCPeerConnection pc) async {
    final reports = await pc.getStats();
    final now = DateTime.now();
    final dt = _prevAt == null ? 0.0 : now.difference(_prevAt!).inMilliseconds / 1000.0;
    final byId = {for (final r in reports) r.id: r};
    final s = CallStats();

    double rate(String id, Map<String, dynamic> v, String key) {
      final prev = _prev[id]?[key];
      final cur = v[key];
      if (dt <= 0 || prev is! num || cur is! num) return 0;
      return (cur - prev) / dt;
    }

    String codec(Map<String, dynamic> v) {
      final c = byId[v['codecId']];
      final mime = c?.values['mimeType']?.toString() ?? '';
      return mime.contains('/') ? mime.split('/').last : mime;
    }

    for (final r in reports) {
      final v = Map<String, dynamic>.from(r.values);
      final kind = v['kind'] ?? v['mediaType'];
      switch (r.type) {
        case 'candidate-pair':
          final active = v['nominated'] == true && v['state'] == 'succeeded';
          if (!active) break;
          final local = byId[v['localCandidateId']]?.values['candidateType'];
          final remote = byId[v['remoteCandidateId']]?.values['candidateType'];
          final relayed = local == 'relay' || remote == 'relay';
          s.route = '${relayed ? 'через TURN' : 'напрямую'} ($local↔$remote)';
          final rtt = v['currentRoundTripTime'];
          if (rtt is num) s.rttMs = rtt * 1000;
        case 'outbound-rtp':
          final kbps = rate(r.id, v, 'bytesSent') * 8 / 1000;
          if (kind == 'audio') {
            s.audioOutKbps = kbps;
            s.audioCodec = codec(v);
          } else if (kind == 'video') {
            s.videoOutKbps = kbps;
            s.videoOutFps = rate(r.id, v, 'framesEncoded');
            if (v['frameWidth'] != null) s.videoOutSize = '${v['frameWidth']}x${v['frameHeight']}';
            if (s.videoCodec.isEmpty) s.videoCodec = codec(v);
            s.videoEncoder = v['encoderImplementation']?.toString() ?? '';
            s.videoLimit = v['qualityLimitationReason']?.toString() ?? '';
          }
        case 'inbound-rtp':
          final kbps = rate(r.id, v, 'bytesReceived') * 8 / 1000;
          final lost = rate(r.id, v, 'packetsLost');
          final got = rate(r.id, v, 'packetsReceived');
          final loss = got + lost > 0 ? lost / (got + lost) * 100 : 0.0;
          if (kind == 'audio') {
            s.audioInKbps = kbps;
            s.audioLossPct = loss;
          } else if (kind == 'video') {
            s.videoInKbps = kbps;
            s.videoLossPct = loss;
            s.videoInFps = rate(r.id, v, 'framesDecoded');
            if (v['frameWidth'] != null) s.videoInSize = '${v['frameWidth']}x${v['frameHeight']}';
            final c = codec(v);
            if (c.isNotEmpty) s.videoCodec = c;
            s.videoDecoder = v['decoderImplementation']?.toString() ?? '';
          }
      }
      _prev[r.id] = v;
    }
    _prevAt = now;
    return s;
  }
}
