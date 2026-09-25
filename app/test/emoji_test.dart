import 'package:botato/emoji.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('only emoji, up to three, count as big', () {
    expect(emojiOnly('👍'), 1);
    expect(emojiOnly(' 😂😂 '), 2);
    expect(emojiOnly('👨‍👩‍👧'), 1); // ZWJ family is one character
    expect(emojiOnly('🇷🇺'), 1); // flag
    expect(emojiOnly('👍🏽'), 1); // skin tone
    expect(emojiOnly('❤️'), 1);
    expect(emojiOnly('😂😂😂😂'), 0);
    expect(emojiOnly('ok 👍'), 0);
    expect(emojiOnly('1'), 0);
    expect(emojiOnly(''), 0);
  });
}
