#!/usr/bin/env bash
# Сборка Mini Player для Linux и macOS (Qt WebEngine). Нужен Python 3.12+ и:  pip install -r requirements-qt.txt
# Результат: dist/MiniPlayer (Linux, папка) или dist/MiniPlayer.app (macOS).
set -euo pipefail
cd "$(dirname "$0")/.."
python build_ui.py
COMMON=(--noconfirm --clean --name MiniPlayer
        --add-data "ui.html:." --add-data "player.html:." --add-data "panel.html:." --add-data "tiktok_inject.js:."
        --add-data "icons:icons" --collect-all webview --collect-submodules keyring
        --hidden-import webview.platforms.qt --hidden-import qtpy)
if [[ "$(uname)" == "Darwin" ]]; then
  python -m PyInstaller "${COMMON[@]}" --windowed --icon icons/app.icns --osx-bundle-identifier io.github.falcontechdev.miniplayer miniplayer.py
  codesign --force --deep -s - dist/MiniPlayer.app || true      # подпись ad-hoc: без неё приложения Apple Silicon не запускаются
else
  python -m PyInstaller "${COMMON[@]}" --icon icons/app.png miniplayer.py
fi
