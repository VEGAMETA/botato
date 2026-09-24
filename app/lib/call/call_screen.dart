import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

import 'call_service.dart';
import 'media_settings.dart';

class CallScreen extends StatefulWidget {
  const CallScreen({super.key, required this.calls});

  final CallService calls;

  @override
  State<CallScreen> createState() => _CallScreenState();
}

class _CallScreenState extends State<CallScreen> {
  Timer? _tick;

  @override
  void initState() {
    super.initState();
    widget.calls.addListener(_onChange);
    _tick = Timer.periodic(const Duration(seconds: 1), (_) => setState(() {}));
  }

  @override
  void dispose() {
    widget.calls.removeListener(_onChange);
    _tick?.cancel();
    super.dispose();
  }

  void _onChange() {
    if (widget.calls.phase == CallPhase.idle && mounted) {
      Navigator.of(context).maybePop();
    } else {
      setState(() {});
    }
  }

  String _status(CallService c) {
    if (c.reconnecting) return 'переподключение…';
    return switch (c.phase) {
      CallPhase.outgoing => 'вызов…',
      CallPhase.incoming => c.videoCall ? 'входящий видеозвонок' : 'входящий звонок',
      CallPhase.connecting => 'соединение…',
      CallPhase.active => _elapsed(c.connectedAt),
      CallPhase.ended => c.endReason ?? 'завершён',
      CallPhase.idle => '',
    };
  }

  static String _elapsed(DateTime? t) {
    if (t == null) return '';
    final d = DateTime.now().difference(t);
    final m = d.inMinutes.toString().padLeft(2, '0');
    final s = (d.inSeconds % 60).toString().padLeft(2, '0');
    return '$m:$s';
  }

