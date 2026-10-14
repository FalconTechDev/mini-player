"""Платформа Qt (Linux, macOS; на Windows включается переменной MP_QT=1): окна pywebview на Qt WebEngine."""
import calendar
import os as _os
_os.environ.setdefault("QT_API", "pyqt6")
import json
import os
import re
import sys
import threading
import time
from urllib.parse import urlparse

from qtpy import QtCore, QtGui, QtWidgets
from qtpy.QtCore import QObject, Qt, QUrl, Signal, Slot
from qtpy.QtGui import QCursor, QIcon
from qtpy.QtNetwork import QNetworkCookie
from qtpy.QtWidgets import QApplication

KEEPALIVE_JS = """(()=>{try{
 Object.defineProperty(Document.prototype,'hidden',{get:()=>false,configurable:true});
 Object.defineProperty(Document.prototype,'visibilityState',{get:()=>'visible',configurable:true});
 Object.defineProperty(Document.prototype,'webkitHidden',{get:()=>false,configurable:true});
 Object.defineProperty(Document.prototype,'webkitVisibilityState',{get:()=>'visible',configurable:true});
 ['visibilitychange','webkitvisibilitychange'].forEach(t=>document.addEventListener(t,e=>e.stopImmediatePropagation(),true));
}catch(e){}})();"""

SAMESITE = {"none": 0, "lax": 1, "strict": 2}


class _Invoker(QObject):
    """Живёт в главном потоке и выполняет присланные из других потоков функции."""
    call_sig = Signal(object)

    def __init__(self):
        super().__init__()
        self.call_sig.connect(self._run, Qt.ConnectionType.QueuedConnection)

    @Slot(object)
    def _run(self, job):
        job()


class _Grip(QtWidgets.QWidget):
    """Невидимая полоска по краю окна без рамки: тянешь - меняется размер."""

    def __init__(self, parent, edges, cursor):
        super().__init__(parent)
        self.edges = edges
        self.setCursor(cursor)
        self.setMouseTracking(True)

    def mousePressEvent(self, e):
        if e.button() == Qt.MouseButton.LeftButton:
            h = self.window().windowHandle()
            if h is not None:
                h.startSystemResize(self.edges)


