"""Запускает Rust-версию во временной папке данных и управляет ею через DevTools (мышь пользователя не трогаем)."""
import asyncio, json, os, subprocess, sys, tempfile, time, urllib.request
import websockets

EXE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "rs", "target", "release", "MiniPlayer.exe")
PORT = 9333

def start(appdata=None):
    appdata = appdata or tempfile.mkdtemp(prefix="mp_rs_")
    env = dict(os.environ, APPDATA=appdata, MP_EXTRA_ARGS=f"--remote-debugging-port={PORT}")
    p = subprocess.Popen([EXE], env=env, cwd=os.path.dirname(EXE))
    for _ in range(60):
        time.sleep(0.5)
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json", timeout=1).read()
            break
        except Exception:
            pass
    return p, appdata

def targets():
    return json.loads(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json", timeout=3).read())

async def _eval(ws_url, js, await_promise=True):
    async with websockets.connect(ws_url, max_size=None) as ws:
        await ws.send(json.dumps({"id": 1, "method": "Runtime.evaluate", "params": {"expression": js, "awaitPromise": await_promise, "returnByValue": True}}))
        while True:
            m = json.loads(await ws.recv())
            if m.get("id") == 1:
                r = m.get("result", {})
                if "exceptionDetails" in r:
                    return {"error": r["exceptionDetails"].get("exception", {}).get("description", str(r["exceptionDetails"]))}
                return r.get("result", {}).get("value")

def ev(url_part, js):
    for t in targets():
        if t.get("type") == "page" and url_part in t.get("url", ""):
            return asyncio.run(_eval(t["webSocketDebuggerUrl"], js))
    return {"error": "no target " + url_part}

def stop(p):
    p.kill()
    subprocess.run(["taskkill", "/F", "/IM", "msedgewebview2.exe"], capture_output=True)
