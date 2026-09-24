; botato installer (Inno Setup 6). Per-user install, no admin rights.
; Build: flutter build windows --release, then
;   ISCC.exe windows\installer\botato.iss      (from app/)
; Output: app\build\installer\botato-setup-<version>.exe

#define AppName "botato"
#define AppVersion "0.1.0"
#define Release "..\..\build\windows\x64\runner\Release"

[Setup]
; Fixed id: upgrades replace the previous install instead of adding a second one.
AppId={{6F0B7C2E-3D1A-4B8E-9C55-B07A70B07A70}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=botato
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\build\installer
OutputBaseFilename=botato-setup-{#AppVersion}
SetupIconFile=..\runner\resources\app_icon.ico
UninstallDisplayIcon={app}\botato.exe
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
; A running botato is closed before files are replaced, and started again afterwards.
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"

[Tasks]
Name: "autostart"; Description: "Запускать при входе в Windows (свёрнутым в трей), чтобы не пропускать звонки"
Name: "desktopicon"; Description: "Значок на рабочем столе"; Flags: unchecked

[Files]
Source: "{#Release}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
; The AppUserModelID must match the one the app registers for its notifications.
Name: "{userprograms}\{#AppName}"; Filename: "{app}\botato.exe"; AppUserModelID: "botato"
Name: "{userdesktop}\{#AppName}"; Filename: "{app}\botato.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "botato"; \
  ValueData: """{app}\botato.exe"" --minimized"; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\botato.exe"; Description: "Запустить botato"; Flags: nowait postinstall skipifsilent

; Keys, certificate and history live in the user's app data and OS secure storage; the
; uninstaller leaves them, so a reinstall keeps this device enrolled.
