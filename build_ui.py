"""Собирает ui.html из ui_template.html (подставляет SVG-иконки из icons/svg)."""
import json, os
here = os.path.dirname(os.path.abspath(__file__))
import base64, io
from PIL import Image
icons = {n: open(f"{here}/icons/svg/{n}.svg", encoding="utf-8").read() for n in ["youtube", "tiktok", "twitch", "other", "fav", "settings"]}
for n in ["youtube", "tiktok", "twitch", "other", "fav", "settings"]:        # готовые картинки (нарисованные иконки) вместо SVG
    clay = f"{here}/icons/{n}_clay.png"
    if os.path.exists(clay):
        buf = io.BytesIO()
        Image.open(clay).convert("RGBA").resize((128, 128), Image.LANCZOS).save(buf, "PNG", optimize=True)
        icons[n] = "data:image/png;base64," + base64.b64encode(buf.getvalue()).decode()
act = f"{here}/icons/fav_active_clay.png"                     # «избранное» в нажатом состоянии
if os.path.exists(act):
    buf = io.BytesIO()
    Image.open(act).convert("RGBA").resize((128, 128), Image.LANCZOS).save(buf, "PNG", optimize=True)
    icons["fav_on"] = "data:image/png;base64," + base64.b64encode(buf.getvalue()).decode()
t = open(f"{here}/ui_template.html", encoding="utf-8").read()
open(f"{here}/ui.html", "w", encoding="utf-8").write(t.replace("__ICONS__", json.dumps(icons, ensure_ascii=False)))
print("ui.html собран")
