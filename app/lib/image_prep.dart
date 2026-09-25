import 'dart:math';
import 'dart:typed_data';
import 'dart:ui' as ui;

/// PNG signature.
bool isPng(Uint8List b) => b.length > 8 && b[0] == 0x89 && b[1] == 0x50 && b[2] == 0x4e && b[3] == 0x47;

/// File extension and MIME type of a sent photo (JPEG or PNG, §6.3).
(String ext, String mime) imageType(Uint8List b) => isPng(b) ? ('png', 'image/png') : ('jpg', 'image/jpeg');

/// Phones: decodes any gallery image the OS understands (HEIC included) at most [maxSide]
/// pixels on the long side, with its EXIF orientation applied, and returns it as PNG. Rust
/// then makes the final JPEG; a 100-megapixel original never reaches it at full size.
Future<Uint8List> downscaleToPng(Uint8List bytes, {int maxSide = 3200}) async {
  final buffer = await ui.ImmutableBuffer.fromUint8List(bytes);
  final desc = await ui.ImageDescriptor.encoded(buffer);
  try {
    final long = max(desc.width, desc.height);
    // Only the width is given: the decoder keeps the aspect ratio either way.
    final width = long > maxSide ? (desc.width * maxSide / long).round() : null;
    final codec = await desc.instantiateCodec(targetWidth: width);
    final frame = await codec.getNextFrame();
    final data = await frame.image.toByteData(format: ui.ImageByteFormat.png);
    frame.image.dispose();
    codec.dispose();
    if (data == null) throw 'не удалось прочитать картинку';
    return data.buffer.asUint8List();
  } finally {
    desc.dispose();
    buffer.dispose();
  }
}
