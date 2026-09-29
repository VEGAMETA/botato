import 'dart:io';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';

import '../call/call_notifier.dart';
import '../image_prep.dart';
import '../src/rust/api/client.dart' as core;
import '../theme.dart';
import '../l10n.dart';

/// Decrypted photos recently shown (they live encrypted in the store).
class _PhotoCache {
  static const _max = 24;
  static final _items = <String, Uint8List>{};

  static Uint8List? get(String id) {
    final v = _items.remove(id);
    if (v != null) _items[id] = v; // most recent last
    return v;
  }

  static void put(String id, Uint8List v) {
    _items[id] = v;
    while (_items.length > _max) {
      _items.remove(_items.keys.first);
    }
  }
}

Future<Uint8List?> loadPhoto(String msgId) async {
  final cached = _PhotoCache.get(msgId);
  if (cached != null) return cached;
  final data = await core.clientPhoto(msgId: msgId);
  if (data != null) _PhotoCache.put(msgId, data);
  return data;
}

/// The photo part of a chat bubble: the preview until the file arrives, then the photo.
class PhotoContent extends StatefulWidget {
  const PhotoContent({super.key, required this.msgId, required this.photo, required this.gallery});

  final String msgId;
  final core.PhotoInfo photo;

  /// Downloaded photos of the chat, oldest first: the viewer swipes through them.
  final List<String> Function() gallery;

  @override
  State<PhotoContent> createState() => _PhotoContentState();
}

class _PhotoContentState extends State<PhotoContent> {
  Uint8List? _data;

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void didUpdateWidget(PhotoContent old) {
    super.didUpdateWidget(old);
    // The list may hand this state to another message.
    if (old.msgId != widget.msgId) _data = null;
    if (widget.photo.ready && _data == null) _load();
  }

  Future<void> _load() async {
    if (!widget.photo.ready) return;
    final id = widget.msgId;
    final d = await loadPhoto(id);
    if (mounted && d != null && id == widget.msgId) setState(() => _data = d);
  }

  @override
  Widget build(BuildContext context) {
    final p = widget.photo;
    const maxW = 260.0, maxH = 320.0;
    final ratio = p.width / (p.height == 0 ? 1 : p.height);
    var w = maxW, h = maxW / ratio;
    if (h > maxH) {
      h = maxH;
      w = maxH * ratio;
    }
    final data = _data;
    Widget child;
    if (data != null) {
      child = GestureDetector(
        onTap: () {
          var ids = widget.gallery();
          if (!ids.contains(widget.msgId)) ids = [widget.msgId];
          // Transparent route: while the photo is dragged away, the chat shows through.
          Navigator.of(context).push(
            PageRouteBuilder<void>(
              opaque: false,
              pageBuilder: (_, _, _) => PhotoViewer(ids: ids, initial: ids.indexOf(widget.msgId)),
              transitionsBuilder: (_, anim, _, child) => FadeTransition(opacity: anim, child: child),
            ),
          );
        },
        child: Hero(
          tag: widget.msgId,
          child: Image.memory(data, fit: BoxFit.cover, gaplessPlayback: true),
        ),
      );
    } else {
      child = Stack(
        fit: StackFit.expand,
        children: [
          if (p.thumb.isNotEmpty)
            Image.memory(p.thumb, fit: BoxFit.cover, filterQuality: FilterQuality.medium)
          else
            const ColoredBox(color: BotatoColors.night),
          Center(
            child: p.gone
                ? _Badge(icon: Icons.broken_image_outlined, text: tr('фото недоступно', 'photo unavailable'))
                : const SizedBox(width: 28, height: 28, child: CircularProgressIndicator(strokeWidth: 3)),
          ),
        ],
      );
    }
    return ClipRRect(
      borderRadius: BorderRadius.circular(12),
      child: SizedBox(width: w, height: h, child: child),
    );
  }
}

class _Badge extends StatelessWidget {
  const _Badge({required this.icon, required this.text});

  final IconData icon;
  final String text;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(color: Colors.black54, borderRadius: BorderRadius.circular(12)),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 16, color: Colors.white),
          const SizedBox(width: 6),
          Text(text, style: const TextStyle(color: Colors.white, fontSize: 12)),
        ],
      ),
    );
  }
}

/// Full-screen photos of a chat: swipe (phone) or arrows / ←→ keys (desktop) between them,
/// pinch or wheel to zoom.
class PhotoViewer extends StatefulWidget {
  const PhotoViewer({super.key, required this.ids, required this.initial});

  final List<String> ids;
  final int initial;

  @override
  State<PhotoViewer> createState() => _PhotoViewerState();
}

class _PhotoViewerState extends State<PhotoViewer> {
  late final PageController _pages = PageController(initialPage: widget.initial);
  late int _index = widget.initial;

  /// A zoomed-in photo pans instead of turning the page.
  bool _zoomed = false;

  /// Quarter turns per photo, view only.
  final _turns = <String, int>{};

