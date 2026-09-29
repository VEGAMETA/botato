import 'dart:ffi';

import 'package:ffi/ffi.dart';

/// Windows privacy switches that deny the microphone (Settings → Privacy → Microphone). A denied
/// desktop app still opens the mic and records zeros, so this is the only way to tell. Read
/// straight from the registry (advapi32), no child process.
List<String> windowsMicDenied() {
  const store = r'Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone';
  return [
    if (_denied(_hklm, store)) 'this device',
    if (_denied(_hkcu, store)) 'apps',
    if (_denied(_hkcu, '$store\\NonPackaged')) 'desktop apps',
  ];
}

const _hkcu = 0x80000001;
const _hklm = 0x80000002;
const _rrfRtRegSz = 0x2;

typedef _RegGetValueNative = Int32 Function(
  IntPtr,
  Pointer<Utf16>,
  Pointer<Utf16>,
  Uint32,
  Pointer<Uint32>,
  Pointer<Void>,
  Pointer<Uint32>,
);
typedef _RegGetValue = int Function(
  int,
  Pointer<Utf16>,
  Pointer<Utf16>,
  int,
  Pointer<Uint32>,
  Pointer<Void>,
  Pointer<Uint32>,
);

final _regGetValue = DynamicLibrary.open('advapi32.dll')
    .lookupFunction<_RegGetValueNative, _RegGetValue>('RegGetValueW');

bool _denied(int root, String key) {
  return using((arena) {
    const cap = 64;
    final data = arena<Uint16>(cap);
    final size = arena<Uint32>()..value = cap * 2;
    final r = _regGetValue(
      root,
      key.toNativeUtf16(allocator: arena),
      'Value'.toNativeUtf16(allocator: arena),
      _rrfRtRegSz,
      nullptr,
      data.cast(),
      size,
    );
    return r == 0 && data.cast<Utf16>().toDartString() == 'Deny';
  });
}
