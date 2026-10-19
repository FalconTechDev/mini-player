"""Установщик Mini Player: ставит приложение, ярлыки, запись об удалении и (при необходимости) среду WebView2.
Запуск без параметров — окно установки. Параметры: --silent (без окон), --dir <папка>, --no-desktop, --no-startmenu,
--autostart, --no-launch, --uninstall [--remove-data]."""
import os
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
import winreg

APP = "Mini Player"
VERSION = "2.0.1"
# Что нового по версиям (показывается при обновлении: все версии новее установленной)
CHANGES = [
    ("1.0.0", ["Первый выпуск: мини-плеер для YouTube, TikTok, Twitch, Vimeo, mp4 и любых сайтов",
               "Панели выезжают снаружи окна, поиск, аккаунты, «Продолжить просмотр»",
               "Установщик, ярлыки и удаление"]),
    ("1.0.1", ["Исправлено масштабирование интерфейса на экранах со шкалой 125%, 150% и больше: панели больше не обрезаются"]),
    ("1.1.0", ["Новая вкладка Netflix после YouTube (ссылки на фильмы и сериалы, кнопка «Войти»)",
               "Режим обхода блокировок в настройках: работает только внутри приложения, включается без перезапуска",
               "Права администратора для обхода запрашиваются один раз за запуск и только если режим включён",
               "Установщик теперь определяет установленную версию и предлагает обновление со списком изменений",
               "Исправлен вылет при перезапуске приложения"]),
    ("1.1.1", ["Настройка для YouTube: после закрытия видео открывать ленту или главное окно с поиском (в настройках окна и в главных настройках)",
               "Обход блокировок: пока режим выключен, сеть работает как обычно (без прокси), из-за чего YouTube через VPN снова открывается быстро",
               "Обход: заставка «Подождите» по центру с шагами и итогом, запрос прав администратора идёт первым, проверки сокращены",
               "Права администратора для обхода не запрашиваются повторно при перезапуске приложения",
               "Обход не затрагивает TikTok и Netflix",
               "Кнопка «Войти» на вкладке Netflix выглядит как у остальных платформ"]),
    ("1.1.2", ["Запуск стал быстрее: окно появляется за секунду, в том числе при включённом обходе (раньше до 10 секунд)",
               "Приложение больше не распаковывается при каждом запуске (обычная папка вместо одного файла)",
               "Исправлено запоминание окон: после перезапуска они больше не открываются на весь экран, не уезжают за экран и не прыгают",
               "Новый установщик заменяет файлы целиком, лишние остатки старой версии удаляются"]),
    ("2.0.0", ["Приложение полностью переписано на Rust: запуск за доли секунды, меньше памяти, без распаковки при каждом запуске",
               "Данные, настройки, история и аккаунты сохраняются: формат файлов прежний",
               "Панели, окна и запоминание позиции переписаны: окна больше не открываются на весь экран и не прыгают",
               "Режим обхода, вкладка Netflix, поиск и аккаунты работают как раньше"]),
    ("2.0.1", ["Новая иконка приложения: в окне, на панели задач, в установщике и ярлыках"]),
]
EXE = "MiniPlayer.exe"
UNINST_ID = os.environ.get("MP_INSTALL_KEY", "MiniPlayer")
UNINST_KEY = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\%s" % UNINST_ID
RUN_KEY = r"Software\Microsoft\Windows\CurrentVersion\Run"
RUN_NAME = os.environ.get("MP_RUN_NAME", "MiniPlayer")
LNK_NAME = os.environ.get("MP_LNK_NAME", APP)
WV2_GUID = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
WV2_URL = "https://go.microsoft.com/fwlink/p/?LinkId=2124703"      # официальный загрузчик Microsoft Edge WebView2 Runtime
NOWIN = 0x08000000
DEFAULT_DIR = os.path.join(os.environ.get("LOCALAPPDATA", os.path.expanduser("~")), "Programs", APP)
DATA_DIR = os.path.join(os.environ.get("APPDATA", os.path.expanduser("~")), "MiniPlayer")


