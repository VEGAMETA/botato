import 'dart:io';

import 'package:flutter/material.dart';
import 'package:local_notifier/local_notifier.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:tray_manager/tray_manager.dart';
import 'package:window_manager/window_manager.dart';

import 'sounds.dart';
import 'theme.dart';

bool get isDesktop => Platform.isWindows || Platform.isLinux || Platform.isMacOS;

/// Desktop shell: frameless window with our own title bar, close-to-tray, system
/// notifications for messages and incoming calls. Notifications stay on this machine.
class Desktop with TrayListener, WindowListener {
  Desktop._();

  static final instance = Desktop._();

  /// Click on a message notification: the peer user id.
  void Function(String peerUser)? onOpenChat;

  /// 'accept' / 'decline' from the incoming-call notification.
  void Function(String action)? onCallAction;

  bool _focused = true;

  /// Window focus changed (hidden to the tray counts as unfocused).
  void Function(bool focused)? onFocusChanged;
  final _messages = <String, LocalNotification>{};
  LocalNotification? _call;

  /// The window is on screen and has focus.
  bool get focused => _focused;

  static const _dndKey = 'desktop_dnd';
  bool _dnd = false;

  /// "Do not disturb" (tray menu): no system notifications and incoming calls don't raise
  /// the window; calls still ring inside the app.
  bool get dnd => _dnd;

  Future<void> init({bool hidden = false}) async {
    await windowManager.ensureInitialized();
    await windowManager.waitUntilReadyToShow(
      const WindowOptions(
        title: 'botato',
        titleBarStyle: TitleBarStyle.hidden,
        size: Size(1100, 760),
        minimumSize: Size(380, 480),
        center: true,
      ),
      () async {
        if (hidden) {
          _setFocused(false);
          return;
        }
        await windowManager.show();
        await windowManager.focus();
      },
    );
    await windowManager.setPreventClose(true);
    windowManager.addListener(this);

    await localNotifier.setup(appName: 'botato', shortcutPolicy: ShortcutPolicy.requireCreate);

    try {
      _dnd = (await SharedPreferences.getInstance()).getBool(_dndKey) ?? false;
    } catch (_) {}
    await trayManager.setIcon(Platform.isWindows ? 'assets/icon/tray.ico' : 'assets/icon/potato.png');
    await _updateTray();
    trayManager.addListener(this);
  }

  Future<void> _updateTray() async {
    if (!Platform.isLinux) await trayManager.setToolTip(_dnd ? 'botato — не беспокоить' : 'botato');
    await trayManager.setContextMenu(
      Menu(
        items: [
          MenuItem(key: 'show', label: 'Открыть'),
          MenuItem.checkbox(key: 'dnd', label: 'Не беспокоить', checked: _dnd),
          MenuItem.separator(),
          MenuItem(key: 'quit', label: 'Выход'),
        ],
      ),
    );
  }

  Future<void> setDnd(bool on) async {
    _dnd = on;
    try {
      await (await SharedPreferences.getInstance()).setBool(_dndKey, on);
    } catch (_) {}
    if (on) {
      for (final n in _messages.values) {
        await n.close();
      }
      _messages.clear();
    }
    await _updateTray();
  }

  Future<void> showWindow() async {
    if (await windowManager.isMinimized()) await windowManager.restore();
    await windowManager.show();
    await windowManager.focus();
  }

  Future<void> quit() async {
    trayManager.removeListener(this);
    await trayManager.destroy();
    await windowManager.setPreventClose(false);
    await windowManager.destroy();
  }

  // ---- notifications

  Future<void> showMessage(String peerUser, String name, String text) async {
    if (_dnd) return;
    await _messages.remove(peerUser)?.close();
    // Our own sound instead of the Windows one.
    final n = LocalNotification(identifier: 'msg-$peerUser', title: name, body: text, silent: true);
    n.onClick = () {
      _messages.remove(peerUser);
      showWindow();
      onOpenChat?.call(peerUser);
    };
    _messages[peerUser] = n;
    await n.show();
    await Sounds.play('message');
  }

