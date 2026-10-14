"""Mini Player — плавающий мини-плеер для YouTube, TikTok, Vimeo, Twitch и прямых видеофайлов."""
import json
import os
import re
import sys
import threading
import time
import urllib.request
import webbrowser
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs, quote

import webview

DATA_DIR = os.path.join(os.environ.get("APPDATA", os.path.expanduser("~")), "MiniPlayer")
DATA_FILE = os.path.join(DATA_DIR, "data.json")
UA = {"User-Agent": "Mozilla/5.0"}

SIZES_H = {"S": (400, 300), "M": (560, 380), "L": (800, 520)}      # 16:9
SIZES_V = {"S": (300, 560), "M": (380, 720), "L": (460, 860)}      # вертикальное (TikTok)


def resource(name):
    base = getattr(sys, "_MEIPASS", os.path.dirname(os.path.abspath(__file__)))
    return os.path.join(base, name)


def set_icon(win, name="app"):
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


import ctypes
from ctypes import wintypes

_u = ctypes.windll.user32
_u.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.UINT]
_u.GetWindowLongPtrW.restype = ctypes.c_void_p
_u.GetWindowLongPtrW.argtypes = [wintypes.HWND, ctypes.c_int]
_u.SetWindowLongPtrW.argtypes = [wintypes.HWND, ctypes.c_int, ctypes.c_void_p]
_u.SetLayeredWindowAttributes.argtypes = [wintypes.HWND, wintypes.DWORD, ctypes.c_ubyte, wintypes.DWORD]
_u.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
_u.ShowWindowAsync.argtypes = [wintypes.HWND, ctypes.c_int]
_u.IsWindow.argtypes = [wintypes.HWND]
_u.GetClientRect.argtypes = [wintypes.HWND, ctypes.c_void_p]
_u.ClientToScreen.argtypes = [wintypes.HWND, ctypes.c_void_p]
_u.GetForegroundWindow.restype = wintypes.HWND
_u.IsIconic.argtypes = [wintypes.HWND]
_u.IsWindowVisible.argtypes = [wintypes.HWND]
_u.WindowFromPoint.argtypes = [wintypes.POINT]
_u.WindowFromPoint.restype = wintypes.HWND
_u.GetAncestor.argtypes = [wintypes.HWND, wintypes.UINT]
_u.GetAncestor.restype = wintypes.HWND
SWP_NOSIZE, SWP_NOMOVE, SWP_NOZORDER, SWP_NOACTIVATE, SWP_ASYNC = 0x1, 0x2, 0x4, 0x10, 0x4000


class _MONITORINFO(ctypes.Structure):
    _fields_ = [("cbSize", wintypes.DWORD), ("rcMonitor", wintypes.RECT), ("rcWork", wintypes.RECT), ("dwFlags", wintypes.DWORD)]


def ui_run(win, fn, timeout=6):
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


def hwnd_of(win):
    """Дескриптор окна (запоминается в потоке интерфейса при показе окна)."""
    h = getattr(win, "_mp_hwnd", 0)
    if not h:
        h = int(win.native.Handle.ToInt64())
        win._mp_hwnd = h
    return h


def win_rect(h):
    r = wintypes.RECT()
    _u.GetWindowRect(h, ctypes.byref(r))
    return r.left, r.top, r.right - r.left, r.bottom - r.top


def win_place(h, x=None, y=None, w=None, hh=None):
    """Двигает/меняет размер окна без ожидания потока интерфейса (не блокирует)."""
    flags = SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNC
    if x is None:
        flags |= SWP_NOMOVE
    if w is None:
        flags |= SWP_NOSIZE
    _u.SetWindowPos(h, None, x or 0, y or 0, w or 0, hh or 0, flags)


def win_topmost(h, on):
    _u.SetWindowPos(h, wintypes.HWND(-1 if on else -2), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNC)


def work_area(h, full=False):
    """Прямоугольник монитора, на котором окно: (x, y, ширина, высота)."""
    mi = _MONITORINFO()
    mi.cbSize = ctypes.sizeof(mi)
    _u.GetMonitorInfoW(_u.MonitorFromWindow(h, 2), ctypes.byref(mi))
    r = mi.rcMonitor if full else mi.rcWork
    return r.left, r.top, r.right - r.left, r.bottom - r.top


def client_rect_screen(h):
    """Область содержимого окна (без заголовка и рамок) в координатах экрана: (x, y, ширина, высота)."""
    rc = wintypes.RECT()
    _u.GetClientRect(h, ctypes.byref(rc))
    pt = wintypes.POINT(0, 0)
    _u.ClientToScreen(h, ctypes.byref(pt))
    return pt.x, pt.y, rc.right - rc.left, rc.bottom - rc.top


def align_client(h, target):
    """Двигает окно так, чтобы его содержимое совпало с target = (x, y, ширина, высота) на экране."""
    ox, oy, ow, oh = win_rect(h)
    cx, cy, cw, ch = client_rect_screen(h)
    tx, ty, tw, th = target
    win_place(h, tx - (cx - ox), ty - (cy - oy), tw + (ow - cw), th + (oh - ch))


def set_caption(win, visible):
    """Показывает/прячет заголовок окна Windows, оставляя рамку для изменения размера."""
    try:
        h = hwnd_of(win)
        style = _u.GetWindowLongPtrW(h, -16) or 0
        if visible:
            style |= 0x00C00000 | 0x00040000       # WS_CAPTION | WS_THICKFRAME
        else:
            style = (style & ~0x00C00000) | 0x00040000
        _u.SetWindowLongPtrW(h, -16, style)
        _u.SetWindowPos(h, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | 0x0020 | SWP_ASYNC)
    except Exception:
        pass


def set_opacity(win, value):
    """Прозрачность окна целиком (0.3–1.0)."""
    try:
        h = hwnd_of(win)
        v = max(0.3, min(1.0, float(value)))
        ex = _u.GetWindowLongPtrW(h, -20) or 0
        _u.SetWindowLongPtrW(h, -20, ex | 0x00080000)          # WS_EX_LAYERED
        _u.SetLayeredWindowAttributes(h, 0, int(255 * v), 2)    # LWA_ALPHA
    except Exception:
        pass


KEEPALIVE_JS = """(()=>{try{
 Object.defineProperty(Document.prototype,'hidden',{get:()=>false,configurable:true});
 Object.defineProperty(Document.prototype,'visibilityState',{get:()=>'visible',configurable:true});
 Object.defineProperty(Document.prototype,'webkitHidden',{get:()=>false,configurable:true});
 Object.defineProperty(Document.prototype,'webkitVisibilityState',{get:()=>'visible',configurable:true});
 ['visibilitychange','webkitvisibilitychange'].forEach(t=>document.addEventListener(t,e=>e.stopImmediatePropagation(),true));
}catch(e){}})();"""


def install_keepalive(win):
    """Страницы (и вложенные плееры YouTube/Twitch) считают, что окно всегда видно, — не ставят видео на паузу при сворачивании."""
    def run():
        for _ in range(60):
            time.sleep(0.25)
            try:
                ui_run(win, lambda: win.native.webview.CoreWebView2.AddScriptToExecuteOnDocumentCreatedAsync(KEEPALIVE_JS))
                win.evaluate_js(KEEPALIVE_JS)
                return
            except Exception:
                continue
    win.events.shown += lambda: threading.Thread(target=run, daemon=True).start()