def res(name):
    base = getattr(sys, "_MEIPASS", os.path.dirname(os.path.abspath(__file__)))
    return os.path.join(base, name)


def run(cmd, **kw):
    return subprocess.run(cmd, creationflags=NOWIN, capture_output=True, **kw)


def ps(script):
    return run(["powershell", "-NoProfile", "-NonInteractive", "-Command", script])


def kill_app():
    if os.environ.get("MP_INSTALL_KEY"):          # тестовый запуск: настоящее приложение не трогаем
        return
    run(["taskkill", "/F", "/T", "/IM", EXE])
    time.sleep(1.5)


def webview2_installed():
    keys = [(winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\%s" % WV2_GUID),
            (winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\EdgeUpdate\Clients\%s" % WV2_GUID),
            (winreg.HKEY_CURRENT_USER, r"Software\Microsoft\EdgeUpdate\Clients\%s" % WV2_GUID)]
    for hive, key in keys:
        try:
            with winreg.OpenKey(hive, key) as k:
                v, _ = winreg.QueryValueEx(k, "pv")
                if v and v != "0.0.0.0":
                    return True
        except OSError:
            continue
    return False


def install_webview2(log):
    """Скачивает и тихо ставит WebView2 Runtime (без него окна приложения не откроются). -> True/False."""
    try:
        log("Загружаю среду WebView2 (Microsoft)…")
        path = os.path.join(tempfile.gettempdir(), "MicrosoftEdgeWebview2Setup.exe")
        req = urllib.request.Request(WV2_URL, headers={"User-Agent": "Mozilla/5.0"})
        with urllib.request.urlopen(req, timeout=60) as r, open(path, "wb") as f:
            f.write(r.read())
        log("Устанавливаю WebView2 (может появиться запрос Windows)…")
        subprocess.run([path, "/silent", "/install"], creationflags=NOWIN, timeout=600)
        return webview2_installed()
    except Exception:
        return False


def make_shortcuts(install_dir, desktop, startmenu):
    exe = os.path.join(install_dir, EXE)
    lines = ["$w=New-Object -ComObject WScript.Shell"]
    for flag, folder in ((desktop, "Desktop"), (startmenu, "Programs")):
        if flag:
            lines += ["$d=[Environment]::GetFolderPath('%s')" % folder,
                      "$s=$w.CreateShortcut(\"$d\\%s.lnk\")" % LNK_NAME,
                      "$s.TargetPath='%s'" % exe, "$s.WorkingDirectory='%s'" % install_dir,
                      "$s.IconLocation='%s,0'" % exe, "$s.Description='%s'" % APP, "$s.Save()"]
    if len(lines) > 1:
        ps("; ".join(lines))


def remove_shortcuts():
    ps("foreach($f in 'Desktop','Programs'){$p=Join-Path ([Environment]::GetFolderPath($f)) '%s.lnk';"
       "if(Test-Path $p){Remove-Item $p -Force}}" % LNK_NAME)


def vtuple(v):
    try:
        return tuple(int(x) for x in str(v).split("."))
    except ValueError:
        return (0,)


def installed_info():
    """-> (версия, папка) установленного приложения или None."""
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, UNINST_KEY) as k:
            ver = winreg.QueryValueEx(k, "DisplayVersion")[0]
            d = winreg.QueryValueEx(k, "InstallLocation")[0]
    except OSError:
        return None
    if not d or not os.path.exists(os.path.join(d, EXE)):
        return None
    return ver, d


def changes_since(ver):
    return [(v, items) for v, items in CHANGES if vtuple(v) > vtuple(ver)]


def shortcut_exists(folder):
    r = ps("Test-Path (Join-Path ([Environment]::GetFolderPath('%s')) '%s.lnk')" % (folder, LNK_NAME))
    return r.stdout.decode(errors="ignore").strip().lower() == "true"


def autostart_set():
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, RUN_KEY) as k:
            winreg.QueryValueEx(k, RUN_NAME)
            return True
    except OSError:
        return False


