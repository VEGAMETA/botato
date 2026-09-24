import 'package:flutter/material.dart';

/// Palette taken from the app icon (the blue potato).
abstract final class BotatoColors {
  static const potato = Color(0xFF2EB3DC); // body
  static const potatoShade = Color(0xFF1D7F9C); // shading
  static const night = Color(0xFF0E1A22); // outlines, dark background
  static const surface = Color(0xFF16242E);
  static const surfaceHigh = Color(0xFF1E303C);
  static const ember = Color(0xFF7C3AED); // cigarette tip: calls, accents
  static const smoke = Color(0xFF8FA9B6); // secondary text
  static const danger = Color(0xFFE5484D);
  static const ok = Color(0xFF3DD68C);

  /// Stable avatar colors picked by name.
  static const avatars = [potato, ember, Color(0xFFE0A33A), Color(0xFF3DD68C), Color(0xFFE5709B), Color(0xFF5B8DEF)];

  static Color avatarFor(String name) => avatars[name.codeUnits.fold(0, (a, c) => a + c) % avatars.length];
}

ThemeData botatoTheme(Brightness brightness) {
  final dark = brightness == Brightness.dark;
  final scheme = ColorScheme.fromSeed(
    seedColor: BotatoColors.potato,
    brightness: brightness,
    primary: dark ? BotatoColors.potato : BotatoColors.potatoShade,
    tertiary: BotatoColors.ember,
    error: BotatoColors.danger,
    surface: dark ? BotatoColors.night : null,
    surfaceContainer: dark ? BotatoColors.surface : null,
    surfaceContainerHigh: dark ? BotatoColors.surfaceHigh : null,
  );
  return ThemeData(
    colorScheme: scheme,
    scaffoldBackgroundColor: scheme.surface,
    appBarTheme: AppBarTheme(backgroundColor: scheme.surface, elevation: 0, scrolledUnderElevation: 0),
    cardTheme: CardThemeData(
      color: scheme.surfaceContainer,
      elevation: 0,
      margin: const EdgeInsets.symmetric(horizontal: 12, vertical: 5),
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(18)),
    ),
    snackBarTheme: const SnackBarThemeData(behavior: SnackBarBehavior.floating),
    bottomSheetTheme: BottomSheetThemeData(backgroundColor: scheme.surfaceContainer, showDragHandle: true),
  );
}

/// Round avatar with initials (no photos: nothing to leak, nothing to sync).
class Avatar extends StatelessWidget {
  const Avatar(this.name, {super.key, this.size = 44});

  final String name;
  final double size;

  @override
  Widget build(BuildContext context) {
    final color = BotatoColors.avatarFor(name);
    final letter = name.isEmpty ? '?' : name.characters.first.toUpperCase();
    return Container(
      width: size,
      height: size,
      alignment: Alignment.center,
      decoration: BoxDecoration(shape: BoxShape.circle, color: color.withValues(alpha: 0.22)),
      child: Text(
        letter,
        style: TextStyle(color: color, fontSize: size * 0.42, fontWeight: FontWeight.w600),
      ),
    );
  }
}