def apply_frame(win):
    """При показе окна прячет заголовок, если это включено в настройках."""
    def run():
        d = load()["settings"]
        try:
            win._mp_hwnd = int(win.native.Handle.ToInt64())
        except Exception:
            pass
        set_caption(win, False)
        set_opacity(win, d["ui"]["opacity"])
        target = getattr(win, "_mp_align", None)
        if target:
            def align():
                for _ in range(3):                         # рамки меняются не мгновенно — подгоняем несколько раз
                    time.sleep(0.2)
                    try:
                        align_client(hwnd_of(win), target)
                    except Exception:
                        pass
            threading.Thread(target=align, daemon=True).start()
    win.events.shown += run


TT_DEFAULTS = {"hide_desc": False, "hide_author": False, "hide_ads": True, "remember_window": True,
               "no_frame": True, "fill_crop": False, "fit_window": False, "hide_titlebar": True, "auto_scroll": True}
UI_DEFAULTS = {"layout": "tabs", "default_search": False, "engine": "google", "autohide_bar": True,
               "opacity": 1.0, "op_frame": 1.0, "op_video": 1.0, "auto_paste": False}
TT_WINDOWS = []
CFG_LOCK = threading.RLock()
PLAYER_WINDOWS = []
PANEL_WINDOWS = []
PANEL_SETS = []


# ---------- хранилище ----------
def load():
    try:
        with open(DATA_FILE, encoding="utf-8") as f:
            d = json.load(f)
    except Exception:
        d = {}
    d.setdefault("history", [])
    d.setdefault("settings", {"on_top": True, "size": "M"})
    d["settings"].setdefault("tiktok", {})
    for k, v in TT_DEFAULTS.items():
        d["settings"]["tiktok"].setdefault(k, v)
    d["settings"].setdefault("ui", {})
    for k, v in UI_DEFAULTS.items():
        d["settings"]["ui"].setdefault(k, v)
    d.setdefault("windows", {})
    d.setdefault("pos", {})
    return d


def save(d):
    os.makedirs(DATA_DIR, exist_ok=True)
    with open(DATA_FILE, "w", encoding="utf-8") as f:
        json.dump(d, f, ensure_ascii=False, indent=1)


# ---------- разбор ссылок ----------
def fetch_json(url):
    try:
        req = urllib.request.Request(url, headers=UA)
        return json.loads(urllib.request.urlopen(req, timeout=5).read().decode("utf-8"))
    except Exception:
        return {}


def resolve_redirect(url):
    try:
        req = urllib.request.Request(url, headers=UA)
        return urllib.request.urlopen(req, timeout=6).geturl()
    except Exception:
        return url


def parse(link):
    """-> dict(platform, kind, src, vertical, canonical, title) или None."""
    link = link.strip()
    if not link:
        return None
    if not re.match(r"https?://", link):
        link = "https://" + link
    host = urlparse(link).netloc.lower()

    # YouTube
    m = re.search(r"(?:v=|youtu\.be/|shorts/|embed/|live/)([\w-]{11})", link)
    if m and "youtu" in host:
        vid = m.group(1)
        src = f"https://www.youtube.com/embed/{vid}?autoplay=1&rel=0"
        t = re.search(r"[?&]t=(\d+)", link)
        if t:
            src += f"&start={t.group(1)}"
        pl = re.search(r"[?&]list=([\w-]+)", link)
        if pl:
            src += f"&list={pl.group(1)}"
        canon = f"https://www.youtube.com/watch?v={vid}"
        oe = fetch_json("https://www.youtube.com/oembed?format=json&url=" + quote(canon, safe=""))
        return dict(platform="YouTube", kind="iframe", src=src, vertical="/shorts/" in link, vid=vid,
                    start=int(t.group(1)) if t else 0, list=pl.group(1) if pl else "",
                    canonical=canon, title=oe.get("title") or canon, channel=oe.get("author_name", ""),
                    thumb=f"https://i.ytimg.com/vi/{vid}/mqdefault.jpg")

    # TikTok
    if "tiktok.com" in host:
        if "/video/" not in link:
            link = resolve_redirect(link)
        m = re.search(r"/video/(\d+)", link)
        if m:
            clean = link.split("?")[0]
            oe = fetch_json("https://www.tiktok.com/oembed?url=" + quote(clean, safe=""))
            return dict(platform="TikTok", kind="iframe",
                        src=f"https://www.tiktok.com/player/v1/{m.group(1)}?autoplay=1&loop=1&rel=0",
                        vertical=True, canonical=clean, title=oe.get("title") or clean,
                        channel=oe.get("author_name", ""), thumb=oe.get("thumbnail_url", ""))

    # Vimeo
    m = re.search(r"vimeo\.com/(?:.*/)?(\d+)", link)
    if m:
        return dict(platform="Vimeo", kind="iframe", src=f"https://player.vimeo.com/video/{m.group(1)}?autoplay=1",
                    vertical=False, canonical=link, title=link)

    # Twitch (канал)
    m = re.search(r"twitch\.tv/(\w+)/?$", link.split("?")[0])
    if m:
        return dict(platform="Twitch", kind="iframe",
                    src=f"https://player.twitch.tv/?channel={m.group(1)}&parent=127.0.0.1&autoplay=true",
                    vertical=False, canonical=link, title="Twitch: " + m.group(1),
                    thumb=f"https://static-cdn.jtvnw.net/previews-ttv/live_user_{m.group(1).lower()}-440x248.jpg")

    # прямой видеофайл
    if re.search(r"\.(mp4|webm|mkv|mov|ogv)(\?|$)", link, re.I):
        return dict(platform="Файл", kind="video", src=link, vertical=False, canonical=link,
                    title=link.split("/")[-1])

    # любой другой сайт — открываем как есть
    return dict(platform="Сайт", kind="web", src=link, vertical=False, canonical=link, title=host)


# ---------- настоящий поиск ----------
def _walk(o, key, out):
    if isinstance(o, dict):
        if key in o:
            out.append(o[key])
        for v in o.values():
            _walk(v, key, out)
    elif isinstance(o, list):
        for v in o:
            _walk(v, key, out)


def _txt(x):
    if not x:
        return ""
    return x.get("simpleText") or "".join(r.get("text", "") for r in x.get("runs", []))


def search_youtube(q):
    req = urllib.request.Request("https://www.youtube.com/results?hl=ru&search_query=" + quote(q),
                                 headers=dict(UA, **{"Accept-Language": "ru-RU,ru;q=0.9"}))
    html = urllib.request.urlopen(req, timeout=10).read().decode("utf-8", "ignore")
    m = re.search(r"var ytInitialData = (\{.*?\});</script>", html, re.S)
    if not m:
        return None
    vr = []
    _walk(json.loads(m.group(1)), "videoRenderer", vr)
    res = []
    for v in vr:
        vid = v.get("videoId")
        if not vid or any(r["url"].endswith(vid) for r in res):
            continue
        live = any("LIVE" in json.dumps(b) for b in v.get("badges", []))
        res.append(dict(url="https://www.youtube.com/watch?v=" + vid, title=_txt(v.get("title")),
                        channel=_txt(v.get("ownerText")) or _txt(v.get("longBylineText")),
                        dur="LIVE" if live else _txt(v.get("lengthText")), views=_txt(v.get("viewCountText")),
                        thumb=f"https://i.ytimg.com/vi/{vid}/mqdefault.jpg", platform="YouTube"))
        if len(res) >= 12:
            break
    return res


TT_SEARCH_LOCK = threading.Lock()
TT_SEARCH_JS = r"""JSON.stringify((()=>{const seen=new Set(),out=[];
 for(const a of document.querySelectorAll('a[href*="/video/"]')){const h=a.href.split('?')[0];if(seen.has(h))continue;seen.add(h);
  const im=a.querySelector('img');out.push({url:h,alt:im?im.alt:''});}
 return out.slice(0,10)})())"""


