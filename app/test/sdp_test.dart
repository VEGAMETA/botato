import 'package:botato/call/sdp.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  const sdp =
      'v=0\r\n'
      'm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n'
      'a=rtpmap:111 opus/48000/2\r\n'
      'a=fmtp:111 minptime=10;useinbandfec=1\r\n'
      'm=video 9 UDP/TLS/RTP/SAVPF 96\r\n'
      'a=extmap:13 urn:3gpp:video-orientation\r\n'
      'a=extmap:3 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01\r\n'
      'a=rtpmap:96 VP8/90000\r\n';

  test('opus fmtp gets stereo, FEC and the bitrate ceiling', () {
    final out = tuneSdp(sdp);
    expect(
      out,
      contains(
        'a=fmtp:111 minptime=10;useinbandfec=1;stereo=1;sprop-stereo=1;maxaveragebitrate=128000;maxplaybackrate=48000\r\n',
      ),
    );
  });

  test('video orientation extension is removed, others stay', () {
    final out = tuneSdp(sdp);
    expect(out, isNot(contains('video-orientation')));
    expect(out, contains('a=extmap:3 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01\r\n'));
  });

  test('fmtp is added when missing', () {
    final out = tuneSdp('a=rtpmap:111 opus/48000/2\r\n');
    expect(out, contains('a=fmtp:111 stereo=1'));
  });
}
