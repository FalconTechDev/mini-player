"""Рисует иконки .ico/.png из icons/svg/*.svg (стиль «брутальный клейморфизм»).
SVG растеризуется встроенным WebView2 (тот же движок, что показывает интерфейс). Запуск:  python make_icons.py"""
import base64, io, os, time
import webview
from PIL import Image

here = os.path.dirname(os.path.abspath(__file__))
svg_dir, out = os.path.join(here, "icons", "svg"), os.path.join(here, "icons")
NAMES = ["app", "youtube", "tiktok", "twitch", "other", "fav", "settings"]
SIZES = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]

win = webview.create_window("icons", html="<canvas id=c width=256 height=256></canvas>", width=300, height=300, hidden=True)

JS = """(svg)=>new Promise(res=>{const i=new Image();i.onload=()=>{const c=document.getElementById('c'),x=c.getContext('2d');
 x.clearRect(0,0,256,256);x.imageSmoothingEnabled=false;x.drawImage(i,0,0,256,256);res(c.toDataURL('image/png'))};
 i.src='data:image/svg+xml;base64,'+svg})"""

def work():
    time.sleep(1.5)
    for n in NAMES:
        if os.path.exists(os.path.join(out, n + "_clay.png")):
            continue                              # иконка нарисована вручную (icons/<имя>_clay.png)
        svg = open(os.path.join(svg_dir, n + ".svg"), "rb").read()
        key = "r_" + n
        win.evaluate_js("(%s)('%s').then(d=>{window.%s=d})" % (JS, base64.b64encode(svg).decode(), key))
        data = None
        for _ in range(40):
            time.sleep(0.15)
            data = win.evaluate_js("window.%s||null" % key)
            if data:
                break
        im = Image.open(io.BytesIO(base64.b64decode(data.split(",", 1)[1]))).convert("RGBA")
        im.save(os.path.join(out, n + ".png"))
        im.save(os.path.join(out, n + ".ico"), sizes=SIZES)
    im = Image.open(os.path.join(out, "app.png"))
    im.save(os.path.join(here, "icon.ico"), sizes=SIZES)
    print("иконки готовы:", NAMES)
    win.destroy()

webview.start(work, private_mode=True)
