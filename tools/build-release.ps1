# Builds a release of the app: Android APK (arm64) and the Windows installer.
# Version comes from app/pubspec.yaml (bump `version: x.y.z+build` first; build must grow).
# Then sign and publish it from botatoadmin (release section).
#
#   powershell -ExecutionPolicy Bypass -File tools\build-release.ps1

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$app = Join-Path $repo 'app'
$adminDir = if ($env:BOTATO_ADMIN_DIR) { $env:BOTATO_ADMIN_DIR } else { Join-Path $env:USERPROFILE 'botato-admin' }
$flutter = if ($env:FLUTTER) { $env:FLUTTER } else { 'D:\Utils\flutter\bin\flutter.bat' }
$iscc = Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'

$running = Get-Process botato -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$app\build\*" }
if ($running) { throw "botato is running from the build folder ($($running[0].Path)); quit it (tray menu, last item) and retry" }

$line = Select-String -Path (Join-Path $app 'pubspec.yaml') -Pattern '^version:\s*(\S+)\+(\d+)' | Select-Object -First 1
if (-not $line) { throw 'pubspec.yaml: version must be x.y.z+build' }
$version = $line.Matches[0].Groups[1].Value
$build = $line.Matches[0].Groups[2].Value
Write-Host "botato $version (build $build)"

# The built-in manifest: the newest one the admin signed.
Copy-Item (Join-Path $adminDir 'manifest.bin') (Join-Path $app 'assets\manifest.bin') -Force

Push-Location $app
try {
    # Two APKs: arm64, and 32-bit ARM for phones with a 32-bit OS (e.g. Galaxy A40).
    & $flutter build apk --release --split-per-abi --target-platform android-arm,android-arm64
    if ($LASTEXITCODE -ne 0) { throw 'APK build failed' }
    & $flutter build windows --release
    if ($LASTEXITCODE -ne 0) { throw 'Windows build failed' }
    & $iscc /Q "/DAppVersion=$version" windows\installer\botato.iss
    if ($LASTEXITCODE -ne 0) { throw 'installer build failed' }
} finally {
    Pop-Location
}

$apk = Join-Path $app 'build\app\outputs\flutter-apk\app-arm64-v8a-release.apk'
$apk32 = Join-Path $app 'build\app\outputs\flutter-apk\app-armeabi-v7a-release.apk'
$exe = Join-Path $app "build\installer\botato-setup-$version.exe"
Get-Item $apk, $apk32, $exe | ForEach-Object { '{0,-40} {1,8:N1} MB' -f $_.Name, ($_.Length / 1MB) }
Write-Host 'Next: botatoadmin -> release section -> sign and publish.'
