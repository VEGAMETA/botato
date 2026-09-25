import 'dart:io';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../call/call_notifier.dart';
import '../image_prep.dart';
import '../src/rust/api/client.dart' as core;
import '../theme.dart';

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
          Navigator.of(context).push(
            MaterialPageRoute(
              builder: (_) => PhotoViewer(ids: ids, initial: ids.indexOf(widget.msgId)),
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
                ? const _Badge(icon: Icons.broken_image_outlined, text: 'фото недоступно')
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

  static bool get _desktop => Platform.isWindows || Platform.isLinux || Platform.isMacOS;

  @override
  void dispose() {
    _pages.dispose();
    super.dispose();
  }

  void _go(int delta) {
    final next = _index + delta;
    if (next < 0 || next >= widget.ids.length) return;
    _pages.animateToPage(next, duration: const Duration(milliseconds: 220), curve: Curves.easeOut);
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
        messenger.showSnackBar(const SnackBar(content: Text('Сохранено в галерею (Pictures/botato)')));
      } catch (e) {
        messenger.showSnackBar(SnackBar(content: Text('Не сохранилось: $e')));
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
      messenger.showSnackBar(const SnackBar(content: Text('Сохранено')));
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text('Не сохранилось: $e')));
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
          backgroundColor: Colors.black,
          appBar: AppBar(
            backgroundColor: Colors.black,
            foregroundColor: Colors.white,
            title: count > 1 ? Text('${_index + 1} из $count', style: const TextStyle(fontSize: 16)) : null,
            actions: [
              IconButton(
                tooltip: 'Повернуть',
                icon: const Icon(Icons.rotate_90_degrees_cw_outlined),
                onPressed: () => setState(() {
                  final id = widget.ids[_index];
                  _turns[id] = ((_turns[id] ?? 0) + 1) % 4;
                }),
              ),
              if (_desktop || Platform.isAndroid)
                IconButton(
                  tooltip: _desktop ? 'Сохранить как…' : 'Сохранить в галерею',
                  icon: const Icon(Icons.save_alt),
                  onPressed: _save,
                ),
            ],
          ),
          body: Stack(
            children: [
              PageView.builder(
                controller: _pages,
                physics: _zoomed ? const NeverScrollableScrollPhysics() : const PageScrollPhysics(),
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
              if (_desktop && _index > 0) _Arrow(left: true, onTap: () => _go(-1)),
              if (_desktop && _index < count - 1) _Arrow(left: false, onTap: () => _go(1)),
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
      child: Center(
        child: Hero(
          tag: widget.msgId,
          child: RotatedBox(quarterTurns: widget.turns, child: Image.memory(data)),
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
