/// SDP adjustments applied before setLocalDescription.
String tuneSdp(String sdp) => _dropVideoOrientation(tuneOpus(sdp));

/// Without the CVO extension the sender rotates frames before encoding, so every
/// receiver shows them upright (the Windows renderer does not always honour CVO).
String _dropVideoOrientation(String sdp) =>
    sdp.replaceAll(RegExp(r'^a=extmap:\d+(/\w+)? urn:3gpp:video-orientation\r?\n', multiLine: true), '');

/// Opus `stereo=1`/`sprop-stereo=1` must be negotiated at call setup, otherwise the
/// "clean sound" mode can't switch to stereo later without renegotiation.
/// In-band FEC stays on in every mode.
String tuneOpus(String sdp) {
  final rtpmap = RegExp(r'a=rtpmap:(\d+) opus/48000/2', caseSensitive: false).firstMatch(sdp);
  if (rtpmap == null) return sdp;
  final pt = rtpmap.group(1)!;
  final fmtp = RegExp('^a=fmtp:$pt (.*)\$', multiLine: true);
  // maxaveragebitrate is the ceiling; the actual rate per audio mode is set with setParameters.
  const wanted = {
    'stereo': '1',
    'sprop-stereo': '1',
    'useinbandfec': '1',
    'maxaveragebitrate': '128000',
    'maxplaybackrate': '48000',
  };

  String merge(String params) {
    final map = <String, String>{};
    for (final p in params.split(';')) {
      final kv = p.trim().split('=');
      if (kv.length == 2) map[kv[0]] = kv[1];
    }
    map.addAll(wanted);
    return map.entries.map((e) => '${e.key}=${e.value}').join(';');
  }

  if (fmtp.hasMatch(sdp)) {
    return sdp.replaceFirstMapped(fmtp, (m) => 'a=fmtp:$pt ${merge(m.group(1)!)}');
  }
  // No fmtp line yet: add one right after the rtpmap.
  return sdp.replaceFirst(rtpmap.group(0)!, '${rtpmap.group(0)}\r\na=fmtp:$pt ${merge('')}');
}