class QtPlatform:
    name = "qt"
    frameless_players = True         # окна плеера создаются без системной рамки, размер меняется полосками по краям
    panel_resizable = True
    gui = "qt"

    def __init__(self):
        self.main_win = None
        self._inv = _Invoker()                    # создаётся в главном потоке до запуска цикла Qt
        self._profile = None
        self._mirror = {}
        self._mlock = threading.Lock()
        self._timers = []
        self._grips = {}
        self._install_profile_hook()

    # ---------- окружение ----------
    def data_dir(self):
        if sys.platform == "darwin":
            return os.path.join(os.path.expanduser("~"), "Library", "Application Support", "MiniPlayer")
        base = os.environ.get("XDG_DATA_HOME") or os.path.join(os.path.expanduser("~"), ".local", "share")
        if sys.platform == "win32":
            base = os.environ.get("APPDATA", base)
        return os.path.join(base, "MiniPlayer")

    def prepare_env(self):
        flags = ["--autoplay-policy=no-user-gesture-required", "--disable-renderer-backgrounding",
                 "--disable-background-timer-throttling", "--disable-backgrounding-occluded-windows"]
        if hasattr(os, "geteuid") and os.geteuid() == 0:        # root (например в контейнере): песочница Chromium не запустится
            flags.append("--no-sandbox")
        cur = os.environ.get("QTWEBENGINE_CHROMIUM_FLAGS", "")
        os.environ["QTWEBENGINE_CHROMIUM_FLAGS"] = (cur + " " + " ".join(flags)).strip()
        if sys.platform.startswith("linux") and os.environ.get("WAYLAND_DISPLAY") and os.environ.get("DISPLAY"):
            os.environ.setdefault("QT_QPA_PLATFORM", "xcb")      # на Wayland окна нельзя ставить в заданное место - берём XWayland

    def start_kwargs(self, storage_path):
        return dict(gui="qt", private_mode=False, storage_path=storage_path)

    def set_main(self, win):
        self.main_win = win

    # ---------- общий профиль браузера для всех окон ----------
    def _install_profile_hook(self):
        import webview.platforms.qt as qtmod
        orig = qtmod.QWebEngineProfile
        plat = self

        def factory(*a, **k):
            if not a and not k:
                return orig()                           # приватный режим: отдельный профиль
            if plat._profile is None:
                plat._profile = orig(*a, **k)
                plat._on_profile(plat._profile)
            return plat._profile                        # все окна делят один профиль: cookie общие
        qtmod.QWebEngineProfile = factory

    def _on_profile(self, p):
        try:
            ua = p.httpUserAgent()
            p.setHttpUserAgent(re.sub(r"\s*(QtWebEngine|HeadlessChrome)/[\d.]+", "", ua))
        except Exception:
            pass
        try:
            from qtpy.QtWebEngineCore import QWebEngineScript
            s = QWebEngineScript()
            s.setName("mp-keepalive")
            s.setSourceCode(KEEPALIVE_JS)
            s.setInjectionPoint(QWebEngineScript.InjectionPoint.DocumentCreation)
            s.setWorldId(QWebEngineScript.ScriptWorldId.MainWorld)
            s.setRunsOnSubFrames(True)
            p.scripts().insert(s)
        except Exception:
            pass
        store = p.cookieStore()
        store.cookieAdded.connect(self._cookie_added)
        store.cookieRemoved.connect(self._cookie_removed)
        store.loadAllCookies()

    def _cookie_key(self, c):
        return (bytes(c.name()).decode("utf-8", "ignore"), c.domain(), c.path())

    def _cookie_added(self, c):
        with self._mlock:
            self._mirror[self._cookie_key(c)] = QNetworkCookie(c)

    def _cookie_removed(self, c):
        with self._mlock:
            self._mirror.pop(self._cookie_key(c), None)

    # ---------- работа в потоке интерфейса ----------
    def ui(self, fn, timeout=6):
        if threading.current_thread() is threading.main_thread():
            return fn()
        done = threading.Event()
        box = [None, None]

        def job():
            try:
                box[0] = fn()
            except Exception as e:
                box[1] = e
            finally:
                done.set()
        self._inv.call_sig.emit(job)
        if not done.wait(timeout):
            raise TimeoutError("поток интерфейса занят")
        if box[1] is not None:
            raise box[1]
        return box[0]

    def ui_async(self, fn):
        if threading.current_thread() is threading.main_thread():
            fn()
        else:
            self._inv.call_sig.emit(fn)

    def _n(self, win):
        n = win.native
        if n is None:
            raise RuntimeError("окно ещё не создано")
        return n

    # ---------- геометрия ----------
    def rect(self, win):
        def f():
            g = self._n(win).frameGeometry()
            return g.x(), g.y(), g.width(), g.height()
        return self.ui(f)

    def place(self, win, x=None, y=None, w=None, h=None):
        def f():
            n = self._n(win)
            fg, g = n.frameGeometry(), n.geometry()
            dx, dy = fg.width() - g.width(), fg.height() - g.height()
            if x is not None:
                n.move(int(x), int(y))
            if w is not None:
                n.resize(max(1, int(w) - dx), max(1, int(h) - dy))
        self.ui(f)

    def work_area(self, win, full=False):
        def f():
            wh = self._n(win).windowHandle()                    # у pywebview атрибут .screen перекрывает метод QWidget.screen()
            scr = (wh.screen() if wh is not None else None) or QApplication.primaryScreen()
            g = scr.geometry() if full else scr.availableGeometry()
            return g.x(), g.y(), g.width(), g.height()
        return self.ui(f)

    def client_rect(self, win):
        def f():
            g = self._n(win).geometry()
            return g.x(), g.y(), g.width(), g.height()
        return self.ui(f)

    def align_client(self, win, target):
        def f():
            n = self._n(win)
            fg, g = n.frameGeometry(), n.geometry()
            tx, ty, tw, th = target
            n.move(int(tx - (g.x() - fg.x())), int(ty - (g.y() - fg.y())))
            n.resize(int(tw), int(th))
        self.ui(f)

    # ---------- состояние окна ----------
    def topmost(self, win, on):
        def f():
            n = self._n(win)
            flag = Qt.WindowType.WindowStaysOnTopHint
            if bool(n.windowFlags() & flag) == bool(on):
                return
            vis = n.isVisible()
            n.setWindowFlag(flag, bool(on))
            if vis:
                n.show()
        self.ui(f)

    def minimize(self, win):
        self.ui_async(lambda: self._n(win).showMinimized())

    def restore(self, win):
        self.ui_async(lambda: self._n(win).showNormal())

    def close(self, win):
        self.ui_async(lambda: self._n(win).close())

    def is_alive(self, win):
        return not getattr(win, "_mp_closed", False) and win.native is not None

    def is_topmost(self, win):
        return bool(self.ui(lambda: self._n(win).windowFlags() & Qt.WindowType.WindowStaysOnTopHint))

    def is_minimized(self, win):
        return bool(self.ui(lambda: self._n(win).isMinimized()))

    def is_visible(self, win):
        return bool(self.ui(lambda: self._n(win).isVisible()))

    def set_opacity(self, win, value):
        try:
            v = max(0.3, min(1.0, float(value)))
            self.ui(lambda: self._n(win).setWindowOpacity(v))
        except Exception:
            pass

    def prepare_player(self, win):
        """При показе окна плеера/TikTok (окно без рамки): полоски по краям для изменения размера."""
        def f():
            n = self._n(win)
            if id(n) in self._grips or not (n.windowFlags() & Qt.WindowType.FramelessWindowHint):
                return
            E, C = Qt.Edge, Qt.CursorShape
            spec = [(E.LeftEdge, C.SizeHorCursor), (E.RightEdge, C.SizeHorCursor), (E.TopEdge, C.SizeVerCursor),
                    (E.BottomEdge, C.SizeVerCursor), (E.LeftEdge | E.TopEdge, C.SizeFDiagCursor),
                    (E.RightEdge | E.BottomEdge, C.SizeFDiagCursor), (E.RightEdge | E.TopEdge, C.SizeBDiagCursor),
                    (E.LeftEdge | E.BottomEdge, C.SizeBDiagCursor)]
            grips = [(_Grip(n, e, c), e) for e, c in spec]
            self._grips[id(n)] = grips

            def layout():
                w, h, t, k = n.width(), n.height(), 5, 12
                for g, e in grips:
                    l, r = bool(e & E.LeftEdge), bool(e & E.RightEdge)
                    tp, bt = bool(e & E.TopEdge), bool(e & E.BottomEdge)
                    if (l or r) and (tp or bt):
                        g.setGeometry(0 if l else w - k, 0 if tp else h - k, k, k)
                    elif l or r:
                        g.setGeometry(0 if l else w - t, k, t, max(1, h - 2 * k))
                    else:
                        g.setGeometry(k, 0 if tp else h - t, max(1, w - 2 * k), t)
                    g.raise_()
                    g.show()

            class _Filter(QObject):
                def eventFilter(self, obj, ev):
                    if ev.type() == QtCore.QEvent.Type.Resize:
                        layout()
                    return False
            flt = _Filter(n)
            n.installEventFilter(flt)
            n._mp_filter = flt
            layout()
        try:
            self.ui(f)
        except Exception:
            pass

    def set_icon(self, win, name, resource):
        def apply():
            try:
                path = resource(f"icons/{name}.png")
                self.ui_async(lambda: self._n(win).setWindowIcon(QIcon(path)))
            except Exception:
                pass
        win.events.shown += apply

    def keepalive(self, win):
        """Скрипт «окно всегда видно» вставляется в общий профиль браузера один раз (см. _on_profile)."""
        return

    # ---------- мышь и перетаскивание ----------
    def cursor(self):
        def f():
            p = QCursor.pos()
            return p.x(), p.y()
        return self.ui(f)

    def at_cursor(self, candidates):
        def f():
            wdg = QApplication.widgetAt(QCursor.pos())
            top = wdg.window() if wdg is not None else None
            for w in candidates:
                try:
                    if w.native is not None and w.native is top:
                        return w
                except Exception:
                    continue
            return None
        return self.ui(f)

    def drag(self, win, on_move=None):
        """Двигает окно средствами системы (пока нажата кнопка мыши). Панели подтягиваются по событию перемещения окна."""
        def f():
            h = self._n(win).windowHandle()
            if h is not None:
                h.startSystemMove()
        self.ui(f)

    # ---------- панели ----------
    def panel_setup(self, win, parent):
        def f():
            n = self._n(win)
            n.setWindowFlag(Qt.WindowType.Tool, True)
            n.setAttribute(Qt.WidgetAttribute.WA_ShowWithoutActivating, True)
            n.setFocusPolicy(Qt.FocusPolicy.NoFocus)
            try:
                n.setAttribute(Qt.WidgetAttribute.WA_MacAlwaysShowToolWindow, True)
            except Exception:
                pass
            n.hide()
        self.ui(f)

    def panel_resolve_handle(self, win):
        for _ in range(60):
            if win.native is not None:
                return True
            time.sleep(0.1)
        return False

    def panel_place(self, win, rect):
        x, y, w, h = rect
        self.ui(lambda: self._n(win).setGeometry(int(x), int(y), int(w), int(h)))

    def panel_z(self, win, pinned, reset):
        def f():
            n = self._n(win)
            flag = Qt.WindowType.WindowStaysOnTopHint
            vis = n.isVisible()
            if bool(n.windowFlags() & flag) != bool(pinned):
                n.setWindowFlag(flag, bool(pinned))
                if vis:
                    n.show()
            if vis:
                n.raise_()
        self.ui(f)

    def panel_visible(self, win, on):
        def f():
            n = self._n(win)
            if on:
                n.show()
                n.raise_()
            else:
                n.hide()
        self.ui(f)

    def panel_close(self, win):
        self.close(win)

    def schedule(self, fn, interval_ms):
        def f():
            t = QtCore.QTimer()
            t.timeout.connect(fn)
            t.start(int(interval_ms))
            self._timers.append(t)
        self.ui(f)

    # ---------- cookie профиля ----------
    def _match(self, c, site):
        u = urlparse(site)
        host = (u.hostname or "").lower()
        dom = c.domain().lower()
        if dom.startswith("."):
            ok = host == dom[1:] or host.endswith(dom)
        else:
            ok = host == dom
        if not ok:
            return False
        if c.isSecure() and u.scheme != "https":
            return False
        return (u.path or "/").startswith(c.path() or "/")

    def _dump_one(self, c):
        ss = 1
        try:
            name = c.sameSitePolicy().name.lower().strip("_")
            ss = SAMESITE.get(name, 1)
        except Exception:
            pass
        exp = None
        if not c.isSessionCookie():
            exp = c.expirationDate().toUTC().toString(Qt.DateFormat.ISODate)
        return dict(name=bytes(c.name()).decode("utf-8", "ignore"), value=bytes(c.value()).decode("utf-8", "ignore"),
                    domain=c.domain(), path=c.path(), expires=exp, http=bool(c.isHttpOnly()),
                    secure=bool(c.isSecure()), same=ss)

    def cookies(self, sites, delete=False):
        """Полные cookie для заданных сайтов (list[dict]). С delete=True удаляет их из профиля."""
        if self._profile is None:
            raise RuntimeError("браузер ещё не запущен")
        with self._mlock:
            found = [c for c in self._mirror.values() if any(self._match(c, s) for s in sites)]
        rows = {}
        for c in found:
            d = self._dump_one(c)
            rows[(d["name"], d["domain"], d["path"])] = d
        if delete and found:
            def f():
                store = self._profile.cookieStore()
                for c in found:
                    store.deleteCookie(c, QUrl(("https://" if c.isSecure() else "http://") + c.domain().lstrip(".")))
            self.ui(f)
        return list(rows.values())

    def cookie_restore(self, items):
        def f():
            store = self._profile.cookieStore()
            for it in items:
                c = QNetworkCookie(it["name"].encode(), it["value"].encode())
                c.setDomain(it["domain"])
                c.setPath(it["path"])
                c.setSecure(bool(it.get("secure")))
                c.setHttpOnly(bool(it.get("http")))
                if it.get("expires"):
                    try:
                        m = re.match(r"(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})", it["expires"])
                        secs = calendar.timegm(tuple(int(x) for x in m.groups()) + (0, 0, 0))
                        c.setExpirationDate(QtCore.QDateTime.fromSecsSinceEpoch(secs))
                    except Exception:
                        pass
                try:
                    pol = {0: "None_", 1: "Lax", 2: "Strict"}.get(int(it.get("same", 1)), "Lax")
                    c.setSameSitePolicy(getattr(QNetworkCookie.SameSite, pol))
                except Exception:
                    pass
                store.setCookie(c, QUrl(("https://" if it.get("secure") else "http://") + it["domain"].lstrip(".")))
        self.ui(f, timeout=20)

    # ---------- шифрование сохранённых входов ----------
    def _fernet_key(self):
        try:
            import keyring
            k = keyring.get_password("MiniPlayer", "accounts-key")
            if not k:
                from cryptography.fernet import Fernet
                k = Fernet.generate_key().decode()
                keyring.set_password("MiniPlayer", "accounts-key", k)
            return k.encode()
        except Exception:
            # нет хранилища ключей системы (например, нет сервиса секретов): ключ в файле, доступном только вам
            from cryptography.fernet import Fernet
            os.makedirs(self.data_dir(), exist_ok=True)
            path = os.path.join(self.data_dir(), ".key")
            if os.path.exists(path):
                return open(path, "rb").read()
            k = Fernet.generate_key()
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, "wb") as f:
                f.write(k)
            return k

    def protect(self, data):
        from cryptography.fernet import Fernet
        return Fernet(self._fernet_key()).encrypt(data)

    def unprotect(self, data):
        from cryptography.fernet import Fernet
        return Fernet(self._fernet_key()).decrypt(data)

    # ---------- прочее ----------
    def clipboard(self):
        try:
            return self.ui(lambda: QApplication.clipboard().text())
        except Exception:
            return ""

    def start_watchdog(self, main_win, log):
        return
