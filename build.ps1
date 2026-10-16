# Build Mini Player: the Rust app (dist\MiniPlayer\) and the installer (installer\dist\MiniPlayer_Setup.exe).
# Requires Windows, Rust (rustup), Python 3.12+ (UI build and installer) and:  pip install -r requirements.txt
$ErrorActionPreference = 'Continue'   # native tools write progress to stderr; failures are caught via exit codes
Set-Location $PSScriptRoot

python build_ui.py                                    # ui.html from the template + icons
if ($LASTEXITCODE -ne 0) { throw 'build_ui.py failed' }
foreach ($f in 'ui.html', 'player.html', 'panel.html', 'tiktok_inject.js') { Copy-Item $f rs\assets\$f -Force }

# MSVC toolchain if Visual Studio Build Tools are present, otherwise the GNU toolchain (needs gcc, e.g. WinLibs)
if (Get-Command link.exe -ErrorAction SilentlyContinue) { $env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-msvc' } else { $env:RUSTUP_TOOLCHAIN = 'stable-x86_64-pc-windows-gnu' }
Push-Location rs
cargo build --release
$code = $LASTEXITCODE
Pop-Location
if ($code -ne 0) { throw 'app build failed' }

# app folder: exe + WebView2 loader + zapret files for the bypass mode
Remove-Item dist\MiniPlayer -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force dist\MiniPlayer | Out-Null
Copy-Item rs\target\release\MiniPlayer.exe dist\MiniPlayer\ -Force
Copy-Item rs\vendor\WebView2Loader.dll dist\MiniPlayer\ -Force
New-Item -ItemType Directory -Force dist\MiniPlayer\third_party | Out-Null
Copy-Item third_party\zapret dist\MiniPlayer\third_party\ -Recurse -Force

New-Item -ItemType Directory -Force installer\payload | Out-Null
Remove-Item installer\payload\* -Force -Recurse -ErrorAction SilentlyContinue
Compress-Archive -Path dist\MiniPlayer\* -DestinationPath installer\payload\app.zip -CompressionLevel Optimal -Force
Copy-Item icons\app.ico installer\app.ico -Force

Push-Location installer
python -m PyInstaller --noconfirm --clean --onefile --noconsole --name MiniPlayer_Setup --icon app.ico `
    --add-data "payload\app.zip;payload" --add-data "app.ico;." installer.py
$code = $LASTEXITCODE
Pop-Location
if ($code -ne 0) { throw 'installer build failed' }

Write-Host 'Done:'
Write-Host '  app:        dist\MiniPlayer\MiniPlayer.exe'
Write-Host '  installer:  installer\dist\MiniPlayer_Setup.exe'
