"""Платформа Windows: WebView2 (pywebview/winforms) и Win32 через ctypes."""
import ctypes
import os
import threading
import time
from ctypes import wintypes

KEEPALIVE_JS = """(()=>{try{
 Object.defineProperty(Document.prototype,'hidden',{get:()=>false,configurable:true});
 Object.defineProperty(Document.prototype,'visibilityState',{get:()=>'visible',configurable:true});
 Object.defineProperty(Document.prototype,'webkitHidden',{get:()=>false,configurable:true});
 Object.defineProperty(Document.prototype,'webkitVisibilityState',{get:()=>'visible',configurable:true});
 ['visibilitychange','webkitvisibilitychange'].forEach(t=>document.addEventListener(t,e=>e.stopImmediatePropagation(),true));
}catch(e){}})();"""

SWP_NOSIZE, SWP_NOMOVE, SWP_NOZORDER, SWP_NOACTIVATE, SWP_ASYNC = 0x1, 0x2, 0x4, 0x10, 0x4000


class _MONITORINFO(ctypes.Structure):
    _fields_ = [("cbSize", wintypes.DWORD), ("rcMonitor", wintypes.RECT), ("rcWork", wintypes.RECT), ("dwFlags", wintypes.DWORD)]


class _BLOB(ctypes.Structure):
    _fields_ = [("cbData", wintypes.DWORD), ("pbData", ctypes.POINTER(ctypes.c_char))]


