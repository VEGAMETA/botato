import 'dart:ui';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'src/rust/api/client.dart' as core;

/// UI language: Russian or English, the system one by default (profile → Язык / Language).
enum AppLanguage { system, ru, en }

/// The current UI language. Strings are written in place as `tr('русский', 'English')`: many
/// live outside widgets (call service, notifications), where a BuildContext isn't at hand.
class Lang extends ChangeNotifier {
  Lang._();

  static final instance = Lang._();
  static const _key = 'language';
  static const _channel = MethodChannel('botato/call');

  AppLanguage setting = AppLanguage.system;
  bool english = false;

  Locale get locale => Locale(english ? 'en' : 'ru');

  Future<void> load() async {
    final saved = (await SharedPreferences.getInstance()).getString(_key);
    setting = AppLanguage.values.where((l) => l.name == saved).firstOrNull ?? AppLanguage.system;
    _resolve();
  }

  Future<void> set(AppLanguage l) async {
    setting = l;
    await (await SharedPreferences.getInstance()).setString(_key, l.name);
    _resolve();
    notifyListeners();
  }

  void _resolve() {
    english = switch (setting) {
      AppLanguage.ru => false,
      AppLanguage.en => true,
      AppLanguage.system => PlatformDispatcher.instance.locale.languageCode != 'ru',
    };
    pushToNative();
  }

  /// Rust (errors, message previews) and Android (notifications) speak the same language.
  void pushToNative() {
    try {
      core.clientSetEnglish(english: english);
    } catch (_) {
      // the Rust library isn't loaded yet (called again after init)
    }
    if (defaultTargetPlatform == TargetPlatform.android) {
      _channel.invokeMethod('language', english).catchError((_) => null);
    }
  }
}

/// The string for the current UI language.
String tr(String ru, String en) => Lang.instance.english ? en : ru;
