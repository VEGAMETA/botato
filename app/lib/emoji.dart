import 'package:flutter/widgets.dart';

// The analyzer checks patterns without the unicode flag; these need it.
// ignore: valid_regexps
final _pictographic = RegExp(r'[\p{Extended_Pictographic}\u{1F1E6}-\u{1F1FF}]', unicode: true);
final _emojiParts = RegExp(
  // ignore: valid_regexps
  r'^[\p{Extended_Pictographic}\u{1F1E6}-\u{1F1FF}\u{FE0F}\u{FE0E}\u{200D}\u{20E3}\u{1F3FB}-\u{1F3FF}\u{E0020}-\u{E007F}#*0-9]+$',
  unicode: true,
);

/// 1–3 emoji and nothing else: shown large, without a bubble (like Telegram).
int emojiOnly(String text) {
  final t = text.trim();
  if (t.isEmpty) return 0;
  final chars = t.characters;
  if (chars.length > 3) return 0;
  for (final c in chars) {
    if (!_emojiParts.hasMatch(c) || !_pictographic.hasMatch(c)) return 0;
  }
  return chars.length;
}
