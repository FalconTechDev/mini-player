"""Слой платформы: всё, что зависит от ОС и движка окон (позиция и размер окон, прозрачность, cookie, шифрование).
Windows использует WebView2 и Win32; Linux и macOS (а также Windows с MP_QT=1) используют Qt WebEngine."""
import os
import sys

USE_QT = sys.platform != "win32" or os.environ.get("MP_QT") == "1"

if USE_QT:
    from .qtp import QtPlatform as _Platform
else:
    from .win32 import Win32Platform as _Platform

P = _Platform()