  /// Toolbar and arrows are shown; a tap on the photo toggles them (Telegram-like).
  bool _chrome = true;

  /// Vertical drag of an unzoomed photo: far or fast enough closes the viewer. Tracked from raw
  /// pointer events, not a drag recognizer: a recognizer took the first finger of a pinch
  /// before the second one landed, and zooming rarely worked.
  double _dragDy = 0;
  int _pointers = 0;
  bool _dismissing = false;
  Offset _downAt = Offset.zero;
  Offset _lastAt = Offset.zero;
  Duration _lastTime = Duration.zero;
  double _velocity = 0; // px/s, vertical

  static bool get _desktop => Platform.isWindows || Platform.isLinux || Platform.isMacOS;

  @override
  void initState() {
    super.initState();
    if (Platform.isAndroid) SystemChrome.setEnabledSystemUIMode(SystemUiMode.immersiveSticky);
  }

  @override
  void dispose() {
    _pages.dispose();
    if (Platform.isAndroid) SystemChrome.setEnabledSystemUIMode(SystemUiMode.edgeToEdge);
    super.dispose();
  }

  void _toggleChrome() => setState(() => _chrome = !_chrome);

  void _pointerDown(PointerDownEvent e) {
    _pointers++;
    if (_pointers == 1) {
      _downAt = _lastAt = e.position;
      _lastTime = e.timeStamp;
      _velocity = 0;
    } else if (_dismissing) {
      _dismissing = false; // a second finger: it's a pinch after all
      _dragDy = 0;
    }
    setState(() {}); // page swiping off while two fingers are down
  }

  void _pointerMove(PointerMoveEvent e) {
    if (_pointers != 1 || _zoomed) return;
    final d = e.position - _downAt;
    if (!_dismissing && d.dy.abs() > 18 && d.dy.abs() > d.dx.abs() * 1.5) _dismissing = true;
    if (!_dismissing) return;
    final dt = (e.timeStamp - _lastTime).inMicroseconds;
    if (dt > 0) _velocity = (e.position.dy - _lastAt.dy) / dt * 1e6;
    _lastAt = e.position;
    _lastTime = e.timeStamp;
    setState(() => _dragDy = d.dy);
  }

  void _pointerUp(PointerEvent e) {
    _pointers = (_pointers - 1).clamp(0, 10);
    if (_pointers == 0 && _dismissing) {
      _dismissing = false;
      if (_dragDy.abs() > 120 || _velocity.abs() > 900) {
        Navigator.of(context).maybePop();
        return;
      }
      _dragDy = 0;
    }
    setState(() {});
  }

  void _go(int delta) {
    final next = _index + delta;
    if (next < 0 || next >= widget.ids.length) return;
    _pages.animateToPage(next, duration: const Duration(milliseconds: 220), curve: Curves.easeOut);
  }

