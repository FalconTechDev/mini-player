<div align="center">

<img src="docs/banner.png" alt="Mini Player" width="100%">

**Floating video windows for Windows - no browser needed.**
Paste a YouTube, TikTok, Twitch or Vimeo link (or an mp4) and watch it in a small always-on-top window
with control panels that slide out *beside* the video instead of covering it.

![Windows](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D4?style=for-the-badge&logo=windows&logoColor=white)
![Python](https://img.shields.io/badge/Python-3.12%2B-FFD43B?style=for-the-badge&logo=python&logoColor=black)
![WebView2](https://img.shields.io/badge/WebView2-pywebview-35E8E0?style=for-the-badge&logoColor=black)
![UI](https://img.shields.io/badge/UI%20language-Russian-FF4B3A?style=for-the-badge)

**English** · [Русский](README.ru.md)

[**Download the installer**](../../releases/latest) · [Build from source](#build-from-source) · [How it works](#how-it-works)

</div>

---

## Highlights

| | |
|---|---|
| **Mini windows** | Open a link and get a compact player: YouTube (videos, Shorts, playlists), TikTok, Twitch channels, Vimeo, direct mp4/webm files, or any website. |
| **Panels outside the window** | Controls slide out above, below and beside the window on hover - never over the video. Drag the window by any panel. |
| **Fine-grained transparency** | Window, frame and video opacity are separate. Pin on top, minimize, full screen, S/M/L sizes, remembered position and size. |
| **TikTok, decluttered** | Full feed in a slim window with the extra UI removed; hide captions and author names, skip ads and LIVE, auto-scroll, fit the window to each video. |
| **Real search** | Search YouTube and TikTok right from the app: best match preview plus a grid of results with thumbnails. |
| **Continue watching** | Remembers where you stopped (YouTube and local files) and resumes from that second. |
| **Accounts** | Sign in on YouTube, TikTok or Twitch; keep up to four accounts per platform and switch between them. |
| **History, pins, favorites** | Pin or delete recent items, star favorites, clear history without losing what matters. |
| **Keeps playing** | Video and audio continue when the window is minimized. |
| **Real installer** | One `Setup.exe`: shortcuts, uninstaller, optional autostart, and the WebView2 runtime if it is missing. |

## Screenshots

<table>
  <tr>
    <td width="50%"><img src="docs/main.png" alt="Main window"><br><sub><b>Main window</b> - continue watching, pinned and recent videos</sub></td>
    <td width="50%"><img src="docs/search.png" alt="Search"><br><sub><b>Search</b> - best match and results with thumbnails</sub></td>
  </tr>
  <tr>
    <td width="50%"><img src="docs/tiktok.png" alt="TikTok tab"><br><sub><b>TikTok</b> - open feeds in a slim window</sub></td>
    <td width="50%"><img src="docs/settings.png" alt="Settings"><br><sub><b>Settings</b> - layout, window options, TikTok view modes</sub></td>
  </tr>
</table>

> The interface language is currently **Russian**.

## Quick start

1. Download **`MiniPlayer_Setup.exe`** from the [latest release](../../releases/latest) and run it
   (no administrator rights needed; installs to `%LOCALAPPDATA%\Programs\Mini Player`).
2. Paste a link and press **Open** - or press **Open** with an empty field to open the platform's home feed.
3. Hover the top or bottom edge of a player window to reveal the panels.

> The installer is not code-signed, so Windows SmartScreen may warn you:
> click **More info, then Run anyway**.
> Silent install: `MiniPlayer_Setup.exe --silent` · uninstall: `uninstall.exe --uninstall`.

## How it works

```mermaid
flowchart LR
    M["Main window<br/>links · search · history"] -->|open| P["Player window<br/>video only"]
    P --- T["Top panel<br/>pin · settings · minimize · fullscreen"]
    P --- B["Bottom panel<br/>TikTok feeds"]
    P --- S["Settings window<br/>opacity · size · view modes"]
```

The app is a Python program that hosts several [pywebview](https://pywebview.flowrl.com/) (Microsoft Edge **WebView2**)
windows. The player window shows only the video; the panels are separate frameless windows that follow it around
and appear when the cursor reaches the edge. TikTok pages are cleaned with an injected script; accounts are stored as
encrypted browser sessions so you can switch between them.

## Build from source

```powershell
pip install -r requirements.txt

# run directly
python build_ui.py          # builds ui.html from the template and embeds the icons
python miniplayer.py

# or build the .exe and the installer
.\build.ps1                 # -> dist\MiniPlayer.exe  and  installer\dist\MiniPlayer_Setup.exe
```

Requires Windows 10/11 and Python 3.12+.

## Project structure

| Path | What it is |
|---|---|
| `miniplayer.py` | The program: windows, JS API, panels, accounts, search |
| `ui_template.html` | The main window (single HTML/CSS/JS file); `build_ui.py` embeds icons and writes `ui.html` |
| `player.html` | Player window page (YouTube IFrame API, Twitch chat, local files) |
| `panel.html` | Panel pages: top, bottom (TikTok feeds) and settings |
| `tiktok_inject.js` | Cleans up tiktok.com, hides ads, controls auto-scroll |
| `icons/` | Icon set (`*_clay.png` are the hand-drawn ones; `svg/` holds the sources of the rest) |
| `installer/installer.py` | Installer and uninstaller (tkinter) |
| `build.ps1` | One-command build |
| `docs/` | Banner and screenshots |

## Privacy and data

Everything is stored locally in `%APPDATA%\MiniPlayer`: settings and history (`data.json`), the embedded browser profile
(`web`) and a crash log (`crash.log`). Saved sessions of multiple accounts live in `accounts.bin`, **encrypted with
Windows DPAPI - only your Windows user can decrypt it**. Never share that file.
The app talks only to the sites you open (YouTube, TikTok, Twitch, …); it has no telemetry.

## Notes

- Not affiliated with YouTube, TikTok, Twitch or Vimeo; all names belong to their owners.
- Hiding TikTok interface elements, reading account names and TikTok search rely on how those sites are built and may
  break when they change.
- Videos whose authors disabled embedding can't be played inside the app (use **Open in browser** from the window settings).