def search_tiktok(q):
    """Поиск видео TikTok: страница поиска читается в скрытом окне (с вашим входом), обложки — через oEmbed."""
    if not TT_SEARCH_LOCK.acquire(timeout=40):
        return None
    win = None
    try:
        win = webview.create_window("tts", "https://www.tiktok.com/search/video?q=" + quote(q), width=900, height=700, hidden=True)
        items = []
        for _ in range(14):
            time.sleep(1.5)
            try:
                items = json.loads(win.evaluate_js(TT_SEARCH_JS) or "[]")
            except Exception:
                items = []
            if len(items) >= 3:
                break
        if not items:
            return []
    except Exception:
        return None
    finally:
        try:
            if win:
                win.destroy()
        except Exception:
            pass
        TT_SEARCH_LOCK.release()
    res = [None] * len(items)

    def one(i, it):
        oe = fetch_json("https://www.tiktok.com/oembed?url=" + quote(it["url"], safe=""))
        cap = re.search(r"«(.*?)» от автора", it.get("alt", ""), re.S)
        res[i] = dict(url=it["url"], title=oe.get("title") or (cap.group(1) if cap else it["url"]),
                      channel=oe.get("author_name", ""), dur="", views="", thumb=oe.get("thumbnail_url", ""),
                      platform="TikTok")
    ths = [threading.Thread(target=one, args=(i, it), daemon=True) for i, it in enumerate(items)]
    for t in ths:
        t.start()
    for t in ths:
        t.join(8)
    return [r for r in res if r]


TW_GQL = ("query($q:String!){searchFor(userQuery:$q,platform:\"web\",options:{targets:[{index:CHANNEL}]})"
          "{channels{items{login displayName stream{viewersCount game{displayName}}}}}}")


def search_twitch(q):
    req = urllib.request.Request("https://gql.twitch.tv/gql", method="POST",
                                 data=json.dumps({"query": TW_GQL, "variables": {"q": q}}).encode(),
                                 headers={"Client-ID": "kimne78kd3eoqrcdpqpfg1rskidb3b", "Content-Type": "application/json", **UA})
    j = json.loads(urllib.request.urlopen(req, timeout=10).read().decode("utf-8"))
    items = j["data"]["searchFor"]["channels"]["items"]
    res = []
    for c in items[:12]:
        st = c.get("stream")
        res.append(dict(url="https://www.twitch.tv/" + c["login"], title=c["displayName"],
                        channel=(st["game"]["displayName"] if st and st.get("game") else "Не в эфире"),
                        dur="LIVE" if st else "", views=(f"{st['viewersCount']} зрителей" if st else ""),
                        thumb=f"https://static-cdn.jtvnw.net/previews-ttv/live_user_{c['login'].lower()}-440x248.jpg",
                        platform="Twitch"))
    return res


# ---------- локальный сервер страниц плеера ----------
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if urlparse(self.path).path == "/panel":
            body = open(resource("panel.html"), encoding="utf-8").read().encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.end_headers()
            self.wfile.write(body)
            return
        q = parse_qs(urlparse(self.path).query)
        src = q.get("src", [""])[0].replace('"', "")
        kind = q.get("kind", ["iframe"])[0]
        if kind == "yt":
            body = open(resource("yt_player.html"), encoding="utf-8").read().encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.end_headers()
            self.wfile.write(body)
            return
        if kind == "video":
            media = f'<video id="v" src="{src}" controls autoplay loop></video>'
        elif q.get("platform", [""])[0] == "YouTube" and q.get("vid", [""])[0]:
            media = '<div id="v"></div>'
        else:
            media = (f'<iframe id="v" src="{src}" allow="autoplay; encrypted-media; picture-in-picture; fullscreen" '
                     'allowfullscreen></iframe>')
        body = open(resource("player.html"), encoding="utf-8").read().replace("__MEDIA__", media).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


server = HTTPServer(("127.0.0.1", 0), Handler)
PORT = server.server_port
threading.Thread(target=server.serve_forever, daemon=True).start()


# ---------- API окна плеера ----------
class PlayerApi:
    def __init__(self, info, on_top):
        self._info = info
        self._window = None
        self._on_top = on_top

    def is_pinned(self):
        return self._on_top

    def pin(self):
        """Закрепить/открепить окно поверх остальных. Возвращает новое состояние."""
        self._on_top = not self._on_top
        try:
            win_topmost(hwnd_of(self._window), self._on_top)
            win = self._window

            def sync():                                   # состояние самой формы WinForms (в её потоке, без ожидания)
                import System
                win.native.BeginInvoke(System.Action(lambda: setattr(win.native, "TopMost", self._on_top)))
            threading.Thread(target=sync, daemon=True).start()
        except Exception:
            pass
        return self._on_top

    def size(self, key):
        table = SIZES_V if self._info["vertical"] else SIZES_H
        w, h = table[key]
        try:
            win_place(hwnd_of(self._window), w=w, hh=h)
        except Exception:
            pass

    def browser(self):
        webbrowser.open(self._info["canonical"])

    def close(self):
        try:
            _u.PostMessageW(hwnd_of(self._window), 0x0010, 0, 0)   # WM_CLOSE
        except Exception:
            self._window.destroy()

    def minimize(self):
        try:
            _u.ShowWindowAsync(hwnd_of(self._window), 6)            # SW_MINIMIZE
        except Exception:
            pass

    def fullscreen(self):
        """Переключает полноэкранный и оконный режимы. Возвращает True, если теперь полный экран."""
        w = self._window
        try:
            h = hwnd_of(w)
            if not getattr(w, "_mp_fs", False):
                self._saved = win_rect(h)
                x, y, ww, hh = work_area(h, full=True)
                w._mp_fs = True
                win_topmost(h, True)
                win_place(h, x, y, ww, hh)
            else:
                x, y, ww, hh = self._saved
                w._mp_fs = False
                win_place(h, x, y, ww, hh)
                win_topmost(h, self._on_top)
        except Exception:
            pass
        return bool(getattr(w, "_mp_fs", False))

    def ui_state(self):
        d = load()["settings"]
        return {"ui": d["ui"], "tt": d["tiktok"], "pinned": self._on_top}

    def ui_setting(self, key, value):
        return update_ui(key, value)

    def tt_setting(self, key, value):
        return update_tt(key, value)

    def drag(self):
        """Перетаскивание окна: пока зажата левая кнопка мыши, окно идёт за курсором."""
        w = self._window
        if getattr(w, "_mp_fs", False):
            return
        try:
            h = hwnd_of(w)
            pt = wintypes.POINT()
            _u.GetCursorPos(ctypes.byref(pt))
            x0, y0, _, _ = win_rect(h)
            ox, oy = pt.x - x0, pt.y - y0
            t0 = time.time()
            while _u.GetAsyncKeyState(0x01) & 0x8000 and time.time() - t0 < 60:
                _u.GetCursorPos(ctypes.byref(pt))
                win_place(h, pt.x - ox, pt.y - oy)
                ps = getattr(self, "_panels", None)
                if ps:
                    ps.reposition()
                time.sleep(0.008)
        except Exception:
            pass

    def save_progress(self, t, dur):
        """Запоминает, на какой секунде остановились (для «Продолжить»). Конец видео = просмотрено."""
        url = self._info.get("canonical")
        if not url:
            return
        with CFG_LOCK:
            d = load()
            for h in d["history"]:
                if h["url"] == url:
                    t = int(t or 0)
                    dur = int(dur or 0)
                    h["pos"] = 0 if (dur and t > dur - 10) else t
                    h["dur"] = dur
                    break
            save(d)

    def get_pos(self, vid):
        return load()["pos"].get(vid, 0)

    def save_pos(self, vid, t):
        d = load()
        if t and t > 5:
            d["pos"][vid] = int(t)
        else:
            d["pos"].pop(vid, None)
        if len(d["pos"]) > 300:
            d["pos"] = dict(list(d["pos"].items())[-200:])
        save(d)

    def fit(self, vw, vh, inner_w, inner_h):
        """Подгоняет окно под пропорции видео (по текущей ширине)."""
        try:
            h = hwnd_of(self._window)
            x, y, ww, hh = win_rect(h)
            target = int(inner_w * vh / vw) + (hh - inner_h)
            target = max(260, min(target, work_area(h)[3] - 20))
            if abs(target - hh) > 4:
                win_place(h, w=ww, hh=target)
        except Exception:
            pass


