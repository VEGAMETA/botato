import 'package:flutter/material.dart';

import '../theme.dart';
import 'call_service.dart';
import '../l10n.dart';

/// While a call runs with its screen closed (the user went to a chat), a bar above the
/// whole app leads back to it.
///
/// The tree shape never changes: [child] is the app's Navigator, and moving it would
/// rebuild every open screen.
class CallBanner extends StatelessWidget {
  const CallBanner({super.key, required this.calls, required this.child});

  final CallService calls;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: calls,
      child: child,
      builder: (context, child) {
        final show = calls.busy && calls.phase != CallPhase.ended && !calls.screenShown;
        return Column(
          children: [
            if (show) _bar() else const SizedBox.shrink(),
            // The bar took the status-bar inset; the app below must not add it again.
            Expanded(
              child: MediaQuery.removePadding(context: context, removeTop: show, child: child!),
            ),
          ],
        );
      },
    );
  }

  Widget _bar() {
    return Material(
      color: BotatoColors.ok,
      child: SafeArea(
        bottom: false,
        child: InkWell(
          onTap: calls.showScreen,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
            child: Row(
              children: [
                const Icon(Icons.call, color: Colors.white, size: 18),
                const SizedBox(width: 10),
                Expanded(
                  child: Text(
                    '${calls.peerName} · ${calls.reconnecting ? tr('переподключение…', 'reconnecting…') : tr('идёт звонок', 'call in progress')}',
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(color: Colors.white, fontWeight: FontWeight.w600),
                  ),
                ),
                Text(tr('вернуться', 'return'), style: TextStyle(color: Colors.white)),
                const Icon(Icons.chevron_right, color: Colors.white),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
