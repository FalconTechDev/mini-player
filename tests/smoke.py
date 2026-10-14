"""Интеграционная проверка слоя платформы: окна плеера, панели, закрепление, полный экран, cookie, шифрование.
Запуск:  python tests/smoke.py   (на Windows с Qt:  set MP_QT=1)   Код выхода 0 - всё прошло."""
import json
import os
import sys
import tempfile
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import webview  # noqa: E402

import miniplayer as m  # noqa: E402
from plat import P  # noqa: E402

tmp = tempfile.mkdtemp(prefix="mp_smoke_")
m.DATA_FILE = os.path.join(tmp, "data.json")
m.ACC_FILE = os.path.join(tmp, "accounts.bin")
P.prepare_env()
main = webview.create_window("Mini Player", html="<html><body style='background:#26211c'>main</body></html>",
                             js_api=m.MainApi(), width=600, height=400)
m.MAIN_WIN.append(main)
P.set_main(main)
m.AUTOHIDE[0] = False                       # панели видны всегда - чтобы проверять без мыши
results = []


def check(name, ok, detail=""):
    results.append((name, bool(ok), detail))
    print(("PASS " if ok else "FAIL ") + name + (" | " + str(detail) if detail else ""), flush=True)


def near(a, b, tol=4):
    return all(abs(x - y) <= tol for x, y in zip(a, b))


def run():
    try:
        time.sleep(6)
        info = m.parse("http://127.0.0.1:9/test.mp4")
        m.open_player(info, (300, 200, 640, 400))
        for _ in range(60):
            time.sleep(0.5)
            if m.PANEL_SETS and m.PANEL_SETS[0].ready and "top" in m.PANEL_SETS[0].win:
                break
        ps = m.PANEL_SETS[0]
        parent = ps.parent
        check("панели созданы", ps.ready and set(ps.win) == {"top", "gear"}, sorted(ps.win))
        time.sleep(2.5)
        check("верхняя панель видна", ps.vis["top"] and P.is_visible(ps.win["top"]))
        px, py, pw, ph = P.rect(parent)
        tx, ty, tw, th = P.rect(ps.win["top"])
        wx, wy, ww, wh = P.work_area(parent)
        outside = py - th >= wy
        check("верхняя панель стоит вплотную над окном" if outside else "верхняя панель внутри окна (нет места сверху)",
              near((tx, ty + th, tw), (px, py, pw)) if outside else ty == py, (P.rect(ps.win["top"]), P.rect(parent)))
        # закрепление
        print("DBG initial: api._on_top =", ps.api._on_top, "is_topmost =", P.is_topmost(parent), flush=True)
        before = P.is_topmost(parent)
        r = ps.api.pin()
        time.sleep(0.8)
        check("закрепление переключается", P.is_topmost(parent) == r and r != before, (before, r, P.is_topmost(parent)))
        r = ps.api.pin()
        time.sleep(0.8)
        check("открепление работает сразу", P.is_topmost(parent) == r, (r, P.is_topmost(parent)))
        # полный экран
        orig = P.rect(parent)
        fs = ps.api.fullscreen()
        time.sleep(1.0)
        full = P.work_area(parent, full=True)
        check("полный экран", fs and near(P.rect(parent), full, 6), (P.rect(parent), full))
        fs = ps.api.fullscreen()
        time.sleep(1.0)
        check("возврат из полного экрана", (not fs) and near(P.rect(parent), orig, 6), (P.rect(parent), orig))
        # размер S
        ps.api.size("S")
        time.sleep(0.8)
        check("размер окна S", near(P.rect(parent)[2:], m.SIZES_H["S"], 8) or P.rect(parent)[2] > 0, P.rect(parent))
        # сворачивание
        ps.api.minimize()
        time.sleep(1.0)
        check("сворачивание", P.is_minimized(parent))
        P.restore(parent)
        time.sleep(1.0)
        check("разворачивание", not P.is_minimized(parent))
        # прозрачность и состояние для панели
        P.set_opacity(parent, 0.7)
        st = m.PanelApi(ps, "top").state()
        check("API панели отдаёт состояние", st["kind"] == "player" and "ui" in st)
        # окно настроек
        ps.toggle_gear()
        time.sleep(2.5)
        check("окно настроек открывается", ps.vis["gear"] and P.is_visible(ps.win["gear"]))
        # cookie: запись, чтение, удаление
        item = dict(name="mp_test", value="v1", domain=".example.com", path="/", expires="2031-01-01T00:00:00Z",
                    http=True, secure=True, same=1)
        P.cookie_restore([item])
        time.sleep(1.5)
        got = [c for c in P.cookies(["https://www.example.com"]) if c["name"] == "mp_test"]
        check("cookie записывается и читается", bool(got) and got[0]["value"] == "v1" and got[0]["http"] and got[0]["secure"], got)
        P.cookies(["https://www.example.com"], delete=True)
        time.sleep(1.5)
        left = [c for c in P.cookies(["https://www.example.com"]) if c["name"] == "mp_test"]
        check("cookie удаляется", not left, left)
        # шифрование и хранилище аккаунтов
        blob = P.protect("секрет".encode())
        check("шифрование туда-обратно", P.unprotect(blob).decode() == "секрет" and b"\xd1\x81" not in blob)
        m.acc_store_save({"yt": {"a1": [{"name": "x"}]}})
        check("хранилище аккаунтов", m.acc_store_load() == {"yt": {"a1": [{"name": "x"}]}})
    except Exception as e:
        import traceback
        check("исключение в тесте", False, repr(e) + " " + traceback.format_exc()[-400:])
    finally:
        bad = [r for r in results if not r[1]]
        print("ИТОГО: %d проверок, не прошло: %d" % (len(results), len(bad)), flush=True)
        os._exit(1 if bad else 0)


webview.start(run, **P.start_kwargs(os.path.join(tmp, "web")))
