# Build Mini Player: the app (dist\MiniPlayer.exe) and the installer (installer\dist\MiniPlayer_Setup.exe).
# Requires Windows, Python 3.12+ and:  pip install -r requirements.txt
$ErrorActionPreference = 'Continue'   # PyInstaller writes progress to stderr; failures are caught via exit codes
Set-Location $PSScriptRoot

python build_ui.py                                    # ui.html from the template + icons
if ($LASTEXITCODE -ne 0) { throw 'build_ui.py failed' }

python -m PyInstaller --noconfirm --clean --onedir --noconsole --name MiniPlayer `
    --icon icons\app.ico --add-data "ui.html;." --add-data "player.html;." --add-data "panel.html;." `
    --add-data "tiktok_inject.js;." --add-data "icons;icons" --add-data "third_party\zapret;third_party\zapret" --collect-all webview miniplayer.py
if ($LASTEXITCODE -ne 0) { throw 'app build failed' }

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
