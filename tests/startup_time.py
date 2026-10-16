"""Замеряет, сколько времени уходит на запуск главного окна (WebView2)."""
import os, sys, tempfile, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
t0 = time.time()
import webview
import miniplayer as m
from plat import P
def lg(s): print("%.2f %s" % (time.time() - t0, s), flush=True)
lg("imports done")
tmp = tempfile.mkdtemp(prefix="mp_st_")
m.DATA_FILE = os.path.join(tmp, "data.json")
P.prepare_env()
if os.environ.get('MP_PROXY'):
    import bypass
    px = bypass.Proxy()
    os.environ['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] += ' --proxy-server=http://127.0.0.1:%d --disable-quic' % px.port
    lg('proxy on')
main = webview.create_window("Mini Player", html=m.HTML, js_api=m.MainApi(), width=760, height=620)
m.MAIN_WIN.append(main); P.set_main(main)
for flag, fn in (('KEEP', lambda: m.install_keepalive(main)), ('ICON', lambda: m.set_icon(main)), ('TRACK', lambda: m.track_geometry(main, 'main')), ('DIAG', lambda: m.start_diagnostics(main))):
    if os.environ.get('MP_'+flag): fn(); lg('applied '+flag)
main.events.shown += lambda: lg("shown")
main.events.loaded += lambda: lg("loaded")
def run():
    lg("start callback (gui ready)")
    time.sleep(1.5)
    os._exit(0)
lg("calling webview.start")
webview.start(run, **P.start_kwargs(os.path.join(tmp, "web")))
