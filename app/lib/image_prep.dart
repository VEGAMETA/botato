import 'dart:async';
import 'dart:math';
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'l10n.dart';

/// PNG signature.
bool isPng(Uint8List b) => b.length > 8 && b[0] == 0x89 && b[1] == 0x50 && b[2] == 0x4e && b[3] == 0x47;

bool isJpeg(Uint8List b) => b.length > 3 && b[0] == 0xff && b[1] == 0xd8 && b[2] == 0xff;

/// A clipboard image as PNG. Windows hands out a BMP whose 32-bit pixels carry no real alpha
/// (the 4th byte is zero), so it is read as opaque; PNG and JPEG pass through.
Future<Uint8List> clipboardToPng(Uint8List bytes) async {
  if (isPng(bytes) || isJpeg(bytes)) return bytes;
  final png = await downscaleToPng(bytes, maxSide: 4096);
  if (bytes.length > 30 && bytes[0] == 0x42 && bytes[1] == 0x4d && bytes[28] == 32) return _opaque(png);
  return png;
}

Future<Uint8List> _opaque(Uint8List png) async {
  final codec = await ui.instantiateImageCodec(png);
  final frame = await codec.getNextFrame();
  final img = frame.image;
  final raw = (await img.toByteData(format: ui.ImageByteFormat.rawStraightRgba))!.buffer.asUint8List();
  final w = img.width, h = img.height;
  img.dispose();
  codec.dispose();
  var anyAlpha = false;
  for (var i = 3; i < raw.length; i += 4) {
    if (raw[i] != 0) {
      anyAlpha = true;
      break;
    }
  }
  if (anyAlpha) return png; // the decoder already treated it as opaque, or the alpha is real
  for (var i = 3; i < raw.length; i += 4) {
    raw[i] = 255;
  }
  final done = Completer<ui.Image>();
  ui.decodeImageFromPixels(raw, w, h, ui.PixelFormat.rgba8888, done.complete);
  final fixed = await done.future;
  final data = await fixed.toByteData(format: ui.ImageByteFormat.png);
  fixed.dispose();
  return data!.buffer.asUint8List();
}

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
    if (data == null) throw tr('не удалось прочитать картинку', 'couldn\'t read the image');
    return data.buffer.asUint8List();
  } finally {
    desc.dispose();
    buffer.dispose();
  }
}