  Future<void> _shareScreen(CallService c) async {
    if (c.screenOn) return c.stopScreenShare();
    if (Platform.isAndroid) return c.startScreenShare(null);
    final id = await showDialog<String>(context: context, builder: (_) => const _ScreenPicker());
    if (id != null) await c.startScreenShare(id);
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.calls;
    final live = c.phase == CallPhase.active || c.phase == CallPhase.connecting;
    final showRemote = live && c.peerVideo;
    final showLocal = live && (c.cameraOn || c.screenOn);
    return PopScope(
      canPop: c.phase == CallPhase.idle || c.phase == CallPhase.ended,
      child: Scaffold(
        backgroundColor: Colors.black,
        body: Stack(
          children: [
            if (showRemote)
              Positioned.fill(
                child: RTCVideoView(c.remoteRenderer, objectFit: RTCVideoViewObjectFit.RTCVideoViewObjectFitContain),
              ),
            if (showLocal)
              Positioned(
                right: 16,
                top: 48,
                width: 120,
                height: 170,
                child: ClipRRect(
                  borderRadius: BorderRadius.circular(12),
                  child: RTCVideoView(
                    c.localRenderer,
                    mirror: c.cameraOn && !c.screenOn,
                    objectFit: RTCVideoViewObjectFit.RTCVideoViewObjectFitCover,
                  ),
                ),
              ),
            SafeArea(
              child: Column(
                children: [
                  const SizedBox(height: 48),
                  Text(c.peerName, style: const TextStyle(color: Colors.white, fontSize: 32)),
                  const SizedBox(height: 8),
                  Text(_status(c), style: const TextStyle(color: Colors.white70, fontSize: 18)),
                  const Spacer(),
                  Padding(padding: const EdgeInsets.only(bottom: 32), child: _buttons(c)),
                ],
              ),
            ),
            SafeArea(
              child: Align(
                alignment: Alignment.topLeft,
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    IconButton(
                      tooltip: 'Статистика',
                      icon: const Icon(Icons.insights, color: Colors.white54),
                      onPressed: c.toggleStats,
                    ),
                    if (live)
                      IconButton(
                        tooltip: 'Настройки',
                        icon: const Icon(Icons.tune, color: Colors.white54),
                        onPressed: () => showMediaSettings(context, c),
                      ),
                  ],
                ),
              ),
            ),
            if (c.showStats && c.stats != null)
              SafeArea(
                child: Padding(
                  padding: const EdgeInsets.only(left: 12, top: 140, right: 12),
                  child: Container(
                    color: Colors.black54,
                    padding: const EdgeInsets.all(8),
                    child: Text(
                      c.stats.toString(),
                      style: const TextStyle(color: Colors.white, fontSize: 12, fontFamily: 'monospace'),
                    ),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }

  Widget _buttons(CallService c) {
    if (c.phase == CallPhase.incoming) {
      return Wrap(
        alignment: WrapAlignment.spaceEvenly,
        spacing: 24,
        runSpacing: 16,
        children: [
          _round(Icons.call_end, Colors.red, c.decline, 'Отклонить'),
          _round(Icons.call, Colors.green, () => c.accept(video: false), 'Голосом'),
          if (c.videoCall) _round(Icons.videocam, Colors.green, () => c.accept(video: true), 'С видео'),
        ],
      );
    }
    if (c.phase == CallPhase.ended) return const SizedBox(height: 96);
    final live = c.phase == CallPhase.active || c.phase == CallPhase.connecting;
    return Wrap(
      alignment: WrapAlignment.spaceEvenly,
      spacing: 20,
      runSpacing: 16,
      children: [
        _toggle(c.muted ? Icons.mic_off : Icons.mic, c.muted, c.toggleMute, 'Микрофон'),
        if (live) _toggle(c.cameraOn ? Icons.videocam : Icons.videocam_off, c.cameraOn, c.toggleCamera, 'Камера'),
        if (live) _toggle(Icons.screen_share, c.screenOn, () => _shareScreen(c), 'Экран'),
        _round(Icons.call_end, Colors.red, () => c.hangup(), 'Завершить'),
        if (Platform.isAndroid)
          _toggle(c.speaker ? Icons.volume_up : Icons.hearing, c.speaker, c.toggleSpeaker, 'Динамик'),
      ],
    );
  }

  Widget _toggle(IconData icon, bool on, VoidCallback onTap, String label) =>
      _round(icon, on ? Colors.white : Colors.white24, onTap, label, iconColor: on ? Colors.black : Colors.white);

  Widget _round(IconData icon, Color bg, VoidCallback onTap, String label, {Color iconColor = Colors.white}) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        RawMaterialButton(
          onPressed: onTap,
          fillColor: bg,
          shape: const CircleBorder(),
          constraints: const BoxConstraints.tightFor(width: 64, height: 64),
          child: Icon(icon, color: iconColor, size: 28),
        ),
        const SizedBox(height: 6),
        Text(label, style: const TextStyle(color: Colors.white70, fontSize: 12)),
      ],
    );
  }
}

/// Desktop: choose a screen or a window to share.
class _ScreenPicker extends StatefulWidget {
  const _ScreenPicker();

  @override
  State<_ScreenPicker> createState() => _ScreenPickerState();
}

class _ScreenPickerState extends State<_ScreenPicker> {
  List<DesktopCapturerSource>? _sources;

  @override
  void initState() {
    super.initState();
    desktopCapturer
        .getSources(types: [SourceType.Screen, SourceType.Window], thumbnailSize: ThumbnailSize(320, 180))
        .then((s) => setState(() => _sources = s));
  }

  @override
  Widget build(BuildContext context) {
    final sources = _sources;
    return AlertDialog(
      title: const Text('Что показать'),
      content: SizedBox(
        width: 720,
        height: 420,
        child: sources == null
            ? const Center(child: CircularProgressIndicator())
            : GridView.count(
                crossAxisCount: 3,
                childAspectRatio: 16 / 11,
                mainAxisSpacing: 8,
                crossAxisSpacing: 8,
                children: [
                  for (final s in sources)
                    InkWell(
                      onTap: () => Navigator.pop(context, s.id),
                      child: Column(
                        children: [
                          Expanded(
                            child: s.thumbnail == null
                                ? const Icon(Icons.desktop_windows, size: 48)
                                : Image.memory(s.thumbnail!, fit: BoxFit.contain, gaplessPlayback: true),
                          ),
                          Text(s.name, maxLines: 1, overflow: TextOverflow.ellipsis),
                        ],
                      ),
                    ),
                ],
              ),
      ),
      actions: [TextButton(onPressed: () => Navigator.pop(context), child: const Text('Отмена'))],
    );
  }
}

/// Audio mode and device selection; usable in and out of a call.
Future<void> showMediaSettings(BuildContext context, CallService c) {
  return showModalBottomSheet(
    context: context,
    isScrollControlled: true,
    builder: (_) => _MediaSettingsSheet(calls: c),
  );
}

class _MediaSettingsSheet extends StatefulWidget {
  const _MediaSettingsSheet({required this.calls});

  final CallService calls;

  @override
  State<_MediaSettingsSheet> createState() => _MediaSettingsSheetState();
}

class _MediaSettingsSheetState extends State<_MediaSettingsSheet> {
  List<MediaDeviceInfo> _devices = [];

  @override
  void initState() {
    super.initState();
    navigator.mediaDevices.enumerateDevices().then((d) {
      if (mounted) setState(() => _devices = d);
    });
  }

  Widget _picker(String title, String kind, String? current, Future<void> Function(String?) onPick) {
    final list = _devices.where((d) => d.kind == kind).toList();
    if (list.isEmpty) return const SizedBox.shrink();
    final value = list.any((d) => d.deviceId == current) ? current : null;
    return ListTile(
      title: Text(title),
      subtitle: DropdownButton<String?>(
        isExpanded: true,
        value: value,
        items: [
          const DropdownMenuItem(value: null, child: Text('по умолчанию')),
          for (final d in list)
            DropdownMenuItem(
              value: d.deviceId,
              child: Text(d.label.isEmpty ? d.deviceId : d.label, overflow: TextOverflow.ellipsis),
            ),
        ],
        onChanged: (v) async {
          await onPick(v);
          setState(() {});
        },
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.calls;
    final s = c.settings;
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text('Звук', style: TextStyle(fontSize: 18)),
            const SizedBox(height: 8),
            SegmentedButton<AudioMode>(
              segments: const [
                ButtonSegment(value: AudioMode.voice, label: Text('Голос'), icon: Icon(Icons.record_voice_over)),
                ButtonSegment(value: AudioMode.clean, label: Text('Чистый звук'), icon: Icon(Icons.graphic_eq)),
              ],
              selected: {s.audioMode},
              onSelectionChanged: (v) async {
                await c.setAudioMode(v.first);
                setState(() {});
              },
            ),
            if (s.audioMode == AudioMode.clean)
              const Padding(
                padding: EdgeInsets.only(top: 8),
                child: Text(
                  'Без шумо- и эхоподавления, стерео 128 кбит/с. Только в наушниках, иначе будет эхо.',
                  style: TextStyle(color: Colors.orange),
                ),
              ),
            if (c.isDesktop) ...[
              _picker('Микрофон', 'audioinput', s.micId, c.setMicrophone),
              _picker('Вывод звука', 'audiooutput', s.outputId, c.setOutput),
              _picker('Камера', 'videoinput', s.cameraId, c.setCamera),
            ] else if (c.cameraOn)
              ListTile(
                leading: const Icon(Icons.cameraswitch),
                title: const Text('Сменить камеру'),
                onTap: c.switchCamera,
              ),
          ],
        ),
      ),
    );
  }
}
