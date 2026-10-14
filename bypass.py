"""Режим обхода блокировок (только Windows) на основе zapret (https://github.com/bol-van/zapret, MIT)
и драйвера WinDivert (LGPL / GPL).

Обход действует только внутри нашего приложения:
  * приложение поднимает локальный HTTP-прокси, а браузерные окна (WebView2) и запросы Python идут через него;
  * прокси открывает исходящие соединения только с зарезервированных портов PORT_LO..PORT_HI;
  * winws фильтрует пакеты (WinDivert) только с этих портов, поэтому остальные программы и браузеры не затрагиваются.

winws требует прав администратора. Приложение запускает собственную копию в режиме помощника через UAC
(один запрос), помощник следит за winws и завершает его, когда закрывается приложение.
"""
import ctypes
import json
import os
import select
import shutil
import socket
import ssl
import subprocess
import sys
import threading
import time
import urllib.request
from urllib.parse import urlsplit

PORT_LO, PORT_HI = 52000, 52999
HELPER_FLAG = "--bypass-helper"

# Наши стратегии (параметры winws из документации zapret). Перебираются по очереди, пока не заработает.
STRATEGIES = [
    ("Разделение TLS", "--dpi-desync=multisplit --dpi-desync-split-pos=1,midsld"),
    ("Разделение с перекрытием", "--dpi-desync=multisplit --dpi-desync-split-seqovl=681 --dpi-desync-split-pos=1"),
    ("Подмена и разделение", "--dpi-desync=fake,multisplit --dpi-desync-fooling=badseq --dpi-desync-split-pos=1,midsld --dpi-desync-repeats=4"),
    ("Подмена и обратный порядок", "--dpi-desync=fake,multidisorder --dpi-desync-split-pos=1,midsld --dpi-desync-fooling=md5sig,badseq"),
    ("Ложное разделение", "--dpi-desync=fakedsplit --dpi-desync-split-pos=1,midsld --dpi-desync-fooling=ts"),
    ("Подмена с малым TTL", "--dpi-desync=fake --dpi-desync-ttl=5 --dpi-desync-fooling=md5sig"),
]

PROBE_HOSTS = ["www.youtube.com", "redirector.googlevideo.com", "www.tiktok.com", "www.twitch.tv", "www.netflix.com"]

WIN_FILTER = (
    "(outbound and tcp.DstPort==443 and tcp.SrcPort>=%d and tcp.SrcPort<=%d) or "
    "(inbound and tcp.SrcPort==443 and tcp.DstPort>=%d and tcp.DstPort<=%d)" % (PORT_LO, PORT_HI, PORT_LO, PORT_HI)
)
WIN_FILTER = "!impostor and !loopback and (%s)" % WIN_FILTER


def supported():
    return sys.platform == "win32"


def resource_dir():
    base = getattr(sys, "_MEIPASS", os.path.dirname(os.path.abspath(__file__)))
    return os.path.join(base, "third_party", "zapret")


