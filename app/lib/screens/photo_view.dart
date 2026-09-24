import 'dart:io';
import 'dart:typed_data';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';

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
  const PhotoContent({super.key, required this.msgId, required this.photo});

  final String msgId;
  final core.PhotoInfo photo;

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
    if (widget.photo.ready && _data == null) _load();
  }

  Future<void> _load() async {
    if (!widget.photo.ready) return;
    final d = await loadPhoto(widget.msgId);
    if (mounted && d != null) setState(() => _data = d);
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
        onTap: () => Navigator.of(context).push(
          MaterialPageRoute(
            builder: (_) => PhotoViewer(msgId: widget.msgId, data: data),
          ),
        ),
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

/// Full-screen photo with pinch / wheel zoom.
class PhotoViewer extends StatelessWidget {
  const PhotoViewer({super.key, required this.msgId, required this.data});

  final String msgId;
  final Uint8List data;

  static bool get _desktop => Platform.isWindows || Platform.isLinux || Platform.isMacOS;

  Future<void> _save(BuildContext context) async {
    final messenger = ScaffoldMessenger.of(context);
    final loc = await getSaveLocation(
      suggestedName: 'botato-${msgId.substring(0, 8)}.jpg',
      acceptedTypeGroups: const [
        XTypeGroup(label: 'JPEG', extensions: ['jpg']),
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
    return Scaffold(
      backgroundColor: Colors.black,
      appBar: AppBar(
        backgroundColor: Colors.black,
        foregroundColor: Colors.white,
        actions: [
          if (_desktop)
            IconButton(tooltip: 'Сохранить как…', icon: const Icon(Icons.save_alt), onPressed: () => _save(context)),
        ],
      ),
      body: InteractiveViewer(
        maxScale: 8,
        child: Center(
          child: Hero(tag: msgId, child: Image.memory(data)),
        ),
      ),
    );
  }
}