def notify_main():
    """Просит главное окно перечитать настройки (они могли измениться из окна плеера/TikTok)."""
    try:
        if MAIN_WIN:
            MAIN_WIN[0].evaluate_js("window.__mpSync&&window.__mpSync()")
    except Exception:
        pass


# ---------- панели управления: отдельные окна снаружи окна плеера ----------
AUTOHIDE = [True]
HOT_ZONE = 64            # ширина зоны у края окна, где выезжают панели (пикселей)


class PanelApi:
    """API страницы панели: действия применяются к родительскому окну."""

    def __init__(self, ps, role):
        self._ps = ps
        self._role = role

    def state(self):
        d = load()["settings"]
        w = self._ps.parent
        return {"ui": d["ui"], "tt": d["tiktok"], "pinned": self._ps.api._on_top, "fs": bool(getattr(w, "_mp_fs", False)),
                "title": self._ps.title, "platform": self._ps.platform, "kind": self._ps.kind,
                "gearOpen": self._ps.gear_open}

    def pin(self):
        return self._ps.api.pin()

    def minimize(self):
        self._ps.api.minimize()

    def fullscreen(self):
        return self._ps.api.fullscreen()

    def close(self):
        self._ps.api.close()

    def browser(self):
        self._ps.api.browser()

    def size(self, key):
        self._ps.api.size(key)

    def drag(self):
        self._ps.api.drag()

    def nav(self, path):
        self._ps.parent.evaluate_js("location.href='https://www.tiktok.com%s'" % path)

    def cur_path(self):
        try:
            return self._ps.parent.evaluate_js("location.pathname") or ""
        except Exception:
            return ""

    def act(self, name):
        try:
            return self._ps.parent.evaluate_js("window.__mpAct&&__mpAct('%s')" % name)
        except Exception:
            return None

    def ui_setting(self, key, value):
        return update_ui(key, value)

    def tt_setting(self, key, value):
        return update_tt(key, value)

    def gear(self):
        return self._ps.toggle_gear()

    def set_height(self, role, h):
        self._ps.heights[role] = max(24, min(int(h), 900))


class PanelSet:
    """Верхняя/нижняя панели и окно настроек рядом с окном плеера. Показываются у края окна при наведении."""

    def __init__(self, parent, api, kind, platform, title):
        self.parent, self.api, self.kind, self.platform, self.title = parent, api, kind, platform, title
        self.win, self.hw = {}, {}
        self.heights = {"top": 42 if kind == "player" else 66, "bottom": 56, "gear": 420}
        self.vis = {"top": False, "bottom": False, "gear": False}
        self.last_want = {"top": 0.0, "bottom": 0.0}
        self.gear_open = False
        self.ready = False
        self.alive = True
        self.pos = {}
        self.zstate = {}

    def url(self, role):
        return (f"http://127.0.0.1:{PORT}/panel?role={role}&kind={self.kind}&platform={quote(self.platform, safe='')}"
                f"&title={quote(self.title or '', safe='')}")

    def create(self):
        roles = ["top", "bottom", "gear"] if self.kind == "tt" else ["top", "gear"]
        for role in roles:
            try:
                win = webview.create_window("panel", self.url(role), js_api=PanelApi(self, role), width=300,
                                            height=self.heights[role], frameless=True, hidden=True, resizable=False,
                                            focus=False, min_size=(10, 10), background_color="#26211c")
                self.win[role] = win
                PANEL_WINDOWS.append(win)
            except Exception:
                continue
        for role, win in self.win.items():
            for _ in range(60):
                try:
                    self.hw[role] = int(win.native.Handle.ToInt64())
                    win._mp_hwnd = self.hw[role]
                    break
                except Exception:
                    time.sleep(0.1)
            h = self.hw.get(role)
            if h:
                ex = _u.GetWindowLongPtrW(h, -20) or 0
                _u.SetWindowLongPtrW(h, -20, ex | 0x00000080 | 0x08000000)      # TOOLWINDOW | NOACTIVATE
                _u.ShowWindowAsync(h, 0)
        self.ready = True
        ensure_panel_loop()

    def toggle_gear(self):
        self.gear_open = not self.gear_open
        return self.gear_open

    def destroy(self):
        self.alive = False
        if self in PANEL_SETS:
            PANEL_SETS.remove(self)
        for role, h in self.hw.items():
            try:
                _u.PostMessageW(h, 0x0010, 0, 0)
            except Exception:
                pass
        for w in self.win.values():
            if w in PANEL_WINDOWS:
                PANEL_WINDOWS.remove(w)

    def _show(self, role, on, rect):
        h = self.hw.get(role)
        if not h:
            return
        if on:
            if self.pos.get(role) != rect:
                self.pos[role] = rect
                win_place(h, *rect)
            # держим панель выше окна плеера (окно могло быть поднято кликом): закреплённое — «поверх всего»
            pinned = bool(self.api._on_top)
            if not pinned and self.zstate.get(role) is not False:        # окно открепили — снимаем «поверх всего» и с панели
                _u.SetWindowPos(h, wintypes.HWND(-2), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNC)
            self.zstate[role] = pinned
            _u.SetWindowPos(h, wintypes.HWND(-1 if pinned else 0), 0, 0, 0, 0,
                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNC)
            if not self.vis[role]:
                self.vis[role] = True
                _u.ShowWindowAsync(h, 4)                         # SW_SHOWNOACTIVATE
        elif self.vis[role]:
            self.vis[role] = False
            _u.ShowWindowAsync(h, 0)

    def layout(self, ph):
        """Положение панелей относительно окна плеера: -> {роль: (x, y, w, h)}."""
        x, y, w, h = win_rect(ph)
        mx, my, mw, mh = work_area(ph)
        fs = bool(getattr(self.parent, "_mp_fs", False))
        ht, hb, hg = self.heights["top"], self.heights["bottom"], self.heights["gear"]
        top_y = y if (fs or y - ht < my) else y - ht
        bot_y = y + h - hb if (fs or y + h + hb > my + mh) else y + h
        gw = 270
        gx = x + w + 6
        if gx + gw > mx + mw:
            gx = x - gw - 6
        gh = min(hg, mh - 20)
        return {"top": (x, top_y, w, ht), "bottom": (x, bot_y, w, hb), "gear": (max(mx, gx), max(my, y), gw, gh)}

    def reposition(self):
        """Сразу переставляет видимые панели за окном (во время перетаскивания)."""
        try:
            rects = self.layout(hwnd_of(self.parent))
            for role, rect in rects.items():
                h = self.hw.get(role)
                if h and self.vis.get(role) and self.pos.get(role) != rect:
                    self.pos[role] = rect
                    win_place(h, *rect)
        except Exception:
            pass

    def tick(self):
        ph = hwnd_of(self.parent)
        if not _u.IsWindow(ph):
            self.destroy()
            return
        gone = _u.IsIconic(ph) or not _u.IsWindowVisible(ph)
        x, y, w, h = win_rect(ph)
        rects = self.layout(ph)
        pt = wintypes.POINT()
        _u.GetCursorPos(ctypes.byref(pt))
        root = _u.GetAncestor(_u.WindowFromPoint(pt), 2)
        now = time.time()
        in_p = root == ph and x <= pt.x < x + w and y <= pt.y < y + h
        always = not AUTOHIDE[0]
        want_top = always or self.gear_open or (in_p and pt.y < y + HOT_ZONE) or (self.vis["top"] and root == self.hw.get("top"))
        want_bot = always or (in_p and pt.y >= y + h - HOT_ZONE) or (self.vis["bottom"] and root == self.hw.get("bottom"))
        # под курсором чужое окно (переключились на другое приложение, оно перекрыло край) — панели убираем сразу, без задержки
        foreign = root not in ([ph] + list(self.hw.values()))
        if gone or (foreign and not always and not self.gear_open):
            want_top = want_bot = False
            self.last_want["top"] = self.last_want["bottom"] = 0
        if want_top:
            self.last_want["top"] = now
        if want_bot:
            self.last_want["bottom"] = now
        if "top" in self.hw:
            self._show("top", now - self.last_want["top"] < 0.45, rects["top"])
        if "bottom" in self.hw:
            self._show("bottom", now - self.last_want["bottom"] < 0.45, rects["bottom"])
        if "gear" in self.hw:
            self._show("gear", self.gear_open and not gone and (not foreign or self.vis["gear"]), rects["gear"])