# ---------------------------------------------------------------- прокси
class Proxy:
    """HTTP-прокси на 127.0.0.1: CONNECT (HTTPS) и обычные http-запросы. Исходящие соединения идут с портов PORT_LO..PORT_HI."""

    def __init__(self):
        self.srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.srv.bind(("127.0.0.1", 0))
        self.srv.listen(128)
        self.port = self.srv.getsockname()[1]
        self._next = PORT_LO
        self._lock = threading.Lock()
        self.alive = True
        threading.Thread(target=self._accept, daemon=True).start()

    def _accept(self):
        while self.alive:
            try:
                c, _ = self.srv.accept()
            except OSError:
                return
            threading.Thread(target=self._serve, args=(c,), daemon=True).start()

    def _connect(self, host, port):
        infos = socket.getaddrinfo(host, port, socket.AF_INET, socket.SOCK_STREAM)
        last = None
        for _fam, _t, _p, _cn, addr in infos:
            for _ in range(60):
                with self._lock:
                    sp = self._next
                    self._next = PORT_LO if self._next >= PORT_HI else self._next + 1
                s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                s.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                try:
                    s.bind(("0.0.0.0", sp))
                except OSError:
                    s.close()
                    continue
                try:
                    s.settimeout(10)
                    s.connect(addr)
                    s.settimeout(None)
                    return s
                except OSError as e:
                    last = e
                    s.close()
                    break
        raise last or OSError("connect failed")

    @staticmethod
    def _pipe(a, b):
        socks = [a, b]
        try:
            while True:
                r, _, x = select.select(socks, [], socks, 120)
                if x or not r:
                    return
                for s in r:
                    data = s.recv(65536)
                    if not data:
                        return
                    (b if s is a else a).sendall(data)
        except OSError:
            return

    def _serve(self, c):
        up = None
        try:
            c.settimeout(15)
            buf = b""
            while b"\r\n\r\n" not in buf:
                d = c.recv(8192)
                if not d:
                    return
                buf += d
                if len(buf) > 65536:
                    return
            head, _, rest = buf.partition(b"\r\n\r\n")
            lines = head.decode("latin1").split("\r\n")
            method, target, ver = lines[0].split(" ", 2)
            c.settimeout(None)
            if method.upper() == "CONNECT":
                host, _, port = target.rpartition(":")
                try:
                    up = self._connect(host.strip("[]"), int(port or 443))
                except OSError:
                    c.sendall(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n")
                    return
                c.sendall(b"HTTP/1.1 200 Connection established\r\n\r\n")
                if rest:
                    up.sendall(rest)
            else:
                u = urlsplit(target)
                host, port = u.hostname, u.port or 80
                up = self._connect(host, port)
                path = (u.path or "/") + ("?" + u.query if u.query else "")
                hdrs = [h for h in lines[1:] if not h.lower().startswith(("proxy-connection", "connection"))]
                up.sendall(("%s %s %s\r\n%s\r\nConnection: close\r\n\r\n" % (method, path, ver, "\r\n".join(hdrs))).encode("latin1") + rest)
            self._pipe(c, up)
        except Exception:
            pass
        finally:
            for s in (c, up):
                try:
                    if s:
                        s.close()
                except OSError:
                    pass

    def close(self):
        self.alive = False
        try:
            self.srv.close()
        except OSError:
            pass


def probe(host, proxy_port, timeout=7.0):
    """Проверяет, что к host:443 через прокси проходит TLS-рукопожатие и приходит ответ HTTP."""
    s = None
    try:
        s = socket.create_connection(("127.0.0.1", proxy_port), timeout=timeout)
        s.sendall(("CONNECT %s:443 HTTP/1.1\r\nHost: %s:443\r\n\r\n" % (host, host)).encode())
        resp = b""
        while b"\r\n\r\n" not in resp:
            d = s.recv(1024)
            if not d:
                return False
            resp += d
        if b" 200 " not in resp.split(b"\r\n")[0]:
            return False
        ctx = ssl.create_default_context()
        ctx.check_hostname = False
        ctx.verify_mode = ssl.CERT_NONE
        s.settimeout(timeout)
        t = ctx.wrap_socket(s, server_hostname=host)
        t.sendall(("HEAD / HTTP/1.1\r\nHost: %s\r\nConnection: close\r\nUser-Agent: Mozilla/5.0\r\n\r\n" % host).encode())
        return t.recv(16).startswith(b"HTTP/")
    except Exception:
        return False
    finally:
        try:
            if s:
                s.close()
        except OSError:
            pass


def score(proxy_port, hosts=PROBE_HOSTS):
    res = {}
    ths = []

    def one(h):
        res[h] = probe(h, proxy_port)
    for h in hosts:
        t = threading.Thread(target=one, args=(h,), daemon=True)
        t.start()
        ths.append(t)
    for t in ths:
        t.join(12)
    return sum(1 for v in res.values() if v), res


# ---------------------------------------------------------------- помощник с правами администратора
def _parent_alive(pid):
    h = ctypes.windll.kernel32.OpenProcess(0x00100000, False, pid)      # SYNCHRONIZE
    if not h:
        return False
    try:
        return ctypes.windll.kernel32.WaitForSingleObject(h, 0) == 0x102   # WAIT_TIMEOUT: процесс ещё идёт
    finally:
        ctypes.windll.kernel32.CloseHandle(h)


def helper_main(parent_pid, workdir):
    """Выполняется с правами администратора: запускает/перезапускает winws по командам приложения и следит за родителем."""
    bindir = os.path.join(workdir, "bin")
    cmdf, statf, filt = (os.path.join(workdir, n) for n in ("cmd.json", "status.json", "filter.txt"))
    proc = None
    cur = None
    err = [""]

    def write(**kw):
        tmp = statf + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(dict(kw, ts=time.time(), strategy=cur, error=err[0], running=bool(proc and proc.poll() is None)), f)
        os.replace(tmp, statf)

    def stop():
        nonlocal proc
        if proc and proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(5)
            except Exception:
                proc.kill()
        proc = None

    def start(i):
        nonlocal proc, cur
        stop()
        args = [os.path.join(bindir, "winws.exe"), "--wf-raw=@" + filt] + STRATEGIES[i][1].split()
        proc = subprocess.Popen(args, cwd=bindir, creationflags=0x08000000)        # CREATE_NO_WINDOW
        cur = i

    last = None
    try:
        while _parent_alive(parent_pid):
            try:
                with open(cmdf, encoding="utf-8") as f:
                    cmd = json.load(f)
            except Exception:
                cmd = {}
            if cmd.get("quit"):
                break
            want = cmd.get("strategy")
            if want != last:
                last = want
                err[0] = ""
                if want is None:
                    stop()
                    cur = None
                else:
                    try:
                        start(int(want))
                    except Exception as e:
                        cur = None
                        err[0] = str(e)
            if proc and proc.poll() is not None:                                    # winws упал: сообщаем и ждём новую команду
                proc = None
            write()
            time.sleep(0.5)
    finally:
        stop()
        try:
            write(done=True)
        except Exception:
            pass


# ---------------------------------------------------------------- управление из приложения
class Bypass:
    def __init__(self, data_dir, settings_get, settings_set):
        self.dir = os.path.join(data_dir, "bypass")
        self.get, self.set = settings_get, settings_set
        self.proxy = None
        self.state, self.msg, self.strategy = "off", "", None
        self.busy = False
        self.want = False

    # --- вспомогательное
    def _cmd(self, **kw):
        with open(os.path.join(self.dir, "cmd.json"), "w", encoding="utf-8") as f:
            json.dump(kw, f)

    def _status(self):
        try:
            with open(os.path.join(self.dir, "status.json"), encoding="utf-8") as f:
                return json.load(f)
        except Exception:
            return {}

    def helper_alive(self):
        st = self._status()
        return bool(st) and not st.get("done") and time.time() - st.get("ts", 0) < 4

    def _prepare_files(self):
        os.makedirs(os.path.join(self.dir, "bin"), exist_ok=True)
        for n in ("winws.exe", "cygwin1.dll", "WinDivert.dll", "WinDivert64.sys"):
            src, dst = os.path.join(resource_dir(), n), os.path.join(self.dir, "bin", n)
            if not os.path.exists(dst) or os.path.getsize(dst) != os.path.getsize(src):
                shutil.copy2(src, dst)
        with open(os.path.join(self.dir, "filter.txt"), "w", encoding="utf-8") as f:
            f.write(WIN_FILTER)
        for n in ("cmd.json", "status.json"):
            try:
                os.remove(os.path.join(self.dir, n))
            except OSError:
                pass

    def start_proxy(self):
        if self.proxy is None:
            self.proxy = Proxy()
        return self.proxy.port

    def browser_args(self):
        """Дополнительные флаги Chromium (WebView2): весь трафик окон идёт через наш прокси, QUIC выключен."""
        return "--proxy-server=http://127.0.0.1:%d --disable-quic" % self.start_proxy()

    def install_python_proxy(self):
        """Запросы самого приложения (поиск, oembed) тоже идут через прокси."""
        p = "http://127.0.0.1:%d" % self.start_proxy()
        urllib.request.install_opener(urllib.request.build_opener(urllib.request.ProxyHandler({"http": p, "https": p})))

    # --- запуск
    def launch(self):
        """Запускает помощника с правами администратора (запрос UAC) и подбирает стратегию в фоне."""
        self.want = True
        if self.busy or not supported():
            return
        self.busy = True
        self.state, self.msg = "starting", "Запуск"
        threading.Thread(target=self._run, daemon=True).start()

    def disable(self):
        """Выключает обход без перезапуска: winws останавливается, помощник остаётся ждать (повторный запрос прав не нужен)."""
        self.want = False
        if not self.busy:
            self._off()

    def _off(self):
        if self.helper_alive():
            self._cmd(strategy=None)
        self.state, self.msg, self.strategy = "off", "", None

    def _run(self):
        try:
            port = self.start_proxy()
            self._cmd(strategy=None) if self.helper_alive() else None
            time.sleep(0.5) if self.helper_alive() else None
            base, _ = score(port)
            if base == len(PROBE_HOSTS):
                self.state, self.msg, self.strategy = "unneeded", "Сайты и так доступны, обход не нужен", None
                return
            if not self.want:
                return
            if self.helper_alive():                          # помощник уже работает с прошлого включения: права не нужны
                self._select(base)
                return
            self._prepare_files()
            if getattr(sys, "frozen", False):
                exe, params = sys.executable, '%s %d "%s"' % (HELPER_FLAG, os.getpid(), self.dir)
            else:
                script = os.path.abspath(sys.argv[0])
                exe, params = sys.executable, '"%s" %s %d "%s"' % (script, HELPER_FLAG, os.getpid(), self.dir)
            rc = ctypes.windll.shell32.ShellExecuteW(None, "runas", exe, params, None, 0)
            if rc <= 32:
                self.state, self.msg = "denied", "Права администратора не выданы, обход выключен"
                return
            t0 = time.time()
            while not self.helper_alive():
                if time.time() - t0 > 30:
                    self.state, self.msg = "failed", "Помощник не запустился"
                    return
                time.sleep(0.3)
            self._select(base)
        except Exception as e:
            self.state, self.msg = "failed", str(e)
        finally:
            self.busy = False
            if not self.want:
                self._off()

    def _select(self, base):
        self.state, self.msg = "testing", "Подбор стратегии"
        saved = self.get("strategy")
        order = list(range(len(STRATEGIES)))
        if isinstance(saved, int) and 0 <= saved < len(order):
            order.remove(saved)
            order.insert(0, saved)
        best, best_i = base, None
        for i in order:
            if not self.want:
                return
            self.msg = "Проверка: %s" % STRATEGIES[i][0]
            self._cmd(strategy=i)
            time.sleep(1.5)
            st = self._status()
            if not st.get("running"):
                continue
            sc, _ = score(self.proxy.port)
            if sc > best:
                best, best_i = sc, i
            if sc == len(PROBE_HOSTS):
                break
        if best_i is None:
            self._cmd(strategy=None)
            self.state, self.msg, self.strategy = "failed", "Ни одна стратегия не помогла у вашего провайдера", None
            return
        self._cmd(strategy=best_i)
        self.strategy = best_i
        self.set("strategy", best_i)
        self.state = "on"
        self.msg = "Работает: %s (%d из %d сайтов)" % (STRATEGIES[best_i][0], best, len(PROBE_HOSTS))

    def retest(self):
        if self.busy or not supported():
            return
        self.set("strategy", None)
        self.launch()

    def stop(self):
        try:
            self._cmd(quit=True)
        except Exception:
            pass
        if self.proxy:
            self.proxy.close()
        self.state = "off"

    def info(self):
        return {"supported": supported(), "state": self.state, "msg": self.msg,
                "strategy": STRATEGIES[self.strategy][0] if self.strategy is not None else ""}