  Future<void> clearMessages(String peerUser) async => _messages.remove(peerUser)?.close();

  Future<void> showIncoming(String peer, {required bool video}) async {
    if (_dnd) return;
    await _call?.close();
    final n = LocalNotification(
      identifier: 'call',
      silent: true, // the app rings itself (CallSounds)
      title: peer,
      body: video ? 'Видеозвонок' : 'Звонок',
      actions: [
        LocalNotificationAction(text: 'Принять'),
        LocalNotificationAction(text: 'Отклонить'),
      ],
    );
    n.onClick = showWindow;
    n.onClickAction = (i) {
      _call = null;
      if (i == 0) showWindow();
      onCallAction?.call(i == 0 ? 'accept' : 'decline');
    };
    _call = n;
    await n.show();
  }

  Future<void> cancelIncoming() async {
    await _call?.close();
    _call = null;
  }

  // ---- window / tray events

  @override
  void onWindowClose() => windowManager.hide(); // to the tray; "Выход" in the tray menu quits

  @override
  void onWindowFocus() => _setFocused(true);

  @override
  void onWindowBlur() => _setFocused(false);

  void _setFocused(bool f) {
    _focused = f;
    onFocusChanged?.call(f);
  }

  @override
  void onTrayIconMouseDown() => showWindow();

  @override
  // Foreground first, or Windows keeps the menu open after a click elsewhere.
  // ignore: deprecated_member_use
  void onTrayIconRightMouseDown() => trayManager.popUpContextMenu(bringAppToFront: true);

  @override
  void onTrayMenuItemClick(MenuItem menuItem) {
    if (menuItem.key == 'show') showWindow();
    if (menuItem.key == 'dnd') setDnd(!_dnd);
    if (menuItem.key == 'quit') quit();
  }
}

/// Our title bar: drag to move, double-click to maximize, close hides to the tray.
class TitleBar extends StatefulWidget {
  const TitleBar({super.key});

  static const height = 34.0;

  @override
  State<TitleBar> createState() => _TitleBarState();
}

class _TitleBarState extends State<TitleBar> with WindowListener {
  bool _maximized = false;

  @override
  void initState() {
    super.initState();
    windowManager.addListener(this);
    windowManager.isMaximized().then((m) {
      if (mounted) setState(() => _maximized = m);
    });
  }

  @override
  void dispose() {
    windowManager.removeListener(this);
    super.dispose();
  }

  @override
  void onWindowMaximize() => setState(() => _maximized = true);

  @override
  void onWindowUnmaximize() => setState(() => _maximized = false);

  bool _fullscreen = false;

  @override
  void onWindowEnterFullScreen() => setState(() => _fullscreen = true);

  @override
  void onWindowLeaveFullScreen() => setState(() => _fullscreen = false);

  @override
  Widget build(BuildContext context) {
    if (_fullscreen) return const SizedBox.shrink();
    final brightness = Theme.of(context).brightness;
    return Material(
      color: BotatoColors.night,
      child: SizedBox(
        height: TitleBar.height,
        child: Row(
          children: [
            Expanded(
              child: DragToMoveArea(
                child: Row(
                  children: [
                    const SizedBox(width: 12),
                    Text('botato', style: Theme.of(context).textTheme.labelLarge?.copyWith(color: BotatoColors.smoke)),
                  ],
                ),
              ),
            ),
            WindowCaptionButton.minimize(brightness: brightness, onPressed: windowManager.minimize),
            _maximized
                ? WindowCaptionButton.unmaximize(brightness: brightness, onPressed: windowManager.unmaximize)
                : WindowCaptionButton.maximize(brightness: brightness, onPressed: windowManager.maximize),
            // No Tooltip here: the bar sits above the Navigator, so there is no Overlay.
            WindowCaptionButton.close(brightness: brightness, onPressed: windowManager.hide),
          ],
        ),
      ),
    );
  }
}