def panel_loop():
    while True:
        time.sleep(0.06)
        for ps in list(PANEL_SETS):
            try:
                if ps.ready and ps.alive:
                    ps.tick()
            except Exception:
                pass


_PANEL_THREAD = []


def ensure_panel_loop():
    if not _PANEL_THREAD:
        t = threading.Thread(target=panel_loop, daemon=True)
        _PANEL_THREAD.append(t)
        t.start()


def attach_panels(win, api, kind, platform, title):
    ps = PanelSet(win, api, kind, platform, title)
    api._panels = ps
    PANEL_SETS.append(ps)
    win.events.shown += lambda: threading.Thread(target=ps.create, daemon=True).start()
    win.events.closed += ps.destroy
    return ps


def update_ui(key, value):
    """Общие настройки окон (панель, прозрачность, раскладка…) — применяются ко всем открытым окнам."""
    with CFG_LOCK:
        d = load()
        d["settings"]["ui"][key] = value
        save(d)
    if key == "autohide_bar":
        AUTOHIDE[0] = bool(value)
    if key == "opacity":
        for w in list(TT_WINDOWS) + list(PLAYER_WINDOWS):
            set_opacity(w, value)
    for w in list(TT_WINDOWS) + list(PLAYER_WINDOWS) + list(PANEL_WINDOWS):
        try:
            w.evaluate_js("window.__mpUi&&window.__mpUi(%s)" % json.dumps(d["settings"]["ui"]))
        except Exception:
            pass
    notify_main()
    return d


def update_tt(key, value):
    with CFG_LOCK:
        d = load()
        d["settings"]["tiktok"][key] = value
        save(d)
    for w in list(TT_WINDOWS):
        try:
            w.evaluate_js(tiktok_js())
        except Exception:
            pass
    for w in list(PANEL_WINDOWS):
        try:
            w.evaluate_js("window.__mpRefresh&&window.__mpRefresh()")
        except Exception:
            pass
    notify_main()
    return d


def open_player(info, geom=None):
    s = load()["settings"]
    table = SIZES_V if info["vertical"] else SIZES_H
    w, h = table.get(s.get("size", "M"), table["M"])
    if info["kind"] == "web":
        webview.create_window(info["title"], info["src"], width=w, height=h,
                              on_top=s["on_top"], min_size=(240, 150))
        return
    extra = f"&start={info.get('start', 0)}"
    if info["platform"] == "YouTube":
        extra += f"&vid={info['vid']}&list={info.get('list', '')}"
    url = (f"http://127.0.0.1:{PORT}/?kind={info['kind']}&src={quote(info['src'], safe='')}"
           f"&title={quote(info['title'] or '', safe='')}&platform={quote(info['platform'], safe='')}{extra}")
    api = PlayerApi(info, s["on_top"])
    key = {"YouTube": "youtube"}.get(info["platform"], "player")
    kw = restore_geometry(key, w, h)
    if geom:                                                  # открыто из окна сайта — точно на его месте и того же размера
        kw = dict(x=geom[0], y=geom[1], width=geom[2], height=geom[3])
    win = webview.create_window(f"{info['platform']} — Mini Player", url, js_api=api, on_top=s["on_top"],
                                min_size=(300, 220), **kw)
    if geom:
        win._mp_align = geom
    api._window = win
    set_icon(win, {"YouTube": "youtube", "Twitch": "twitch"}.get(info["platform"], "other"))
    track_geometry(win, key)
    apply_frame(win)
    install_keepalive(win)
    attach_panels(win, api, "player", info["platform"], info["title"])
    PLAYER_WINDOWS.append(win)
    win.events.closed += lambda: PLAYER_WINDOWS.remove(win) if win in PLAYER_WINDOWS else None


def restore_geometry(key, w, h):
    """-> kwargs для create_window с сохранёнными размером и позицией."""
    d = load()
    g = d["windows"].get(key)
    kw = dict(width=w, height=h)
    if d["settings"]["tiktok"]["remember_window"] and g:
        if g.get("width", 0) >= 240 and g.get("height", 0) >= 240:
            kw.update(width=g["width"], height=g["height"])
        if -3000 < g.get("x", 99999) < 6000 and -3000 < g.get("y", 99999) < 6000:
            kw.update(x=g["x"], y=g["y"])
    return kw


def track_geometry(win, key):
    state = {}
    last = [0.0]

    def write(force=False):
        if not state or getattr(win, "_mp_fs", False) or not load()["settings"]["tiktok"]["remember_window"]:
            return
        if not force and time.time() - last[0] < 1.0:
            return
        last[0] = time.time()
        d = load()
        d["windows"][key] = dict(d["windows"].get(key, {}), **state)
        save(d)

    def moved(x, y):
        state.update(x=int(x), y=int(y))
        write()

    def resized(width, height):
        state.update(width=int(width), height=int(height))
        write()

    win.events.moved += moved
    win.events.resized += resized
    win.events.closing += lambda: write(True)


def tiktok_js():
    st = load()["settings"]["tiktok"]
    pre = "window.__mpset=" + json.dumps(st) + ";if(window.__mpApply)window.__mpApply();"
    return pre + open(resource("tiktok_inject.js"), encoding="utf-8").read()


def open_tiktok(url):
    st = load()["settings"]
    w, h = SIZES_V.get(st.get("size", "M"), SIZES_V["M"])
    api = PlayerApi(dict(vertical=True, canonical=url), st["on_top"])
    win = webview.create_window("TikTok — Mini Player", url, js_api=api, on_top=st["on_top"],
                                min_size=(320, 240), **restore_geometry("tiktok", w, h))
    api._window = win
    set_icon(win, "tiktok")
    apply_frame(win)
    install_keepalive(win)
    attach_panels(win, api, "tt", "TikTok", "TikTok")
    TT_WINDOWS.append(win)
    track_geometry(win, "tiktok")
    win.events.closed += lambda: TT_WINDOWS.remove(win) if win in TT_WINDOWS else None
    win.events.loaded += lambda: win.evaluate_js(tiktok_js())


# ---------- API главного окна ----------
LOGIN_SITES = {
    "yt": (["https://www.youtube.com", "https://www.google.com"], {"SAPISID", "__Secure-3PAPISID", "LOGIN_INFO"}, None),
    "tt": (["https://www.tiktok.com"], {"sessionid", "sid_tt"}, None),
    "tw": (["https://www.twitch.tv"], {"auth-token"}, "login"),
}
MAIN_WIN = []


