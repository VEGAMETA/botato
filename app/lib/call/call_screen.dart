import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';
import 'package:window_manager/window_manager.dart';

import '../screens/chat_screen.dart';
import '../theme.dart';
import 'call_service.dart';
import 'media_settings.dart';
import '../l10n.dart';

class CallScreen extends StatefulWidget {
  const CallScreen({super.key, required this.calls});

  final CallService calls;

  @override
  State<CallScreen> createState() => _CallScreenState();
}

class _CallScreenState extends State<CallScreen> {
  Timer? _tick;

  /// Buttons and labels are shown; a tap on the video toggles them, and they hide by
  /// themselves after a few seconds while the peer's video is on screen.
  bool _chrome = true;
  Timer? _hide;
  bool _fullscreen = false;

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
    _hide?.cancel();
    if (_fullscreen) windowManager.setFullScreen(false);
    SystemChrome.setEnabledSystemUIMode(SystemUiMode.edgeToEdge);
    super.dispose();
  }

  bool get _videoOnScreen {
    final c = widget.calls;
    return (c.phase == CallPhase.active || c.phase == CallPhase.connecting) && c.peerVideo;
  }

  void _setChrome(bool on) {
    _hide?.cancel();
    if (!on && !_videoOnScreen) on = true;
    if (on && _videoOnScreen) _hide = Timer(const Duration(seconds: 4), () => _setChrome(false));
    if (on == _chrome) return;
    setState(() => _chrome = on);
    if (Platform.isAndroid) {
      SystemChrome.setEnabledSystemUIMode(on ? SystemUiMode.edgeToEdge : SystemUiMode.immersiveSticky);
    }
  }

  Future<void> _toggleFullscreen() async {
    _fullscreen = !_fullscreen;
    await windowManager.setFullScreen(_fullscreen);
    if (_fullscreen) _setChrome(false);
    setState(() {});
  }

  /// Leaves the call screen (the call goes on, the banner leads back) — only on an unlocked
  /// phone: over the lock screen the chats stay closed.
  Future<void> _minimize() async {
    if (!await widget.calls.notifier.ensureUnlocked() || !mounted) return;
    Navigator.of(context).pop();
  }

  /// Leaves the call running and shows the chat with the peer; the banner leads back.
  Future<void> _openChat() async {
    final c = widget.calls;
    if (!await c.notifier.ensureUnlocked() || !mounted) return;
    final contact = c.peerUser == null ? null : c.relay.contacts.where((x) => x.userId == c.peerUser).firstOrNull;
    final nav = Navigator.of(context);
    nav.pop();
    if (contact != null) {
      nav.push(
        MaterialPageRoute(
          builder: (_) => ChatScreen(contact: contact, relay: c.relay, calls: c),
        ),
      );
    }
  }

  void _onChange() {
    if (widget.calls.phase == CallPhase.idle && mounted) {
      // The call is over: PopScope lets it close without unlocking (the show-over-lock-screen
      // flag was already dropped when the call ended).
      Navigator.of(context).maybePop();
    } else {
      // Video just appeared: start the auto-hide; it went away: bring the buttons back.
      if (_videoOnScreen && _chrome && !(_hide?.isActive ?? false)) _setChrome(true);
      if (!_videoOnScreen && !_chrome) _setChrome(true);
      setState(() {});
    }
  }

  String _status(CallService c) {
    if (c.reconnecting) return tr('переподключение…', 'reconnecting…');
    return switch (c.phase) {
      CallPhase.outgoing => tr('вызов…', 'calling…'),
      CallPhase.incoming =>
        c.videoCall ? tr('входящий видеозвонок', 'incoming video call') : tr('входящий звонок', 'incoming call'),
      CallPhase.connecting => tr('соединение…', 'connecting…'),
      CallPhase.active => _elapsed(c.connectedAt),
      CallPhase.ended => c.endReason ?? tr('завершён', 'ended'),
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
    final chrome = _chrome || !showRemote;
    final canMinimize = c.phase != CallPhase.incoming && c.phase != CallPhase.ended && c.phase != CallPhase.idle;
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): () {
          if (_fullscreen) _toggleFullscreen();
        },
      },
      child: Focus(
        autofocus: true,
        child: PopScope(
          // A finished call closes freely. A running one is minimized by Back (the banner brings
          // it back) — after unlocking if the phone is locked; ringing can't be left.
          canPop: c.phase == CallPhase.idle || c.phase == CallPhase.ended,
          onPopInvokedWithResult: (didPop, _) {
            if (!didPop && c.phase != CallPhase.incoming) _minimize();
          },
          child: Scaffold(
            backgroundColor: BotatoColors.night,
            body: Stack(
              children: [
                const Positioned.fill(
                  child: DecoratedBox(
                    decoration: BoxDecoration(
                      gradient: LinearGradient(
                        begin: Alignment.topCenter,
                        end: Alignment.bottomCenter,
                        colors: [Color(0xFF123444), BotatoColors.night],
                      ),
                    ),
                  ),
                ),
                if (!showRemote)
                  Align(
                    alignment: const Alignment(0, -0.2),
                    child: _PulsingAvatar(
                      name: c.peerName,
                      pulsing: c.phase == CallPhase.outgoing || c.phase == CallPhase.incoming || c.reconnecting,
                    ),
                  ),
                if (showRemote)
                  Positioned.fill(
                    child: GestureDetector(
                      behavior: HitTestBehavior.opaque,
                      onTap: () => _setChrome(!_chrome),
                      child: RTCVideoView(
                        c.remoteRenderer,
                        objectFit: RTCVideoViewObjectFit.RTCVideoViewObjectFitContain,
                      ),
                    ),
                  ),
                if (showLocal && chrome)
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
                if (chrome)
                  SafeArea(
                    child: Column(
                      children: [
                        const SizedBox(height: 48),
                        Text(c.peerName, style: const TextStyle(color: Colors.white, fontSize: 32)),
                        const SizedBox(height: 8),
                        Text(_status(c), style: const TextStyle(color: Colors.white70, fontSize: 18)),
                        const Spacer(),
                        Container(
                          width: double.infinity,
                          margin: const EdgeInsets.fromLTRB(12, 0, 12, 20),
                          padding: const EdgeInsets.symmetric(vertical: 16, horizontal: 8),
                          decoration: BoxDecoration(
                            color: Colors.black.withValues(alpha: 0.35),
                            borderRadius: BorderRadius.circular(28),
                          ),
                          child: _buttons(c),
                        ),
                      ],
                    ),
                  ),
                if (chrome)
                  SafeArea(
                    child: Align(
                      alignment: Alignment.topRight,
                      child: Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          if (canMinimize && c.peerUser != null)
                            IconButton(
                              tooltip: tr('Чат', 'Chat'),
                              icon: const Icon(Icons.chat_bubble_outline, color: Colors.white70),
                              onPressed: _openChat,
                            ),
                          if (showRemote && c.isDesktop)
                            IconButton(
                              tooltip: _fullscreen
                                  ? tr('Выйти из полного экрана (Esc)', 'Exit full screen (Esc)')
                                  : tr('Полный экран', 'Full screen'),
                              icon: Icon(_fullscreen ? Icons.fullscreen_exit : Icons.fullscreen, color: Colors.white70),
                              onPressed: _toggleFullscreen,
                            ),
                          if (canMinimize)
                            IconButton(
                              tooltip: tr('Свернуть', 'Minimize'),
                              icon: const Icon(Icons.keyboard_arrow_down, color: Colors.white70),
                              onPressed: _minimize,
                            ),
                        ],
                      ),
                    ),
                  ),
                if (chrome)
                  SafeArea(
                    child: Align(
                      alignment: Alignment.topLeft,
                      child: Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          IconButton(
                            tooltip: tr('Статистика', 'Statistics'),
                            icon: const Icon(Icons.insights, color: Colors.white54),
                            onPressed: c.toggleStats,
                          ),
                          if (live)
                            IconButton(
                              tooltip: tr('Настройки', 'Settings'),
                              icon: const Icon(Icons.tune, color: Colors.white54),
                              onPressed: () => showMediaSettings(context, c),
                            ),
                        ],
                      ),
                    ),
                  ),
                if (chrome && c.showStats && c.stats != null)
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
          _round(Icons.call_end, Colors.red, c.decline, tr('Отклонить', 'Decline')),
          _round(Icons.call, Colors.green, () => c.accept(video: false), tr('Голосом', 'Voice')),
          if (c.videoCall) _round(Icons.videocam, Colors.green, () => c.accept(video: true), tr('С видео', 'Video')),
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
        _micButton(c),
        if (live)
          _toggle(c.cameraOn ? Icons.videocam : Icons.videocam_off, c.cameraOn, c.toggleCamera, tr('Камера', 'Camera')),
        if (live) _toggle(Icons.screen_share, c.screenOn, () => _shareScreen(c), tr('Экран', 'Screen')),
        _round(Icons.call_end, Colors.red, () => c.hangup(), tr('Завершить', 'End')),
        if (Platform.isAndroid)
          _toggle(_routeIcon(c.audioRoute), c.audioRoute != 'earpiece', () => _pickRoute(c), tr('Звук', 'Audio')),
      ],
    );
  }

  static IconData _routeIcon(String id) => switch (id) {
    'bluetooth' => Icons.bluetooth_audio,
    'wired-headset' => Icons.headset,
    'speaker' => Icons.volume_up,
    _ => Icons.phone_in_talk,
  };

  static String _routeName(String id) => switch (id) {
    'bluetooth' => 'Bluetooth',
    'wired-headset' => tr('Наушники', 'Headphones'),
    'speaker' => tr('Громкая связь', 'Loudspeaker'),
    _ => tr('Телефон', 'Phone'),
  };

  /// Only earpiece and loudspeaker: a toggle. With a headset around: pick from the list.
  Future<void> _pickRoute(CallService c) async {
    final outs = c.audioOutputs;
    if (!outs.contains('bluetooth') && !outs.contains('wired-headset')) return c.toggleSpeaker();
    final id = await showModalBottomSheet<String>(
      context: context,
      builder: (ctx) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            for (final o in outs)
              ListTile(
                leading: Icon(_routeIcon(o)),
                title: Text(_routeName(o)),
                trailing: o == c.audioRoute ? const Icon(Icons.check, color: BotatoColors.potato) : null,
                onTap: () => Navigator.pop(ctx, o),
              ),
          ],
        ),
      ),
    );
    if (id != null) await c.setAudioRoute(id);
  }

  /// The mic button fills from the bottom with the loudness actually sent: you see that the
  /// microphone works (and that it doesn't).
  Widget _micButton(CallService c) {
    final on = c.muted;
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        SizedBox(
          width: 64,
          height: 64,
          child: ClipOval(
            child: Stack(
              children: [
                Positioned.fill(child: ColoredBox(color: on ? Colors.white : Colors.white24)),
                if (!on)
                  Align(
                    alignment: Alignment.bottomCenter,
                    child: ValueListenableBuilder<double>(
                      valueListenable: c.micLevel,
                      builder: (_, level, _) => AnimatedContainer(
                        duration: const Duration(milliseconds: 120),
                        height: 64 * level,
                        color: BotatoColors.potato.withValues(alpha: 0.7),
                      ),
                    ),
                  ),
                Positioned.fill(
                  child: Material(
                    type: MaterialType.transparency,
                    child: InkWell(
                      onTap: c.toggleMute,
                      child: Icon(on ? Icons.mic_off : Icons.mic, color: on ? Colors.black : Colors.white, size: 28),
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
        const SizedBox(height: 6),
        Text(tr('Микрофон', 'Microphone'), style: const TextStyle(color: Colors.white70, fontSize: 12)),
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
      title: Text(tr('Что показать', 'What to share')),
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
      actions: [TextButton(onPressed: () => Navigator.pop(context), child: Text(tr('Отмена', 'Cancel')))],
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
  List<String> _codecs = [];

  @override
  void initState() {
    super.initState();
    navigator.mediaDevices.enumerateDevices().then((d) {
      if (mounted) setState(() => _devices = d);
    });
    CallService.availableVideoCodecs().then((c) {
      if (mounted) setState(() => _codecs = c);
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
          DropdownMenuItem(value: null, child: Text(tr('по умолчанию', 'default'))),
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
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(16),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(tr('Звук', 'Audio'), style: TextStyle(fontSize: 18)),
            const SizedBox(height: 8),
            SegmentedButton<AudioMode>(
              segments: [
                ButtonSegment(
                  value: AudioMode.voice,
                  label: Text(tr('Голос', 'Voice')),
                  icon: Icon(Icons.record_voice_over),
                ),
                ButtonSegment(
                  value: AudioMode.clean,
                  label: Text(tr('Чистый звук', 'Clean audio')),
                  icon: Icon(Icons.graphic_eq),
                ),
              ],
              selected: {s.audioMode},
              onSelectionChanged: (v) async {
                await c.setAudioMode(v.first);
                setState(() {});
              },
            ),
            if (s.audioMode == AudioMode.clean)
              Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Text(
                  tr(
                    'Без наушников собеседник может слышать эхо',
                    'Without headphones the other side may hear an echo',
                  ),
                  style: const TextStyle(color: BotatoColors.smoke, fontSize: 12),
                ),
              ),
            if (Platform.isAndroid)
              SwitchListTile(
                contentPadding: EdgeInsets.zero,
                value: s.rawMic,
                onChanged: (v) => setState(() => s.rawMic = v),
                title: Text(tr('Микрофон без обработки системы', 'Microphone without system processing')),
                subtitle: Text(tr('после перезапуска', 'after a restart')),
              ),
            if (Platform.isAndroid)
              SwitchListTile(
                contentPadding: EdgeInsets.zero,
                value: s.hwAudioProcessing,
                onChanged: (v) => setState(() => s.hwAudioProcessing = v),
                title: Text(tr('Шумоподавление телефона', 'Phone noise suppression')),
                subtitle: Text(tr('после перезапуска', 'after a restart')),
              ),
            if (c.isDesktop) ...[
              _picker(tr('Микрофон', 'Microphone'), 'audioinput', s.micId, c.setMicrophone),
              if (Platform.isWindows)
                SwitchListTile(
                  contentPadding: EdgeInsets.zero,
                  value: s.wasapiMic,
                  onChanged: (v) async {
                    await c.setWasapiMic(v);
                    setState(() {});
                  },
                  title: Text(tr('Прямой захват микрофона', 'Direct microphone capture')),
                  subtitle: Text(tr('если вас не слышно', "if you can't be heard")),
                ),
              _picker(tr('Вывод звука', 'Audio output'), 'audiooutput', s.outputId, c.setOutput),
              _picker(tr('Камера', 'Camera'), 'videoinput', s.cameraId, c.setCamera),
            ] else if (c.cameraOn)
              ListTile(
                leading: const Icon(Icons.cameraswitch),
                title: Text(tr('Сменить камеру', 'Switch camera')),
                onTap: c.switchCamera,
              ),
            const SizedBox(height: 16),
            Text(tr('Видео', 'Video'), style: TextStyle(fontSize: 18)),
            const SizedBox(height: 8),
            Row(
              children: [
                Expanded(child: Text(tr('Показ экрана', 'Screen sharing'))),
                SegmentedButton<int>(
                  segments: const [
                    ButtonSegment(value: 30, label: Text('30 fps')),
                    ButtonSegment(value: 60, label: Text('60 fps')),
                  ],
                  selected: {s.screenFps},
                  onSelectionChanged: (v) => setState(() => s.screenFps = v.first),
                ),
              ],
            ),
            if (_codecs.isNotEmpty)
              ListTile(
                contentPadding: EdgeInsets.zero,
                title: Text(tr('Видеокодек', 'Video codec')),
                subtitle: DropdownButton<String?>(
                  isExpanded: true,
                  value: _codecs.contains(s.videoCodec) ? s.videoCodec : null,
                  items: [
                    DropdownMenuItem(value: null, child: Text(tr('авто', 'auto'))),
                    for (final name in _codecs) DropdownMenuItem(value: name, child: Text(name)),
                  ],
                  onChanged: (v) => setState(() => s.videoCodec = v),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// Peer avatar; pulses while ringing or reconnecting.
class _PulsingAvatar extends StatefulWidget {
  const _PulsingAvatar({required this.name, required this.pulsing});

  final String name;
  final bool pulsing;

  @override
  State<_PulsingAvatar> createState() => _PulsingAvatarState();
}

class _PulsingAvatarState extends State<_PulsingAvatar> with SingleTickerProviderStateMixin {
  late final _anim = AnimationController(vsync: this, duration: const Duration(milliseconds: 1600))..repeat();

  @override
  void dispose() {
    _anim.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    const size = 132.0;
    final color = BotatoColors.avatarFor(widget.name);
    return SizedBox(
      width: size * 2,
      height: size * 2,
      child: AnimatedBuilder(
        animation: _anim,
        builder: (_, _) => Stack(
          alignment: Alignment.center,
          children: [
            if (widget.pulsing)
              for (final phase in [0.0, 0.5])
                Builder(
                  builder: (_) {
                    final t = (_anim.value + phase) % 1.0;
                    return Container(
                      width: size * (1 + t),
                      height: size * (1 + t),
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: color.withValues(alpha: 0.25 * (1 - t)),
                      ),
                    );
                  },
                ),
            Avatar(widget.name, size: size),
          ],
        ),
      ),
    );
  }
}
