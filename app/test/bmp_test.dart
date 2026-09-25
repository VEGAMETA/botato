import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:botato/image_prep.dart';
import 'package:flutter_test/flutter_test.dart';

/// A 32 bpp BI_RGB bitmap as the Windows clipboard gives it: the 4th byte is unused (zero).
Uint8List bmp32(int w, int h) {
  final row = w * 4;
  final size = 54 + row * h;
  final b = ByteData(size);
  b.setUint8(0, 0x42);
  b.setUint8(1, 0x4d);
  b.setUint32(2, size, Endian.little);
  b.setUint32(10, 54, Endian.little);
  b.setUint32(14, 40, Endian.little);
  b.setInt32(18, w, Endian.little);
  b.setInt32(22, h, Endian.little);
  b.setUint16(26, 1, Endian.little);
  b.setUint16(28, 32, Endian.little);
  b.setUint32(34, row * h, Endian.little);
  for (var i = 54; i < size; i += 4) {
    b.setUint8(i, 200); // B
    b.setUint8(i + 1, 100); // G
    b.setUint8(i + 2, 50); // R
  }
  return b.buffer.asUint8List();
}

void main() {
  testWidgets('clipboard BMP becomes an opaque PNG', (tester) async {
    await tester.runAsync(() async {
      final png = await clipboardToPng(bmp32(4, 3));
      expect(isPng(png), isTrue);
      final codec = await ui.instantiateImageCodec(png);
      final img = (await codec.getNextFrame()).image;
      final px = (await img.toByteData(format: ui.ImageByteFormat.rawRgba))!;
      expect([img.width, img.height], [4, 3]);
      expect([px.getUint8(0), px.getUint8(1), px.getUint8(2), px.getUint8(3)], [50, 100, 200, 255]);
    });
  });
}