COOKIE_LOCK = threading.Lock()
LOGIN_CACHE = {}


def read_cookies(sites, delete=False):
    """Cookie профиля приложения для заданных сайтов (имя, значение). С delete=True удаляет их.
    Никогда не блокирует поток интерфейса: незавершённые запросы просто пропускаются."""
    import System
    if not MAIN_WIN or not COOKIE_LOCK.acquire(timeout=8):
        raise RuntimeError("cookie заняты")
    try:
        nat = MAIN_WIN[0].native
        box = {}

        def start():
            cm = nat.webview.CoreWebView2.CookieManager
            for site in sites:
                box[site] = cm.GetCookiesAsync(site)

        for _ in range(40):                   # движок страницы может ещё запускаться
            try:
                ui_run(MAIN_WIN[0], start)
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
        res = {}

        def extract():                        # COM-объекты cookie читаются только в потоке интерфейса
            cm = nat.webview.CoreWebView2.CookieManager
            for site, task in box.items():
                lst = []
                try:
                    if task.IsCompleted:
                        for c in task.Result:
                            lst.append((str(c.Name), str(c.Value)))
                            if delete:
                                cm.DeleteCookie(c)
                except Exception:
                    pass
                res[site] = lst

        ui_run(MAIN_WIN[0], extract)
        return res
    finally:
        COOKIE_LOCK.release()


# ---------- данные аккаунта (ник и аватар) со страницы сайта ----------
ACCOUNT_JS = {
    "tt": ("https://www.tiktok.com/foryou", None, r"""(()=>{const a=document.querySelector('[data-e2e="nav-profile"]');if(!a)return null;
 const l=a.closest('a')||a.querySelector('a')||a;const href=l.getAttribute&&l.getAttribute('href')||'';
 if(!/^\/@/.test(href))return null;const im=a.querySelector('img');
 return JSON.stringify({name:'',handle:href.slice(1),avatar:im?im.src:''})})()"""),
    "yt": ("https://www.youtube.com", """(async()=>{try{window.__acc=null;const c=window.ytcfg;const key=c.get('INNERTUBE_API_KEY'),ctx=c.get('INNERTUBE_CONTEXT');
 const sid=(document.cookie.match(/(?:^|; )SAPISID=([^;]+)/)||[])[1];const ts=Math.floor(Date.now()/1000);
 const buf=new TextEncoder().encode(ts+' '+sid+' https://www.youtube.com');
 const h=[...new Uint8Array(await crypto.subtle.digest('SHA-1',buf))].map(b=>b.toString(16).padStart(2,'0')).join('');
 const r=await fetch('/youtubei/v1/account/account_menu?key='+key,{method:'POST',credentials:'include',headers:{'Content-Type':'application/json','Authorization':'SAPISIDHASH '+ts+'_'+h,'X-Origin':'https://www.youtube.com','X-Goog-AuthUser':'0'},body:JSON.stringify({context:ctx})});
 const j=await r.json();const hd=j.actions[0].openPopupAction.popup.multiPageMenuRenderer.header.activeAccountHeaderRenderer;
 window.__acc=JSON.stringify({name:hd.accountName.simpleText,handle:(hd.channelHandle||{}).simpleText||'',avatar:hd.accountPhoto.thumbnails[0].url})}catch(e){window.__acc='ERR '+e}})();1""",
           "window.__acc"),
    "tw": ("https://www.twitch.tv", None, """(()=>{const b=document.querySelector('[data-a-target="user-menu-toggle"]');if(!b)return null;
 const im=b.querySelector('img');return JSON.stringify({name:'',handle:'',avatar:im?im.src:''})})()"""),
}
ACCOUNT_BUSY = set()


def fetch_account(pid, name_hint=""):
    """Открывает скрытое окно с сайтом и читает ник и аватар вошедшего аккаунта. -> dict или None."""
    if pid in ACCOUNT_BUSY:
        return None
    ACCOUNT_BUSY.add(pid)
    win = None
    result = None
    try:
        spec = ACCOUNT_JS[pid]
        url = spec[0]
        start_js = spec[1] if len(spec) == 3 and spec[1] else None
        poll_js = spec[3] if len(spec) == 4 else spec[2]
        win = webview.create_window("acc", url, width=900, height=700, hidden=True)
        time.sleep(4)
        for i in range(30):
            try:
                if start_js and i == 0:
                    win.evaluate_js(start_js)
                r = win.evaluate_js(poll_js)
                if r and not str(r).startswith("ERR"):
                    result = json.loads(r)
                    break
            except Exception:
                pass
            time.sleep(0.7)
        if result and name_hint and not result.get("name"):
            result["name"] = name_hint
    except Exception:
        result = None
    finally:
        ACCOUNT_BUSY.discard(pid)
        try:
            if win:
                win.destroy()
        except Exception:
            pass
    return result


# ---------- хранилище аккаунтов: сеансы (cookie) шифруются DPAPI и доступны только вам в Windows ----------
ACC_LOCK = threading.RLock()
ACC_FILE = os.path.join(DATA_DIR, "accounts.bin")
ACC_PENDING = set()
ACC_REG_BUSY = set()
ACC_SITES = {
    "yt": ["https://www.youtube.com", "https://www.google.com", "https://accounts.google.com", "https://myaccount.google.com",
           "https://www.googleapis.com"],
    "tt": ["https://www.tiktok.com"],
    "tw": ["https://www.twitch.tv"],
}
LOGIN_PAGES = {"yt": "https://accounts.google.com/ServiceLogin?service=youtube&continue=https://www.youtube.com/",
               "tt": "https://www.tiktok.com/login", "tw": "https://www.twitch.tv/login"}


class _BLOB(ctypes.Structure):
    _fields_ = [("cbData", wintypes.DWORD), ("pbData", ctypes.POINTER(ctypes.c_char))]


def _dpapi(data, protect):
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


def acc_store_load():
    try:
        with open(ACC_FILE, "rb") as f:
            return json.loads(_dpapi(f.read(), False).decode("utf-8"))
    except Exception:
        return {}


def acc_store_save(st):
    os.makedirs(DATA_DIR, exist_ok=True)
    with open(ACC_FILE, "wb") as f:
        f.write(_dpapi(json.dumps(st).encode("utf-8"), True))


def cookie_dump(pid):
    """Полные cookie платформы (для сохранения сеанса аккаунта)."""
    import System
    sites = ACC_SITES[pid]
    nat = MAIN_WIN[0].native
    box = {}

    def start():
        cm = nat.webview.CoreWebView2.CookieManager
        for site in sites:
            box[site] = cm.GetCookiesAsync(site)
    ui_run(MAIN_WIN[0], start)
    for _ in range(60):
        if all(t.IsCompleted for t in box.values()):
            break
        time.sleep(0.1)
    rows = {}

    def extract():
        for task in box.values():
            if not task.IsCompleted:
                continue
            for c in task.Result:
                key = (str(c.Name), str(c.Domain), str(c.Path))
                rows[key] = dict(name=str(c.Name), value=str(c.Value), domain=str(c.Domain), path=str(c.Path),
                                 expires=None if c.IsSession else c.Expires.ToString("o"),
                                 http=bool(c.IsHttpOnly), secure=bool(c.IsSecure), same=int(c.SameSite))
    ui_run(MAIN_WIN[0], extract)
    return list(rows.values())


def cookie_wipe(pid):
    """Удаляет все cookie платформы из профиля (выход из аккаунта)."""
    read_cookies(ACC_SITES[pid], delete=True)