class Win32Platform:
    name = "win32"
    frameless_players = False        # заголовок окон плеера снимается после показа (set_caption), а не при создании
    panel_resizable = False
    gui = None

    def __init__(self):
        self.main_win = None
        self.cookie_lock = threading.Lock()
        u = self.u = ctypes.windll.user32
        u.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.UINT]
        u.GetWindowLongPtrW.restype = ctypes.c_void_p
        u.GetWindowLongPtrW.argtypes = [wintypes.HWND, ctypes.c_int]
        u.SetWindowLongPtrW.argtypes = [wintypes.HWND, ctypes.c_int, ctypes.c_void_p]
        u.SetLayeredWindowAttributes.argtypes = [wintypes.HWND, wintypes.DWORD, ctypes.c_ubyte, wintypes.DWORD]
        u.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
        u.ShowWindowAsync.argtypes = [wintypes.HWND, ctypes.c_int]
        u.IsWindow.argtypes = [wintypes.HWND]
        u.GetClientRect.argtypes = [wintypes.HWND, ctypes.c_void_p]
        u.ClientToScreen.argtypes = [wintypes.HWND, ctypes.c_void_p]
        u.GetForegroundWindow.restype = wintypes.HWND
        u.IsIconic.argtypes = [wintypes.HWND]
        u.IsWindowVisible.argtypes = [wintypes.HWND]
        u.WindowFromPoint.argtypes = [wintypes.POINT]
        u.WindowFromPoint.restype = wintypes.HWND
        u.GetAncestor.argtypes = [wintypes.HWND, wintypes.UINT]
        u.GetAncestor.restype = wintypes.HWND

    # ---------- окружение ----------
    def data_dir(self):
        return os.path.join(os.environ.get("APPDATA", os.path.expanduser("~")), "MiniPlayer")

    def prepare_env(self):
        # окна не «засыпают» при сворачивании: звук и видео продолжаются
        os.environ.setdefault("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
                              "--disable-features=CalculateNativeWinOcclusion --disable-renderer-backgrounding "
                              "--disable-background-timer-throttling --disable-backgrounding-occluded-windows "
                              "--autoplay-policy=no-user-gesture-required")

    def start_kwargs(self, storage_path):
        return dict(private_mode=False, storage_path=storage_path)

    def set_main(self, win):
        self.main_win = win

    # ---------- работа в потоке интерфейса ----------
    def ui_run(self, win, fn, timeout=6):
        """Выполняет fn в потоке интерфейса окна без блокировки: BeginInvoke + ожидание с таймаутом."""
        import System
        done = threading.Event()
        out = []

        def run():
            try:
                out.append(fn())
            except Exception as e:
                out.append(e)
            finally:
                done.set()
        win.native.BeginInvoke(System.Action(run))
        if not done.wait(timeout):
            raise TimeoutError("поток интерфейса занят")
        if out and isinstance(out[0], Exception):
            raise out[0]
        return out[0] if out else None

    def hwnd(self, win):
        """Дескриптор окна (запоминается в потоке интерфейса при показе окна)."""
        h = getattr(win, "_mp_hwnd", 0)
        if not h:
            h = int(win.native.Handle.ToInt64())
            win._mp_hwnd = h
        return h

    # ---------- геометрия ----------
    def rect(self, win):
        r = wintypes.RECT()
        self.u.GetWindowRect(self.hwnd(win), ctypes.byref(r))
        return r.left, r.top, r.right - r.left, r.bottom - r.top

    def place(self, win, x=None, y=None, w=None, h=None):
        """Двигает/меняет размер окна без ожидания потока интерфейса (не блокирует)."""
        flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNC
        if x is None:
            flags |= SWP_NOMOVE
        if w is None:
            flags |= SWP_NOSIZE
        self.u.SetWindowPos(self.hwnd(win), None, x or 0, y or 0, w or 0, h or 0, flags)

    def work_area(self, win, full=False):
        """Прямоугольник монитора, на котором окно: (x, y, ширина, высота)."""
        mi = _MONITORINFO()
        mi.cbSize = ctypes.sizeof(mi)
        self.u.GetMonitorInfoW(self.u.MonitorFromWindow(self.hwnd(win), 2), ctypes.byref(mi))
        r = mi.rcMonitor if full else mi.rcWork
        return r.left, r.top, r.right - r.left, r.bottom - r.top

    def client_rect(self, win):
        """Область содержимого окна (без заголовка и рамок) в координатах экрана: (x, y, ширина, высота)."""
        h = self.hwnd(win)
        rc = wintypes.RECT()
        self.u.GetClientRect(h, ctypes.byref(rc))
        pt = wintypes.POINT(0, 0)
        self.u.ClientToScreen(h, ctypes.byref(pt))
        return pt.x, pt.y, rc.right - rc.left, rc.bottom - rc.top

    def align_client(self, win, target):
        """Двигает окно так, чтобы его содержимое совпало с target = (x, y, ширина, высота) на экране."""
        ox, oy, ow, oh = self.rect(win)
        cx, cy, cw, ch = self.client_rect(win)
        tx, ty, tw, th = target
        self.place(win, tx - (cx - ox), ty - (cy - oy), tw + (ow - cw), th + (oh - ch))

    # ---------- состояние окна ----------
    def topmost(self, win, on):
        h = self.hwnd(win)
        self.u.SetWindowPos(h, wintypes.HWND(-1 if on else -2), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNC)

        def sync():                                   # состояние самой формы WinForms (в её потоке, без ожидания)
            import System
            win.native.BeginInvoke(System.Action(lambda: setattr(win.native, "TopMost", on)))
        threading.Thread(target=sync, daemon=True).start()

    def minimize(self, win):
        self.u.ShowWindowAsync(self.hwnd(win), 6)            # SW_MINIMIZE

    def close(self, win):
        self.u.PostMessageW(self.hwnd(win), 0x0010, 0, 0)    # WM_CLOSE

    def restore(self, win):
        self.u.ShowWindowAsync(self.hwnd(win), 9)            # SW_RESTORE

    def is_alive(self, win):
        try:
            return bool(self.u.IsWindow(self.hwnd(win)))
        except Exception:
            return False

    def is_topmost(self, win):
        self.u.GetWindowLongW.restype = ctypes.c_long
        return bool(self.u.GetWindowLongW(self.hwnd(win), -20) & 8)

    def is_minimized(self, win):
        return bool(self.u.IsIconic(self.hwnd(win)))

    def is_visible(self, win):
        return bool(self.u.IsWindowVisible(self.hwnd(win)))

    def set_opacity(self, win, value):
        """Прозрачность окна целиком (0.3-1.0)."""
        try:
            h = self.hwnd(win)
            v = max(0.3, min(1.0, float(value)))
            ex = self.u.GetWindowLongPtrW(h, -20) or 0
            self.u.SetWindowLongPtrW(h, -20, ex | 0x00080000)          # WS_EX_LAYERED
            self.u.SetLayeredWindowAttributes(h, 0, int(255 * v), 2)    # LWA_ALPHA
        except Exception:
            pass

    def set_caption(self, win, visible):
        """Показывает/прячет заголовок окна Windows, оставляя рамку для изменения размера."""
        try:
            h = self.hwnd(win)
            style = self.u.GetWindowLongPtrW(h, -16) or 0
            if visible:
                style |= 0x00C00000 | 0x00040000       # WS_CAPTION | WS_THICKFRAME
            else:
                style = (style & ~0x00C00000) | 0x00040000
            self.u.SetWindowLongPtrW(h, -16, style)
            self.u.SetWindowPos(h, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | 0x0020 | SWP_ASYNC)
        except Exception:
            pass

    def prepare_player(self, win):
        """Вызывается в потоке интерфейса при показе окна плеера/TikTok: запоминаем дескриптор и снимаем системный заголовок."""
        try:
            win._mp_hwnd = int(win.native.Handle.ToInt64())
        except Exception:
            pass
        self.set_caption(win, False)

    def set_icon(self, win, name, resource):
        """Свой значок в заголовке окна (для каждого режима свой) вместо значка Python."""
        def apply():
            try:
                import System
                from System.Drawing import Icon
                ico = Icon(resource(f"icons/{name}.ico"))
                win.native.Invoke(System.Action(lambda: setattr(win.native, "Icon", ico)))
            except Exception:
                pass
        win.events.shown += apply

    def keepalive(self, win):
        """Страницы (и вложенные плееры) считают, что окно всегда видно, - не ставят видео на паузу при сворачивании."""
        def run():
            for _ in range(60):
                time.sleep(0.25)
                try:
                    self.ui_run(win, lambda: win.native.webview.CoreWebView2.AddScriptToExecuteOnDocumentCreatedAsync(KEEPALIVE_JS))
                    win.evaluate_js(KEEPALIVE_JS)
                    return
                except Exception:
                    continue
        win.events.shown += lambda: threading.Thread(target=run, daemon=True).start()

    # ---------- мышь и перетаскивание ----------
    def cursor(self):
        pt = wintypes.POINT()
        self.u.GetCursorPos(ctypes.byref(pt))
        return pt.x, pt.y

    def at_cursor(self, candidates):
        """Какое из окон candidates находится под курсором (None - там чужое окно или пусто)."""
        pt = wintypes.POINT()
        self.u.GetCursorPos(ctypes.byref(pt))
        root = self.u.GetAncestor(self.u.WindowFromPoint(pt), 2)
        for w in candidates:
            try:
                if self.hwnd(w) == root:
                    return w
            except Exception:
                continue
        return None

    def drag(self, win, on_move=None):
        """Перетаскивание окна: пока зажата левая кнопка мыши, окно идёт за курсором."""
        h = self.hwnd(win)
        pt = wintypes.POINT()
        self.u.GetCursorPos(ctypes.byref(pt))
        x0, y0, _, _ = self.rect(win)
        ox, oy = pt.x - x0, pt.y - y0
        t0 = time.time()
        while self.u.GetAsyncKeyState(0x01) & 0x8000 and time.time() - t0 < 60:
            self.u.GetCursorPos(ctypes.byref(pt))
            self.place(win, pt.x - ox, pt.y - oy)
            if on_move:
                on_move()
            time.sleep(0.008)

    # ---------- панели (отдельные окна снаружи плеера) ----------
    def panel_setup(self, win, parent):
        h = self.hwnd(win)
        ex = self.u.GetWindowLongPtrW(h, -20) or 0
        self.u.SetWindowLongPtrW(h, -20, ex | 0x00000080 | 0x08000000)      # TOOLWINDOW | NOACTIVATE
        self.u.ShowWindowAsync(h, 0)

    def panel_resolve_handle(self, win):
        for _ in range(60):
            try:
                self.hwnd(win)
                return True
            except Exception:
                time.sleep(0.1)
        return False

    def panel_place(self, win, rect):
        self.place(win, *rect)

    def panel_z(self, win, pinned, reset):
        """Держим панель выше окна плеера: закреплённое - «поверх всего»; при откреплении сначала снимаем «поверх всего»."""
        h = self.hwnd(win)
        if reset:
            self.u.SetWindowPos(h, wintypes.HWND(-2), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNC)
        self.u.SetWindowPos(h, wintypes.HWND(-1 if pinned else 0), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNC)

    def panel_visible(self, win, on):
        self.u.ShowWindowAsync(self.hwnd(win), 4 if on else 0)       # SW_SHOWNOACTIVATE / SW_HIDE

    def panel_close(self, win):
        try:
            self.u.PostMessageW(self.hwnd(win), 0x0010, 0, 0)
        except Exception:
            pass

    def schedule(self, fn, interval_ms):
        def loop():
            while True:
                time.sleep(interval_ms / 1000.0)
                try:
                    fn()
                except Exception:
                    pass
        threading.Thread(target=loop, daemon=True).start()

    # ---------- cookie профиля ----------
    def cookies(self, sites, delete=False):
        """Полные cookie для заданных сайтов (list[dict]). С delete=True удаляет их из профиля.
        Никогда не блокирует поток интерфейса: незавершённые запросы пропускаются."""
        win = self.main_win
        if win is None or not self.cookie_lock.acquire(timeout=8):
            raise RuntimeError("cookie заняты")
        try:
            nat = win.native
            box = {}

            def start():
                cm = nat.webview.CoreWebView2.CookieManager
                for site in sites:
                    box[site] = cm.GetCookiesAsync(site)

            for _ in range(40):                   # движок страницы может ещё запускаться
                try:
                    self.ui_run(win, start)
                    break
                except Exception:
                    time.sleep(0.25)
            else:
                raise RuntimeError("WebView2 не готов")
            for _ in range(50):
                if all(t.IsCompleted for t in box.values()):
                    break
                time.sleep(0.1)
            if not all(t.IsCompleted for t in box.values()):
                raise RuntimeError("cookie не получены")
            rows = {}

            def extract():                        # COM-объекты cookie читаются только в потоке интерфейса
                cm = nat.webview.CoreWebView2.CookieManager
                for task in box.values():
                    try:
                        if not task.IsCompleted:
                            continue
                        for c in task.Result:
                            key = (str(c.Name), str(c.Domain), str(c.Path))
                            rows[key] = dict(name=str(c.Name), value=str(c.Value), domain=str(c.Domain), path=str(c.Path),
                                             expires=None if c.IsSession else c.Expires.ToString("o"),
                                             http=bool(c.IsHttpOnly), secure=bool(c.IsSecure), same=int(c.SameSite))
                            if delete:
                                cm.DeleteCookie(c)
                    except Exception:
                        pass

            self.ui_run(win, extract)
            return list(rows.values())
        finally:
            self.cookie_lock.release()

    def cookie_restore(self, items):
        import System
        win = self.main_win

        def go():
            cm = win.native.webview.CoreWebView2.CookieManager
            for it in items:
                c = cm.CreateCookie(it["name"], it["value"], it["domain"], it["path"])
                if it.get("expires"):
                    c.Expires = System.DateTime.Parse(it["expires"], System.Globalization.CultureInfo.InvariantCulture,
                                                      System.Globalization.DateTimeStyles.RoundtripKind)
                c.IsHttpOnly = bool(it.get("http"))
                c.IsSecure = bool(it.get("secure"))
                try:
                    from Microsoft.Web.WebView2.Core import CoreWebView2CookieSameSiteKind as K
                    c.SameSite = K(int(it.get("same", 1)))
                except Exception:
                    pass
                cm.AddOrUpdateCookie(c)
        self.ui_run(win, go, timeout=20)

    # ---------- шифрование сохранённых входов (DPAPI) ----------
    def _dpapi(self, data, protect):
        crypt = ctypes.windll.crypt32
        buf = ctypes.create_string_buffer(data, len(data))
        inp = _BLOB(len(data), ctypes.cast(buf, ctypes.POINTER(ctypes.c_char)))
        out = _BLOB()
        fn = crypt.CryptProtectData if protect else crypt.CryptUnprotectData
        if not fn(ctypes.byref(inp), None, None, None, None, 0, ctypes.byref(out)):
            raise OSError("DPAPI")
        res = ctypes.string_at(out.pbData, out.cbData)
        ctypes.windll.kernel32.LocalFree(out.pbData)
        return res

    def protect(self, data):
        return self._dpapi(data, True)

    def unprotect(self, data):
        return self._dpapi(data, False)

    # ---------- прочее ----------
    def clipboard(self):
        try:
            import tkinter
            r = tkinter.Tk()
            r.withdraw()
            txt = r.clipboard_get()
            r.destroy()
            return txt
        except Exception:
            return ""

    def start_watchdog(self, main_win, log):
        """Следит за зависанием потока интерфейса и пишет стеки потоков в журнал."""
        import faulthandler

        def watch():
            while True:
                time.sleep(20)
                try:
                    import System                      # доступно после запуска движка окон
                except Exception:
                    continue
                flag = threading.Event()
                try:
                    main_win.native.BeginInvoke(System.Action(flag.set))
                except Exception:
                    continue
                if not flag.wait(10):
                    log.write(time.strftime("%Y-%m-%d %H:%M:%S") + " ИНТЕРФЕЙС НЕ ОТВЕЧАЕТ, стеки потоков:\n")
                    faulthandler.dump_traceback(file=log, all_threads=True)
                    time.sleep(60)
        threading.Thread(target=watch, daemon=True).start()