def dir_size_kb(path):
    total = 0
    for root, _, files in os.walk(path):
        for f in files:
            try:
                total += os.path.getsize(os.path.join(root, f))
            except OSError:
                pass
    return total // 1024


def do_install(install_dir, desktop=True, startmenu=True, autostart=False, log=lambda s: None, progress=lambda v: None):
    steps = 7
    n = 0

    def step(msg):
        nonlocal n
        n += 1
        log(msg)
        progress(n / steps * 100)

    step("Закрываю запущенное приложение…")
    kill_app()
    step("Проверяю среду WebView2…")
    wv_ok = webview2_installed() or install_webview2(log)
    step("Копирую файлы приложения…")
    os.makedirs(install_dir, exist_ok=True)
    import shutil
    import zipfile
    for name in os.listdir(install_dir):                     # старые файлы убираем (кроме программы удаления), чтобы не оставались лишние
        if name.lower() != "uninstall.exe":
            p = os.path.join(install_dir, name)
            try:
                shutil.rmtree(p) if os.path.isdir(p) else os.remove(p)
            except OSError:
                pass
    with zipfile.ZipFile(res(os.path.join("payload", "app.zip"))) as z:
        z.extractall(install_dir)
    step("Добавляю программу удаления…")
    uninstaller = os.path.join(install_dir, "uninstall.exe")
    if getattr(sys, "frozen", False):
        with open(sys.executable, "rb") as src, open(uninstaller, "wb") as dst:
            dst.write(src.read())
    step("Создаю ярлыки…")
    make_shortcuts(install_dir, desktop, startmenu)
    step("Регистрирую в «Установке и удалении программ»…")
    exe = os.path.join(install_dir, EXE)
    with winreg.CreateKey(winreg.HKEY_CURRENT_USER, UNINST_KEY) as k:
        for name, val in (("DisplayName", APP), ("DisplayVersion", VERSION), ("Publisher", "Mini Player"),
                          ("InstallLocation", install_dir), ("DisplayIcon", exe),
                          ("UninstallString", '"%s" --uninstall' % uninstaller),
                          ("QuietUninstallString", '"%s" --uninstall --silent' % uninstaller)):
            winreg.SetValueEx(k, name, 0, winreg.REG_SZ, val)
        winreg.SetValueEx(k, "NoModify", 0, winreg.REG_DWORD, 1)
        winreg.SetValueEx(k, "NoRepair", 0, winreg.REG_DWORD, 1)
        winreg.SetValueEx(k, "EstimatedSize", 0, winreg.REG_DWORD, dir_size_kb(install_dir))
    with winreg.CreateKey(winreg.HKEY_CURRENT_USER, RUN_KEY) as k:
        if autostart:
            winreg.SetValueEx(k, RUN_NAME, 0, winreg.REG_SZ, '"%s"' % exe)
        else:
            try:
                winreg.DeleteValue(k, RUN_NAME)
            except OSError:
                pass
    step("Готово.")
    return wv_ok