def cookie_restore(items):
    import System

    def go():
        cm = MAIN_WIN[0].native.webview.CoreWebView2.CookieManager
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
    ui_run(MAIN_WIN[0], go, timeout=20)


def acc_public(d, pid):
    return d.setdefault("acc", {}).setdefault(pid, {"active": None, "list": []})


def acc_register_current(pid):
    """Запоминает вошедший сейчас аккаунт платформы (ник, аватар, сеанс). -> id или None."""
    if pid in ACC_REG_BUSY:
        return None
    ACC_REG_BUSY.add(pid)
    try:
        return _acc_register_current(pid)
    finally:
        ACC_REG_BUSY.discard(pid)


def _acc_register_current(pid):
    info = fetch_account(pid) or {}
    if not (info.get("name") or info.get("handle")):         # данных об аккаунте нет — не создаём пустую запись, попробуем позже
        return None
    cookies = cookie_dump(pid)
    if not cookies:
        return None
    with ACC_LOCK:
        with CFG_LOCK:
            d = load()
            pub = acc_public(d, pid)
            ident = info.get("handle") or info.get("name") or ""
            ent = next((a for a in pub["list"] if ident and (a.get("handle") == info.get("handle") and a.get("name") == info.get("name"))), None)
            if not ent:
                ent = {"id": "a%d" % int(time.time() * 1000)}
                pub["list"].append(ent)
            ent.update(name=info.get("name", ""), handle=info.get("handle", ""), avatar=info.get("avatar", ""))
            pub["active"] = ent["id"]
            save(d)
        st = acc_store_load()
        st.setdefault(pid, {})[ent["id"]] = cookies
        acc_store_save(st)
        return ent["id"]


def acc_snapshot_active(pid):
    with ACC_LOCK:
        d = load()
        act = acc_public(d, pid)["active"]
        if not act:
            return
        cookies = cookie_dump(pid)
        if cookies:
            st = acc_store_load()
            st.setdefault(pid, {})[act] = cookies
            acc_store_save(st)


def acc_switch(pid, aid):
    with ACC_LOCK:
        st = acc_store_load()
        target = st.get(pid, {}).get(aid)
        if not target:
            raise RuntimeError("нет сохранённого сеанса")
        acc_snapshot_active(pid)
        cookie_wipe(pid)
        time.sleep(0.6)
        cookie_restore(target)
        with CFG_LOCK:
            d = load()
            acc_public(d, pid)["active"] = aid
            save(d)


def acc_watch_login(pid, fallback_id):
    """После «Добавить аккаунт»: ждёт вход на сайте и запоминает новый аккаунт; по таймауту возвращает прежний."""
    ACC_PENDING.add(pid)
    try:
        api = MainApi()
        for _ in range(100):                                   # до ~5 минут
            time.sleep(3)
            if api.login_state().get(pid, {}).get("in"):
                time.sleep(2)
                acc_register_current(pid)
                return
        if fallback_id:
            acc_switch(pid, fallback_id)
    except Exception:
        pass
    finally:
        ACC_PENDING.discard(pid)


# ---------- журнал сбоев и зависаний ----------
def start_diagnostics(main_win):
    import faulthandler
    os.makedirs(DATA_DIR, exist_ok=True)
    log = open(os.path.join(DATA_DIR, "crash.log"), "a", encoding="utf-8", buffering=1)
    faulthandler.enable(log)

    def hook(args):
        log.write(time.strftime("%Y-%m-%d %H:%M:%S") + " поток " + str(args.thread) + ": " + repr(args.exc_value) + "\n")
    threading.excepthook = hook

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


INTERCEPT_JS = r"""(()=>{const K='%s';if(window.__mpI)return;window.__mpI=1;
 const isVideo=h=>{try{const u=new URL(h,location.href);
  if(K==='yt')return /(^|\.)youtube\.com$/.test(u.hostname)&&(u.pathname==='/watch'&&u.searchParams.get('v')||/^\/(shorts|live)\/[\w-]{11}/.test(u.pathname));
  return /(^|\.)twitch\.tv$/.test(u.hostname)&&/^\/\w{3,25}\/?$/.test(u.pathname)&&!/^\/(directory|downloads|jobs|turbo|store|settings|subscriptions|inventory|wallet|drops|friends|messages|login|signup|search|p|videos|moderator|u)\/?$/i.test(u.pathname);}catch(e){return false}};
 document.addEventListener('click',e=>{if(e.button!==0||e.ctrlKey||e.metaKey||e.shiftKey)return;
  const a=e.target.closest&&e.target.closest('a[href]');if(!a||!isVideo(a.href))return;
  e.preventDefault();e.stopPropagation();e.stopImmediatePropagation();
  if(window.pywebview&&pywebview.api)pywebview.api.open_video(a.href)},true);
})();"""


class SiteApi:
    """Окно обычного сайта (YouTube/Twitch): клик по видео открывает его в плеере приложения."""

    def __init__(self):
        self._window = None

    def open_video(self, url):
        try:
            geom = client_rect_screen(hwnd_of(self._window))
        except Exception:
            geom = None
        r = MainApi().play(url, "", False, geom)
        if r.get("ok"):
            notify_main()
            try:
                _u.PostMessageW(hwnd_of(self._window), 0x0010, 0, 0)      # закрыть окно сайта
            except Exception:
                pass


