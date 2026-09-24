import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

import 'call_service.dart';

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

  String _status(CallService c) => switch (c.phase) {
    CallPhase.outgoing => 'вызов…',
    CallPhase.incoming => c.video ? 'входящий видеозвонок' : 'входящий звонок',
    CallPhase.connecting => 'соединение…',
    CallPhase.active => _elapsed(c.connectedAt),
    CallPhase.ended => c.endReason ?? 'завершён',
    CallPhase.idle => '',
  };

  static String _elapsed(DateTime? t) {
    if (t == null) return '';
    final d = DateTime.now().difference(t);
    final m = d.inMinutes.toString().padLeft(2, '0');
    final s = (d.inSeconds % 60).toString().padLeft(2, '0');
    return '$m:$s';
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.calls;
    final showVideo = c.video && (c.phase == CallPhase.active || c.phase == CallPhase.connecting);
    return PopScope(
      canPop: c.phase == CallPhase.idle || c.phase == CallPhase.ended,
      child: Scaffold(
        backgroundColor: Colors.black,
        body: Stack(
          children: [
            if (showVideo)
              Positioned.fill(
                child: RTCVideoView(c.remoteRenderer, objectFit: RTCVideoViewObjectFit.RTCVideoViewObjectFitCover),
              ),
            if (showVideo)
              Positioned(
                right: 16,
                top: 48,
                width: 110,
                height: 160,
                child: ClipRRect(
                  borderRadius: BorderRadius.circular(12),
                  child: RTCVideoView(
                    c.localRenderer,
                    mirror: true,
                    objectFit: RTCVideoViewObjectFit.RTCVideoViewObjectFitCover,
                  ),
                ),
              ),
            SafeArea(
              child: Align(
                alignment: Alignment.topLeft,
                child: IconButton(
                  tooltip: 'Статистика',
                  icon: const Icon(Icons.insights, color: Colors.white54),
                  onPressed: c.toggleStats,
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
            SafeArea(
              child: Column(
                children: [
                  const SizedBox(height: 48),
                  Text(c.peerName, style: const TextStyle(color: Colors.white, fontSize: 32)),
                  const SizedBox(height: 8),
                  Text(_status(c), style: const TextStyle(color: Colors.white70, fontSize: 18)),
                  const Spacer(),
                  Padding(padding: const EdgeInsets.only(bottom: 48), child: _buttons(c)),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buttons(CallService c) {
    if (c.phase == CallPhase.incoming) {
      return Row(
        mainAxisAlignment: MainAxisAlignment.spaceEvenly,
        children: [
          _round(Icons.call_end, Colors.red, c.decline, 'Отклонить'),
          _round(c.video ? Icons.videocam : Icons.call, Colors.green, c.accept, 'Принять'),
        ],
      );
    }
    if (c.phase == CallPhase.ended) return const SizedBox(height: 72);
    return Row(
      mainAxisAlignment: MainAxisAlignment.spaceEvenly,
      children: [
        _round(
          c.muted ? Icons.mic_off : Icons.mic,
          c.muted ? Colors.white : Colors.white24,
          c.toggleMute,
          'Микрофон',
          iconColor: c.muted ? Colors.black : Colors.white,
        ),
        _round(Icons.call_end, Colors.red, () => c.hangup(), 'Завершить'),
        if (Platform.isAndroid)
        _round(
          c.speaker ? Icons.volume_up : Icons.hearing,
          c.speaker ? Colors.white : Colors.white24,
          c.toggleSpeaker,
          'Динамик',
          iconColor: c.speaker ? Colors.black : Colors.white,
        ),
      ],
    );
  }

  Widget _round(IconData icon, Color bg, VoidCallback onTap, String label, {Color iconColor = Colors.white}) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        RawMaterialButton(
          onPressed: onTap,
          fillColor: bg,
          shape: const CircleBorder(),
          constraints: const BoxConstraints.tightFor(width: 72, height: 72),
          child: Icon(icon, color: iconColor, size: 32),
        ),
        const SizedBox(height: 8),
        Text(label, style: const TextStyle(color: Colors.white70)),
      ],
    );
  }
}