def do_uninstall(remove_data=False, log=lambda s: None):
    install_dir = None
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, UNINST_KEY) as k:
            install_dir = winreg.QueryValueEx(k, "InstallLocation")[0]
    except OSError:
        pass
    log("Закрываю приложение…")
    kill_app()
    log("Удаляю ярлыки…")
    remove_shortcuts()
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, RUN_KEY, 0, winreg.KEY_SET_VALUE) as k:
            winreg.DeleteValue(k, RUN_NAME)
    except OSError:
        pass
    try:
        winreg.DeleteKey(winreg.HKEY_CURRENT_USER, UNINST_KEY)
    except OSError:
        pass
    if remove_data and os.path.isdir(DATA_DIR) and not os.environ.get("MP_INSTALL_KEY"):
        log("Удаляю данные приложения…")
        subprocess.run(["cmd", "/c", "rmdir", "/s", "/q", DATA_DIR], creationflags=NOWIN)
    if install_dir and os.path.isdir(install_dir):
        log("Удаляю файлы…")
        # сам uninstall.exe сейчас запущен — папку убираем отложенной командой после выхода
        subprocess.Popen('cmd /c ping 127.0.0.1 -n 4 >nul & rmdir /s /q "%s"' % install_dir, shell=True, creationflags=NOWIN)
    log("Готово.")


def parse_args(argv):
    a = {"silent": False, "dir": DEFAULT_DIR, "desktop": True, "startmenu": True, "autostart": False,
         "launch": True, "uninstall": False, "remove_data": False}
    i = 0
    while i < len(argv):
        t = argv[i].lower()
        if t in ("--silent", "/s", "/silent"):
            a["silent"] = True
        elif t == "--dir" and i + 1 < len(argv):
            a["dir"] = argv[i + 1]
            i += 1
        elif t == "--no-desktop":
            a["desktop"] = False
        elif t == "--no-startmenu":
            a["startmenu"] = False
        elif t == "--autostart":
            a["autostart"] = True
        elif t == "--no-launch":
            a["launch"] = False
        elif t == "--uninstall":
            a["uninstall"] = True
        elif t == "--remove-data":
            a["remove_data"] = True
        i += 1
    return a


# ---------------- окно установщика ----------------
BG, PANEL, TXT, DIM, ACC = "#26211c", "#1b1814", "#f3ece0", "#a99f8e", "#ffc83d"