  /// Android: hands the photo to another app through the system share sheet.
  Future<void> _share() async {
    final messenger = ScaffoldMessenger.of(context);
    final id = widget.ids[_index];
    final data = await loadPhoto(id);
    if (data == null) return;
    final (ext, mime) = imageType(data);
    try {
      final dir = Directory('${(await getTemporaryDirectory()).path}/share');
      if (dir.existsSync()) dir.deleteSync(recursive: true);
      dir.createSync(recursive: true);
      final file = File('${dir.path}/botato-${id.substring(0, 8)}.$ext');
      await file.writeAsBytes(data);
      await CallNotifier.shareFile(file.path, mime);
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(tr('Не получилось поделиться: $e', 'Couldn\'t share: $e'))));
    }
  }

  Future<void> _save() async {
    final messenger = ScaffoldMessenger.of(context);
    final id = widget.ids[_index];
    final data = await loadPhoto(id);
    if (data == null) return;
    final (ext, mime) = imageType(data);
    final name = 'botato-${id.substring(0, 8)}.$ext';
    if (Platform.isAndroid) {
      try {
        await CallNotifier.saveToGallery(name, data, mime);
        messenger.showSnackBar(
          SnackBar(
            content: Text(tr('Сохранено в галерею (Pictures/botato)', 'Saved to the gallery (Pictures/botato)')),
          ),
        );
      } catch (e) {
        messenger.showSnackBar(SnackBar(content: Text(tr('Не сохранилось: $e', 'Couldn\'t save: $e'))));
      }
      return;
    }
    final loc = await getSaveLocation(
      suggestedName: name,
      acceptedTypeGroups: [
        XTypeGroup(label: ext.toUpperCase(), extensions: [ext]),
      ],
    );
    if (loc == null) return;
    try {
      await File(loc.path).writeAsBytes(data);
      messenger.showSnackBar(SnackBar(content: Text(tr('Сохранено', 'Saved'))));
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(tr('Не сохранилось: $e', 'Couldn\'t save: $e'))));
    }
  }

  @override
  Widget build(BuildContext context) {
    final count = widget.ids.length;
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.arrowLeft): () => _go(-1),
        const SingleActivator(LogicalKeyboardKey.arrowRight): () => _go(1),
        const SingleActivator(LogicalKeyboardKey.escape): () => Navigator.of(context).maybePop(),
      },
      child: Focus(
        autofocus: true,
        child: Scaffold(
          // Fades out while the photo is dragged away, uncovering the chat.
          backgroundColor: Colors.black.withValues(alpha: (1 - _dragDy.abs() / 400).clamp(0.0, 1.0)),
          extendBodyBehindAppBar: true,
          appBar: !_chrome || _dragDy != 0
              ? null
              : AppBar(
                  backgroundColor: Colors.black54,
                  foregroundColor: Colors.white,
                  title: count > 1
                      ? Text(
                          tr('${_index + 1} из $count', '${_index + 1} of $count'),
                          style: const TextStyle(fontSize: 16),
                        )
                      : null,
                  actions: [
                    IconButton(
                      tooltip: tr('Повернуть', 'Rotate'),
                      icon: const Icon(Icons.rotate_90_degrees_cw_outlined),
                      onPressed: () => setState(() {
                        final id = widget.ids[_index];
                        _turns[id] = ((_turns[id] ?? 0) + 1) % 4;
                      }),
                    ),
                    if (Platform.isAndroid)
                      IconButton(tooltip: tr('Поделиться', 'Share'), icon: const Icon(Icons.share), onPressed: _share),
                    if (_desktop || Platform.isAndroid)
                      IconButton(
                        tooltip: _desktop
                            ? tr('Сохранить как…', 'Save as…')
                            : tr('Сохранить в галерею', 'Save to gallery'),
                        icon: const Icon(Icons.save_alt),
                        onPressed: _save,
                      ),
                  ],
                ),
          body: Stack(
            children: [
              Listener(
                onPointerDown: _pointerDown,
                onPointerMove: _pointerMove,
                onPointerUp: _pointerUp,
                onPointerCancel: _pointerUp,
                child: GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTap: _toggleChrome,
                  child: Transform.translate(
                    offset: Offset(0, _dragDy),
                    child: PageView.builder(
                      controller: _pages,
                      // No page swiping while zoomed, pinching (two fingers) or dismissing.
                      physics: _zoomed || _pointers > 1 || _dismissing
                          ? const NeverScrollableScrollPhysics()
                          : const PageScrollPhysics(),
                      itemCount: count,
                      onPageChanged: (i) => setState(() {
                        _index = i;
                        _zoomed = false;
                      }),
                      itemBuilder: (_, i) => _ZoomablePhoto(
                        key: ValueKey(widget.ids[i]),
                        msgId: widget.ids[i],
                        turns: _turns[widget.ids[i]] ?? 0,
                        onZoomed: (z) {
                          if (z != _zoomed) setState(() => _zoomed = z);
                        },
                      ),
                    ),
                  ),
                ),
              ),
              if (_chrome && _desktop && _index > 0) _Arrow(left: true, onTap: () => _go(-1)),
              if (_chrome && _desktop && _index < count - 1) _Arrow(left: false, onTap: () => _go(1)),
            ],
          ),
        ),
      ),
    );
  }
}

class _ZoomablePhoto extends StatefulWidget {
  const _ZoomablePhoto({super.key, required this.msgId, required this.turns, required this.onZoomed});

  final String msgId;
  final int turns;
  final ValueChanged<bool> onZoomed;

  @override
  State<_ZoomablePhoto> createState() => _ZoomablePhotoState();
}

class _ZoomablePhotoState extends State<_ZoomablePhoto> {
  final _transform = TransformationController();
  Uint8List? _data;

  @override
  void initState() {
    super.initState();
    loadPhoto(widget.msgId).then((d) {
      if (mounted) setState(() => _data = d);
    });
  }

  @override
  void dispose() {
    _transform.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final data = _data;
    if (data == null) return const Center(child: CircularProgressIndicator());
    return InteractiveViewer(
      transformationController: _transform,
      maxScale: 8,
      onInteractionEnd: (_) => widget.onZoomed(_transform.value.getMaxScaleOnAxis() > 1.01),
      // Fills the screen: small photos are scaled up to fit, like large ones are scaled down.
      child: SizedBox.expand(
        child: Hero(
          tag: widget.msgId,
          child: RotatedBox(
            quarterTurns: widget.turns,
            child: Image.memory(data, fit: BoxFit.contain, filterQuality: FilterQuality.medium),
          ),
        ),
      ),
    );
  }
}

class _Arrow extends StatelessWidget {
  const _Arrow({required this.left, required this.onTap});

  final bool left;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Align(
      alignment: left ? Alignment.centerLeft : Alignment.centerRight,
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: IconButton.filledTonal(
          iconSize: 28,
          onPressed: onTap,
          icon: Icon(left ? Icons.chevron_left : Icons.chevron_right),
        ),
      ),
    );
  }
}
