import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:url_launcher/url_launcher.dart';

import 'theme.dart';
import 'l10n.dart';

/// Links: http(s)://… and www.…; trailing punctuation stays outside the link.
final _link = RegExp(r'''(?:https?://|www\.)[^\s<>"]+''', caseSensitive: false);
final _trailing = RegExp(r'''[.,;:!?)\]}'"»]+$''');

/// Message text with clickable links: tap opens the browser, long press copies the link.
class LinkText extends StatelessWidget {
  const LinkText(this.text, {super.key, this.style});

  final String text;
  final TextStyle? style;

  static Future<void> _open(BuildContext context, String link) async {
    final messenger = ScaffoldMessenger.maybeOf(context);
    final uri = Uri.tryParse(link.startsWith('http') ? link : 'https://$link');
    if (uri == null || !await launchUrl(uri, mode: LaunchMode.externalApplication)) {
      messenger?.showSnackBar(SnackBar(content: Text(tr('Не удалось открыть ссылку', 'Couldn\'t open the link'))));
    }
  }

  static void _copy(BuildContext context, String link) {
    Clipboard.setData(ClipboardData(text: link));
    ScaffoldMessenger.maybeOf(context)?.showSnackBar(SnackBar(content: Text(tr('Ссылка скопирована', 'Link copied'))));
  }

  @override
  Widget build(BuildContext context) {
    final spans = <InlineSpan>[];
    var at = 0;
    for (final m in _link.allMatches(text)) {
      var link = m[0]!;
      final tail = _trailing.firstMatch(link)?[0] ?? '';
      link = link.substring(0, link.length - tail.length);
      if (link.length < 5) continue;
      if (m.start > at) spans.add(TextSpan(text: text.substring(at, m.start)));
      spans.add(
        WidgetSpan(
          alignment: PlaceholderAlignment.baseline,
          baseline: TextBaseline.alphabetic,
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: () => _open(context, link),
            onLongPress: () => _copy(context, link),
            child: Text(
              link,
              style: (style ?? const TextStyle()).copyWith(
                color: BotatoColors.potato,
                decoration: TextDecoration.underline,
                decorationColor: BotatoColors.potato,
              ),
            ),
          ),
        ),
      );
      at = m.start + link.length;
    }
    if (at < text.length) spans.add(TextSpan(text: text.substring(at)));
    return Text.rich(TextSpan(children: spans), style: style);
  }
}