def gui(args):
    import tkinter as tk
    from tkinter import filedialog, messagebox, ttk

    root = tk.Tk()
    root.title(APP + (" — удаление" if args["uninstall"] else " — установка"))
    root.configure(bg=BG)
    root.geometry("560x440")
    root.resizable(False, False)
    try:
        root.iconbitmap(res("app.ico"))
    except Exception:
        pass
    F = ("Segoe UI", 10)
    head = tk.Frame(root, bg=ACC, height=70, highlightbackground="#000", highlightthickness=3)
    head.pack(fill="x")
    head.pack_propagate(False)
    tk.Label(head, text=APP, bg=ACC, fg="#000", font=("Segoe UI", 20, "bold")).pack(side="left", padx=18)
    tk.Label(head, text="Удаление" if args["uninstall"] else "Установка", bg=ACC, fg="#000",
             font=("Segoe UI", 11, "bold")).pack(side="right", padx=18)
    body = tk.Frame(root, bg=BG)
    body.pack(fill="both", expand=True, padx=22, pady=14)
    style = ttk.Style()
    style.theme_use("clam")
    style.configure("Y.Horizontal.TProgressbar", troughcolor=PANEL, background=ACC, bordercolor="#000", lightcolor=ACC, darkcolor=ACC)
    status = tk.StringVar(value="")

    def label(text, color=TXT, bold=False):
        return tk.Label(body, text=text, bg=BG, fg=color, font=("Segoe UI", 10, "bold" if bold else "normal"),
                        anchor="w", justify="left", wraplength=510)

    def button(parent, text, cmd, primary=False):
        return tk.Button(parent, text=text, command=cmd, font=("Segoe UI", 10, "bold"), bg=ACC if primary else "#3a342c",
                         fg="#000" if primary else TXT, relief="flat", bd=0, padx=18, pady=7, cursor="hand2",
                         activebackground="#ffe08a" if primary else "#443d33", highlightbackground="#000", highlightthickness=2)

    def clear():
        for w in body.winfo_children():
            w.destroy()

    def check(text, var):
        return tk.Checkbutton(body, text=text, variable=var, bg=BG, fg=TXT, selectcolor=PANEL, activebackground=BG,
                              activeforeground=TXT, font=F, wraplength=500, justify="left")

    # --- удаление
    if args["uninstall"]:
        rm = tk.BooleanVar(value=False)
        label("Удалить Mini Player с этого компьютера?", bold=True).pack(anchor="w")
        label("Будут удалены программа, ярлыки и запись в списке программ Windows.", DIM).pack(anchor="w", pady=(4, 10))
        check("Удалить также настройки, историю и сохранённые входы в аккаунты", rm).pack(anchor="w")
        row = tk.Frame(body, bg=BG)
        row.pack(side="bottom", fill="x", pady=8)

        def go_un():
            clear()
            label("Удаляю…", bold=True).pack(anchor="w", pady=(0, 10))
            tk.Label(body, textvariable=status, bg=BG, fg=DIM, font=F, anchor="w").pack(anchor="w")

            def work():
                do_uninstall(rm.get(), lambda m: root.after(0, lambda: status.set(m)))
                root.after(0, lambda: (messagebox.showinfo(APP, "Mini Player удалён."), root.destroy()))
            threading.Thread(target=work, daemon=True).start()
        button(row, "Удалить", go_un, True).pack(side="right")
        button(row, "Отмена", root.destroy).pack(side="right", padx=8)
        root.mainloop()
        return

    # --- установка или обновление
    inst = installed_info()
    v_dir = tk.StringVar(value=args["dir"])
    v_desk, v_menu = tk.BooleanVar(value=args["desktop"]), tk.BooleanVar(value=args["startmenu"])
    v_auto, v_launch = tk.BooleanVar(value=args["autostart"]), tk.BooleanVar(value=True)
    label("Добро пожаловать!", bold=True).pack(anchor="w")
    label("Установщик поставит всё нужное: приложение (Python не требуется), ярлыки, программу удаления и среду WebView2 "
          "от Microsoft, если её ещё нет в Windows.", DIM).pack(anchor="w", pady=(2, 10))
    label("Папка установки:").pack(anchor="w")
    r1 = tk.Frame(body, bg=BG)
    r1.pack(fill="x", pady=(2, 8))
    tk.Entry(r1, textvariable=v_dir, bg=PANEL, fg=TXT, insertbackground=TXT, relief="flat", font=F,
             highlightbackground="#000", highlightthickness=2).pack(side="left", fill="x", expand=True, ipady=5)

    def browse():
        p = filedialog.askdirectory()
        if p:
            v_dir.set(os.path.join(p, APP))
    button(r1, "Обзор…", browse).pack(side="left", padx=(8, 0))
    for var, text in ((v_desk, "Ярлык на рабочем столе"), (v_menu, "Ярлык в меню «Пуск»"),
                      (v_auto, "Запускать вместе с Windows"), (v_launch, "Запустить после установки")):
        check(text, var).pack(anchor="w")
    row = tk.Frame(body, bg=BG)
    row.pack(side="bottom", fill="x", pady=8)

    def go(target=None, desk=None, menu=None, auto=None, launch=None, title="Устанавливаю…"):
        target = (target or v_dir.get()).strip()
        if not target:
            return
        desk = v_desk.get() if desk is None else desk
        menu = v_menu.get() if menu is None else menu
        auto = v_auto.get() if auto is None else auto
        launch = v_launch.get() if launch is None else launch
        clear()
        label(title, bold=True).pack(anchor="w", pady=(0, 10))
        prog = ttk.Progressbar(body, style="Y.Horizontal.TProgressbar", length=500, mode="determinate")
        prog.pack(fill="x")
        tk.Label(body, textvariable=status, bg=BG, fg=DIM, font=F, anchor="w", wraplength=500, justify="left").pack(anchor="w", pady=10)

        def work():
            try:
                wv_ok = do_install(target, desk, menu, auto,
                                   lambda m: root.after(0, lambda: status.set(m)),
                                   lambda v: root.after(0, lambda: prog.configure(value=v)))
            except Exception as e:
                root.after(0, lambda: (messagebox.showerror(APP, "Не удалось установить:\n%s" % e), root.destroy()))
                return

            def finish():
                msg = "Mini Player обновлён до версии %s." % VERSION if title.startswith("Обновляю") else "Mini Player установлен."
                if not wv_ok:
                    msg += ("\n\nНе удалось поставить среду WebView2 (нет интернета или отказ в доступе). Приложению она нужна: "
                            "скачайте «WebView2 Runtime» на сайте Microsoft и запустите установщик ещё раз.")
                messagebox.showinfo(APP, msg)
                if launch:
                    subprocess.Popen([os.path.join(target, EXE)], cwd=target)
                root.destroy()
            root.after(0, finish)
        threading.Thread(target=work, daemon=True).start()
    button(row, "Установить", go, True).pack(side="right")
    button(row, "Отмена", root.destroy).pack(side="right", padx=8)

    if inst:                                                 # уже установлено: проверяем версию
        iv, idir = inst
        clear()
        newer = vtuple(iv) > vtuple(VERSION)
        same = vtuple(iv) == vtuple(VERSION)
        if newer:
            label("Установлена более новая версия", bold=True).pack(anchor="w")
            label("У вас %s, а этот установщик содержит %s. Обновлять не нужно." % (iv, VERSION), DIM).pack(anchor="w", pady=(4, 10))
        elif same:
            label("Установлена последняя версия (%s)" % iv, bold=True).pack(anchor="w")
            label("Обновлять не нужно. Можно переустановить, если что-то сломалось.", DIM).pack(anchor="w", pady=(4, 10))
        else:
            label("Доступно обновление: %s → %s" % (iv, VERSION), bold=True).pack(anchor="w")
            label("Что нового:", DIM).pack(anchor="w", pady=(4, 4))
            box = tk.Text(body, bg=PANEL, fg=TXT, relief="flat", font=F, wrap="word", height=11, padx=10, pady=8,
                          highlightbackground="#000", highlightthickness=2)
            for v, items in reversed(changes_since(iv)):
                box.insert("end", "Версия %s" % v + chr(10), "h")
                for it in items:
                    box.insert("end", "  • %s" % it + chr(10))
                box.insert("end", chr(10))
            box.tag_configure("h", foreground=ACC, font=("Segoe UI", 10, "bold"))
            box.configure(state="disabled")
            box.pack(fill="both", expand=True)
        r2 = tk.Frame(body, bg=BG)
        r2.pack(side="bottom", fill="x", pady=8)

        def upd():
            go(idir, shortcut_exists("Desktop"), shortcut_exists("Programs"), autostart_set(), True, "Обновляю до %s…" % VERSION)
        button(r2, "Переустановить" if (same or newer) else "Обновить", upd, not (same or newer)).pack(side="right")
        button(r2, "Закрыть", root.destroy).pack(side="right", padx=8)
    root.mainloop()


def main():
    args = parse_args(sys.argv[1:])
    if args["silent"]:
        if args["uninstall"]:
            do_uninstall(args["remove_data"])
        else:
            inst = installed_info()
            if inst and not any(a.lower() == "--dir" for a in sys.argv[1:]):       # уже установлено: обновляем на месте
                if vtuple(inst[0]) >= vtuple(VERSION):
                    return
                args["dir"] = inst[1]
                args["desktop"], args["startmenu"], args["autostart"] = shortcut_exists("Desktop"), shortcut_exists("Programs"), autostart_set()
            do_install(args["dir"], args["desktop"], args["startmenu"], args["autostart"])
            if args["launch"]:
                subprocess.Popen([os.path.join(args["dir"], EXE)], cwd=args["dir"])
        return
    gui(args)


if __name__ == "__main__":
    main()
