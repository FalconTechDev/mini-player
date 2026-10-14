"""Запускает главное окно на Qt и сохраняет его снимок (для проверки вида на Linux/macOS/Qt)."""
import os, sys, tempfile, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import webview
import miniplayer as m
from plat import P

out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(tempfile.gettempdir(), "mp_main.png")
tmp = tempfile.mkdtemp(prefix="mp_shot_")
m.DATA_FILE = os.path.join(tmp, "data.json")
P.prepare_env()
main = webview.create_window("Mini Player", html=m.HTML, js_api=m.MainApi(), width=900, height=660)
m.MAIN_WIN.append(main); P.set_main(main)

def run():
    time.sleep(7)
    try:
        P.ui(lambda: main.native.grab().save(out))
        print("saved", out, flush=True)
    finally:
        os._exit(0)

webview.start(run, **P.start_kwargs(os.path.join(tmp, "web")))
