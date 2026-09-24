import 'dart:io';

import 'package:flutter/material.dart';
import 'package:local_notifier/local_notifier.dart';
import 'package:tray_manager/tray_manager.dart';
import 'package:window_manager/window_manager.dart';

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

  Future<void> init() async {
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
        await windowManager.show();
        await windowManager.focus();
      },
    );
    await windowManager.setPreventClose(true);
    windowManager.addListener(this);

    await localNotifier.setup(appName: 'botato', shortcutPolicy: ShortcutPolicy.requireCreate);

    await trayManager.setIcon(Platform.isWindows ? 'assets/icon/tray.ico' : 'assets/icon/potato.png');
    if (!Platform.isLinux) await trayManager.setToolTip('botato');
    await trayManager.setContextMenu(
      Menu(
        items: [
          MenuItem(key: 'show', label: 'Открыть'),
          MenuItem.separator(),
          MenuItem(key: 'quit', label: 'Выход'),
        ],
      ),
    );
    trayManager.addListener(this);
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
    await _messages.remove(peerUser)?.close();
    final n = LocalNotification(identifier: 'msg-$peerUser', title: name, body: text);
    n.onClick = () {
      _messages.remove(peerUser);
      showWindow();
      onOpenChat?.call(peerUser);
    };
    _messages[peerUser] = n;
    await n.show();
  }

  Future<void> clearMessages(String peerUser) async => _messages.remove(peerUser)?.close();

  Future<void> showIncoming(String peer, {required bool video}) async {
    await _call?.close();
    final n = LocalNotification(
      identifier: 'call',
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
  void onTrayIconRightMouseDown() => trayManager.popUpContextMenu();

  @override
  void onTrayMenuItemClick(MenuItem menuItem) {
    if (menuItem.key == 'show') showWindow();
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

  @override
  Widget build(BuildContext context) {
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
