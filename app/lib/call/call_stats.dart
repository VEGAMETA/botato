import 'package:flutter_webrtc/flutter_webrtc.dart';

import '../l10n.dart';

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

  // Diagnostics for the call log (frozen or empty video).
  String videoOutCodec = '', videoInCodec = '';
  double videoInFramesReceived = 0; // per second, before decoding
  num videoInDropped = 0, videoInFreezes = 0, videoInKeyFrames = 0;
  num videoInPliSent = 0, videoInNackSent = 0;
  num videoOutKeyFrames = 0, videoOutPliReceived = 0, videoOutNackReceived = 0;
  bool videoOutActive = false; // a track is attached to the video sender

  /// One line for the call log: what each direction of video is doing (no addresses).
  String diag() {
    String f(double v) => v.toStringAsFixed(0);
    return 'video out: ${videoOutActive ? '$videoOutCodec/$videoEncoder ${f(videoOutFps)}fps ${f(videoOutKbps)}kbps '
                  '$videoOutSize limit=$videoLimit key=$videoOutKeyFrames pli<-$videoOutPliReceived nack<-$videoOutNackReceived' : 'none'}'
        ' | in: $videoInCodec/$videoDecoder ${f(videoInKbps)}kbps recv=${f(videoInFramesReceived)}fps '
        'dec=${f(videoInFps)}fps $videoInSize loss=${videoLossPct.toStringAsFixed(1)}% '
        'drop=$videoInDropped freeze=$videoInFreezes key=$videoInKeyFrames pli->$videoInPliSent nack->$videoInNackSent'
        ' | $route rtt=${rttMs?.toStringAsFixed(0) ?? '?'}';
  }

  @override
  String toString() {
    String f(double v) => v.toStringAsFixed(0);
    final lines = [
      '${tr('маршрут', 'route')}: $route${rttMs == null ? '' : ', RTT ${f(rttMs!)} ${tr('мс', 'ms')}'}',
      '${tr('аудио', 'audio')} $audioCodec: ↑${f(audioOutKbps)} ↓${f(audioInKbps)} ${tr('кбит/с', 'kbit/s')}, '
          '${tr('потери', 'loss')} ${audioLossPct.toStringAsFixed(1)}%',
      if (videoCodec.isNotEmpty)
        '${tr('видео', 'video')} $videoCodec: ↑${f(videoOutKbps)} ${f(videoOutFps)}fps $videoOutSize'
            ' ↓${f(videoInKbps)} ${f(videoInFps)}fps $videoInSize, ${tr('потери', 'loss')} ${videoLossPct.toStringAsFixed(1)}%',
      if (videoEncoder.isNotEmpty || videoDecoder.isNotEmpty)
        '${tr('кодер', 'encoder')} ${videoEncoder.isEmpty ? '—' : videoEncoder}, '
            '${tr('декодер', 'decoder')} ${videoDecoder.isEmpty ? '—' : videoDecoder}'
            '${videoLimit.isEmpty || videoLimit == 'none' ? '' : ', ${tr('упор', 'limited by')}: $videoLimit'}',
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
          s.route = '${relayed ? tr('через TURN', 'via TURN') : tr('напрямую', 'direct')} ($local↔$remote)';
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
            s.videoOutCodec = codec(v);
            s.videoOutActive = s.videoOutActive || v['framesEncoded'] is num && s.videoOutFps > 0;
            s.videoOutKeyFrames = v['keyFramesEncoded'] as num? ?? 0;
            s.videoOutPliReceived = v['pliCount'] as num? ?? 0;
            s.videoOutNackReceived = v['nackCount'] as num? ?? 0;
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
            s.videoInCodec = c;
            s.videoInFramesReceived = rate(r.id, v, 'framesReceived');
            s.videoInDropped = v['framesDropped'] as num? ?? 0;
            s.videoInFreezes = v['freezeCount'] as num? ?? 0;
            s.videoInKeyFrames = v['keyFramesDecoded'] as num? ?? 0;
            s.videoInPliSent = v['pliCount'] as num? ?? 0;
            s.videoInNackSent = v['nackCount'] as num? ?? 0;
          }
      }
      _prev[r.id] = v;
    }
    _prevAt = now;
    return s;
  }
}
