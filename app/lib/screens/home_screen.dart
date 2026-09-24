import 'dart:async';

import 'package:flutter/material.dart';

import '../call/call_service.dart';
import '../relay_service.dart';
import '../src/rust/api/client.dart' as core;
import '../src/rust/api/identity.dart' as id;

class HomeScreen extends StatefulWidget {
  const HomeScreen({super.key, required this.info, required this.relay, required this.calls});

  final id.CertInfo info;
  final RelayService relay;
  final CallService calls;

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  StreamSubscription<IncomingSignal>? _sub;
  final _pingSent = <String, DateTime>{};

  @override
  void initState() {
    super.initState();
    _sub = widget.relay.signals.listen(_onSignal);
  }

  @override
  void dispose() {
    _sub?.cancel();
    super.dispose();
  }

  void _onSignal(IncomingSignal s) {
    final who = widget.relay.contactOfDevice(s.fromDevice)?.name ?? s.fromDevice;
    switch (s.data['type']) {
      case 'ping':
        widget.relay.signal(s.fromDevice, {'type': 'pong'});
        _toast('ping от $who');
      case 'pong':
        final sent = _pingSent.remove(s.fromDevice);
        final ms = sent == null ? '?' : DateTime.now().difference(sent).inMilliseconds;
        _toast('pong от $who: $ms мс');
    }
  }

  void _toast(String text) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
  }

  void _ping(core.ContactInfo c) {
    for (final d in c.devices) {
      _pingSent[d] = DateTime.now();
      widget.relay.signal(d, {'type': 'ping'});
    }
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: widget.relay,
      builder: (context, _) {
        final r = widget.relay;
        final (color, label) = switch (r.state) {
          ConnState.online => (Colors.green, 'в сети'),
          ConnState.connecting => (Colors.amber, 'подключение…'),
          ConnState.offline => (Colors.red, 'нет связи'),
        };
        final others = r.contacts.where((c) => !c.isMe).toList();
        return Scaffold(
          appBar: AppBar(
            title: Text(widget.info.name),
            actions: [IconButton(icon: const Icon(Icons.refresh), onPressed: r.refreshContacts)],
          ),
          body: ListView(
            children: [
              ListTile(
                leading: Icon(Icons.circle, color: color, size: 14),
                title: Text(label),
                subtitle: Text(r.detail, maxLines: 2, overflow: TextOverflow.ellipsis),
              ),
              const Divider(),
              if (others.isEmpty)
                const ListTile(
                  title: Text('Контактов пока нет'),
                  subtitle: Text('Они появятся, когда подключатся к серверу'),
                ),
              for (final c in others)
                ListTile(
                  leading: CircleAvatar(child: Text(c.name.isEmpty ? '?' : c.name[0].toUpperCase())),
                  title: Text(c.name),
                  subtitle: Text('устройств: ${c.devices.length}'),
                  trailing: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      IconButton(
                        tooltip: 'Проверить связь',
                        icon: const Icon(Icons.network_ping),
                        onPressed: r.state == ConnState.online ? () => _ping(c) : null,
                      ),
                      IconButton(
                        tooltip: 'Позвонить',
                        icon: const Icon(Icons.call),
                        onPressed: r.state == ConnState.online ? () => widget.calls.startCall(c) : null,
                      ),
                      IconButton(
                        tooltip: 'Видеозвонок',
                        icon: const Icon(Icons.videocam),
                        onPressed: r.state == ConnState.online ? () => widget.calls.startCall(c, video: true) : null,
                      ),
                    ],
                  ),
                ),
            ],
          ),
        );
      },
    );
  }
}