class MainApi:
    def state(self):
        return load()

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

    def play(self, link, mode="", resume=False, geom=None):
        link = link.strip()
        if mode == "twitch" and link and "." not in link:
            link = "https://www.twitch.tv/" + link.lstrip("@/")
        if mode == "tiktok" and link.startswith("@"):
            link = "https://www.tiktok.com/" + link
            self.site(link, True)
            return {"ok": True, "state": load()}
        info = parse(link)
        if not info:
            return {"error": "Вставь ссылку"}
        d = load()
        old = next((h for h in d["history"] if h["url"] == info["canonical"]), {})
        if resume and old.get("pos", 0) > 5:
            info["start"] = int(old["pos"])
        d["history"] = [h for h in d["history"] if h["url"] != info["canonical"]]
        d["history"].insert(0, dict(url=info["canonical"], title=info["title"], platform=info["platform"],
                                    thumb=info.get("thumb", ""), fav=old.get("fav", False), ts=int(time.time()),
                                    pos=old.get("pos", 0), dur=old.get("dur", 0), pinned=old.get("pinned", False)))
        d["history"] = [h for i, h in enumerate(d["history"]) if i < 200 or h.get("fav") or h.get("pinned")]
        save(d)
        open_player(info, geom)
        return {"ok": True, "state": d}

    def site(self, url, vertical=False):
        st = load()["settings"]
        if "tiktok.com" in url:
            return open_tiktok(url)
        table = SIZES_V if vertical else SIZES_H
        w, h = table.get(st.get("size", "M"), table["M"])
        if not vertical:
            w, h = w + 120, h + 80
        host = urlparse(url).netloc.lower()
        kind = "yt" if "youtube.com" in host else "tw" if "twitch.tv" in host else ""
        if not kind:
            webview.create_window("Mini Player", url, width=w, height=h, on_top=st["on_top"], min_size=(300, 300))
            return
        api = SiteApi()
        win = webview.create_window("Mini Player", url, js_api=api, on_top=st["on_top"], min_size=(300, 300),
                                    **restore_geometry("site", w, h))
        api._window = win
        track_geometry(win, "site")
        js = INTERCEPT_JS % kind
        win.events.loaded += lambda: win.evaluate_js(js)

    def fav(self, url):
        d = load()
        for h in d["history"]:
            if h["url"] == url:
                h["fav"] = not h.get("fav")
        save(d)
        return d

    def remove(self, url):
        d = load()
        d["history"] = [h for h in d["history"] if h["url"] != url]
        save(d)
        return d

    def clear(self, favs_too=False):
        d = load()
        d["history"] = [] if favs_too else [h for h in d["history"] if h.get("fav") or h.get("pinned")]
        save(d)
        return d

    def pin_item(self, url):
        """Закрепить/открепить запись в «Недавно открытых»: закреплённые идут первыми и не стираются кнопкой «Очистить»."""
        d = load()
        for h in d["history"]:
            if h["url"] == url:
                h["pinned"] = not h.get("pinned")
        save(d)
        return d

    def search(self, query, pid):
        """Результаты поиска внутри платформы; None — если для платформы своего поиска нет."""
        try:
            if pid == "yt":
                return search_youtube(query)
            if pid == "tt":
                return search_tiktok(query)
        except Exception:
            return None
        return None

    def login_state(self):
        """{платформа: {in: вошли ли, name: имя}} по cookie профиля. При сбое чтения возвращает прошлое известное значение."""
        res = {}
        for pid, (sites, need, name_cookie) in LOGIN_SITES.items():
            try:
                allc = read_cookies(sites)
                names = {n: v for lst in allc.values() for n, v in lst}
                res[pid] = {"in": any(n in names for n in need),
                            "name": names.get(name_cookie, "") if name_cookie else ""}
                LOGIN_CACHE[pid] = res[pid]
            except Exception:
                res[pid] = dict(LOGIN_CACHE.get(pid, {"in": False, "name": ""}), unknown=True)
        return res

    def accounts(self):
        """Состояние входа и список аккаунтов платформ. Вошедший, но ещё не запомненный аккаунт запоминается в фоне."""
        state = self.login_state()
        d = load()
        for pid in list(d.get("acc", {})):                          # убрать пустые записи без имени и ника
            pub0 = acc_public(d, pid)
            blank = [a for a in pub0["list"] if not (a.get("name") or a.get("handle"))]
            if blank:
                with CFG_LOCK:
                    d2 = load()
                    pub2 = acc_public(d2, pid)
                    pub2["list"] = [a for a in pub2["list"] if a.get("name") or a.get("handle")]
                    if pub2["active"] and not any(a["id"] == pub2["active"] for a in pub2["list"]):
                        pub2["active"] = None
                    save(d2)
                st0 = acc_store_load()
                for a in blank:
                    st0.get(pid, {}).pop(a["id"], None)
                acc_store_save(st0)
                d = load()
        for pid, st in state.items():
            pub = acc_public(d, pid)
            if st["in"] and not pub["active"] and pid not in ACC_PENDING and pid not in ACCOUNT_BUSY:
                threading.Thread(target=acc_register_current, args=(pid,), daemon=True).start()
            elif not st["in"] and not st.get("unknown") and pub["active"] and pid not in ACC_PENDING:
                pub["active"] = None
        acc = {pid: {"active": v["active"], "list": v["list"]} for pid, v in d.get("acc", {}).items()}
        return {"login": state, "acc": acc, "pending": sorted(ACC_PENDING)}

    def acc_switch(self, pid, aid):
        try:
            acc_switch(pid, aid)
        except Exception as e:
            return dict(self.accounts(), error=str(e))
        return self.accounts()

    def acc_add(self, pid):
        """Начать добавление аккаунта: текущий сеанс сохраняется, окно входа открывается чистым."""
        with ACC_LOCK:
            d = load()
            pub = acc_public(d, pid)
            cur = pub["active"]
            if self.login_state().get(pid, {}).get("in") and not cur:
                cur = acc_register_current(pid)
            if len(pub["list"]) >= 4:
                return dict(self.accounts(), error="Не больше 4 аккаунтов")
            acc_snapshot_active(pid)
            cookie_wipe(pid)
        threading.Thread(target=acc_watch_login, args=(pid, cur), daemon=True).start()
        self.site(LOGIN_PAGES[pid], pid == "tt")
        return self.accounts()

    def acc_remove(self, pid, aid):
        with ACC_LOCK:
            with CFG_LOCK:
                d = load()
                pub = acc_public(d, pid)
                was_active = pub["active"] == aid
                pub["list"] = [a for a in pub["list"] if a["id"] != aid]
                if was_active:
                    pub["active"] = None
                save(d)
            st = acc_store_load()
            st.get(pid, {}).pop(aid, None)
            acc_store_save(st)
            if was_active:
                cookie_wipe(pid)
                if pub["list"]:
                    try:
                        acc_switch(pid, pub["list"][0]["id"])
                    except Exception:
                        pass
        return self.accounts()

    def logout(self, pid):
        """Выйти из всех аккаунтов платформы."""
        with ACC_LOCK:
            try:
                cookie_wipe(pid)
            except Exception:
                pass
            with CFG_LOCK:
                d = load()
                d.setdefault("acc", {})[pid] = {"active": None, "list": []}
                save(d)
            st = acc_store_load()
            st.pop(pid, None)
            acc_store_save(st)
        return self.accounts()

    def tt_setting(self, key, value):
        return update_tt(key, value)

    def ui_setting(self, key, value):
        return update_ui(key, value)

    def preview(self, link):
        """Название и автор по ссылке (для карточки предпросмотра)."""
        info = parse(link)
        if not info:
            return None
        return {"title": info["title"], "channel": info.get("channel", ""), "platform": info["platform"],
                "vertical": info["vertical"], "url": info["canonical"], "thumb": info.get("thumb", "")}

    def fav_link(self, link):
        """Добавляет ссылку в избранное, не запуская её."""
        info = parse(link)
        d = load()
        if info:
            ent = next((h for h in d["history"] if h["url"] == info["canonical"]), None)
            if ent:
                ent["fav"] = not ent.get("fav")
            else:
                d["history"].insert(0, dict(url=info["canonical"], title=info["title"], platform=info["platform"],
                                            thumb=info.get("thumb", ""), fav=True, ts=int(time.time())))
            save(d)
        return d

    def setting(self, key, value):
        d = load()
        d["settings"][key] = value
        save(d)
        return d


HTML = open(resource("ui.html"), encoding="utf-8").read()

if __name__ == "__main__":
    # окна не «засыпают» при сворачивании: звук и видео продолжаются
    os.environ.setdefault("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
                          "--disable-features=CalculateNativeWinOcclusion --disable-renderer-backgrounding "
                          "--disable-background-timer-throttling --disable-backgrounding-occluded-windows "
                          "--autoplay-policy=no-user-gesture-required")
    import socket
    _lock = socket.socket()
    try:
        _lock.bind(("127.0.0.1", 47653))
    except OSError:
        raise SystemExit  # уже запущено
    main = webview.create_window("Mini Player", html=HTML, js_api=MainApi(), min_size=(640, 480),
                                 **restore_geometry("main", 760, 620))
    track_geometry(main, "main")
    set_icon(main)
    install_keepalive(main)
    MAIN_WIN.append(main)
    start_diagnostics(main)
    AUTOHIDE[0] = load()["settings"]["ui"].get("autohide_bar", True)
    os.makedirs(os.path.join(DATA_DIR, "web"), exist_ok=True)
    main_win = webview.windows[0]
    failed = []

    def watchdog():
        # профиль браузера иногда ещё занят после прошлого запуска — тогда окно остаётся белым
        try:
            main_win.evaluate_js("1")
        except Exception:
            failed.append(1)
            main_win.destroy()

    webview.start(watchdog, private_mode=False, storage_path=os.path.join(DATA_DIR, "web"))
    tries = int(os.environ.get("MP_TRY", "0"))
    if failed and tries < 8:
        import subprocess
        _lock.close()
        time.sleep(3)
        subprocess.Popen([sys.executable] + sys.argv, env=dict(os.environ, MP_TRY=str(tries + 1)))
