//! Методы, которые страницы вызывают как `pywebview.api.<имя>(...)`. Выполняются в отдельных потоках.
use crate::state::*;
use crate::{accounts, bypass, data, geometry, net, panels, server, shim, winapi};
use serde_json::{json, Value};
use std::sync::Mutex;

const SIZES_H: [(&str, (i32, i32)); 3] = [("S", (400, 300)), ("M", (560, 380)), ("L", (800, 520))];
const SIZES_V: [(&str, (i32, i32)); 3] = [("S", (300, 560)), ("M", (380, 720)), ("L", (460, 860))];

fn size_of(vertical: bool, key: &str) -> (i32, i32) {
    let t = if vertical { &SIZES_V } else { &SIZES_H };
    t.iter().find(|(k, _)| *k == key).map(|(_, s)| *s).unwrap_or(t[1].1)
}

fn arg_s(a: &[Value], i: usize) -> String {
    a.get(i).and_then(|v| v.as_str()).unwrap_or("").to_string()
}
fn arg_b(a: &[Value], i: usize) -> bool {
    a.get(i).map(|v| v.as_bool().unwrap_or(false)).unwrap_or(false)
}
fn arg_f(a: &[Value], i: usize) -> f64 {
    a.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0)
}

pub fn handle_ipc(id: WinId, role: Role, origin_ok: bool, body: String) {
    std::thread::spawn(move || {
        let Ok(v) = serde_json::from_str::<Value>(&body) else { return };
        let cid = v["id"].as_u64().unwrap_or(0);
        let name = v["name"].as_str().unwrap_or("").to_string();
        let args = v["args"].as_array().cloned().unwrap_or_default();
        let js = match call(id, &role, origin_ok, &name, &args) {
            Ok(val) => format!("__mpReply({cid},true,{val})"),
            Err(e) => format!("__mpReply({cid},false,{})", json!(e)),
        };
        eval(id, &js);
    });
}

pub fn call(id: WinId, role: &Role, origin_ok: bool, name: &str, a: &[Value]) -> Result<Value, String> {
    match role {
        Role::Main if origin_ok => main_api(name, a),
        Role::Player if origin_ok => player_api(id, name, a),
        Role::Panel if origin_ok => panel_api(id, name, a),
        Role::Splash if origin_ok => match name {
            "bypass_info" => Ok(bypass_info()),
            "splash_close" => {
                send(Ev::Close(id));
                Ok(Value::Null)
            }
            _ => Err("нет такого метода".into()),
        },
        Role::Site(_) => match name {
            "open_video" => {
                site_open_video(id, &arg_s(a, 0));
                Ok(Value::Null)
            }
            _ => Err("запрещено".into()),
        },
        _ => Err("запрещено".into()),
    }
}

// ---------- окна ----------
pub fn open_main() {
    let (size, pos) = geometry::restore("main", 760, 620);
    let mut sp = Spec::new(Role::Main, format!("http://127.0.0.1:{}/ui", port()));
    sp.size = size;
    sp.pos = pos;
    sp.min = (640, 480);
    sp.geom_key = Some("main".into());
    app().main = Some(sp.id);
    send(Ev::Create(sp));
}

pub fn notify_main() {
    if let Some(m) = app().main {
        eval(m, "window.__mpSync&&window.__mpSync()");
    }
}

fn icon_for(platform: &str) -> &'static str {
    match platform {
        "YouTube" => "youtube",
        "Twitch" => "twitch",
        "TikTok" => "tiktok",
        "Netflix" => "netflix",
        _ => "other",
    }
}

pub fn open_player(info: &Value, geom: Option<[i32; 4]>) {
    let d = data::load();
    let st = &d["settings"];
    let vertical = info["vertical"].as_bool().unwrap_or(false);
    let (w, h) = size_of(vertical, st["size"].as_str().unwrap_or("M"));
    let on_top = st["on_top"].as_bool().unwrap_or(true);
    let platform = info["platform"].as_str().unwrap_or("").to_string();
    let title = info["title"].as_str().unwrap_or("").to_string();
    if info["kind"] == "web" {
        let mut sp = Spec::new(Role::Plain, info["src"].as_str().unwrap_or("").to_string());
        sp.title = title;
        sp.size = (w, h);
        sp.on_top = on_top;
        sp.min = (240, 150);
        send(Ev::Create(sp));
        return;
    }
    let mut extra = format!("&start={}", info["start"].as_i64().unwrap_or(0));
    if platform == "YouTube" {
        extra += &format!("&vid={}&list={}", info["vid"].as_str().unwrap_or(""), info["list"].as_str().unwrap_or(""));
    }
    let url = format!(
        "http://127.0.0.1:{}/?kind={}&src={}&title={}&platform={}{}",
        port(),
        info["kind"].as_str().unwrap_or("iframe"),
        urlencoding::encode(info["src"].as_str().unwrap_or("")),
        urlencoding::encode(&title),
        urlencoding::encode(&platform),
        extra
    );
    let key = if platform == "YouTube" { "youtube" } else { "player" };
    let mut sp = Spec::new(Role::Player, url);
    sp.title = format!("{platform} - Mini Player");
    let (size, pos) = geometry::restore(key, w, h);
    sp.size = size;
    sp.pos = pos;
    if let Some(g) = geom {                                   // открыто из окна сайта: точно на его месте и того же размера
        let k = winapi::system_scale();
        sp.size = ((g[2] as f64 / k) as i32, (g[3] as f64 / k) as i32);
        sp.pos = Some(((g[0] as f64 / k) as i32, (g[1] as f64 / k) as i32));
        sp.align = Some((g[0], g[1], g[2], g[3]));
    }
    sp.on_top = on_top;
    sp.min = (300, 220);
    sp.icon = Some(icon_for(&platform).into());
    sp.frameless_caption = true;
    sp.opacity = d["settings"]["ui"]["opacity"].as_f64();
    sp.keepalive = true;
    sp.geom_key = Some(key.into());
    sp.panels = Some(("player".into(), platform, title));
    let canon = info["canonical"].as_str().unwrap_or("").to_string();
    if !claim(&format!("player:{canon}"), sp.id) {
        return;
    }
    app().players.insert(sp.id, Player { info: info.clone(), on_top, fs: false, saved: (0, 0, 0, 0), closed: false });
    send(Ev::Create(sp));
}

pub fn tiktok_js() -> String {
    let st = data::load()["settings"]["tiktok"].clone();
    format!("window.__mpset={st};if(window.__mpApply)window.__mpApply();{}", server::TIKTOK_JS)
}

pub fn open_tiktok(url: &str) {
    let d = data::load();
    let st = &d["settings"];
    let (w, h) = size_of(true, st["size"].as_str().unwrap_or("M"));
    let on_top = st["on_top"].as_bool().unwrap_or(true);
    let mut sp = Spec::new(Role::Tiktok, url.to_string());
    sp.title = "TikTok - Mini Player".into();
    let (size, pos) = geometry::restore("tiktok", w, h);
    sp.size = size;
    sp.pos = pos;
    sp.on_top = on_top;
    sp.min = (320, 240);
    sp.icon = Some("tiktok".into());
    sp.frameless_caption = true;
    sp.opacity = d["settings"]["ui"]["opacity"].as_f64();
    sp.keepalive = true;
    sp.geom_key = Some("tiktok".into());
    sp.on_load = OnLoad::Tiktok;
    sp.panels = Some(("tt".into(), "TikTok".into(), "TikTok".into()));
    if !claim(&format!("tiktok:{url}"), sp.id) {
        return;
    }
    app().players.insert(sp.id, Player { info: json!({"vertical": true, "canonical": url}), on_top, fs: false, saved: (0, 0, 0, 0), closed: false });
    send(Ev::Create(sp));
}

pub fn open_site(url: &str, vertical: bool) {
    if url.contains("tiktok.com") {
        return open_tiktok(url);
    }
    let d = data::load();
    let st = &d["settings"];
    let (mut w, mut h) = size_of(vertical, st["size"].as_str().unwrap_or("M"));
    if !vertical {
        w += 120;
        h += 80;
    }
    let on_top = st["on_top"].as_bool().unwrap_or(true);
    let host = url.splitn(4, '/').nth(2).unwrap_or("").to_lowercase();
    let kind = if host.contains("youtube.com") { "yt" } else if host.contains("twitch.tv") { "tw" } else { "" };
    if kind.is_empty() {
        let mut sp = Spec::new(Role::Plain, url.to_string());
        sp.size = (w, h);
        sp.on_top = on_top;
        sp.min = (300, 300);
        send(Ev::Create(sp));
        return;
    }
    let mut sp = Spec::new(Role::Site(kind.to_string()), url.to_string());
    let (size, pos) = geometry::restore("site", w, h);
    sp.size = size;
    sp.pos = pos;
    sp.on_top = on_top;
    sp.min = (300, 300);
    sp.geom_key = Some("site".into());
    sp.on_load = OnLoad::Site(kind.to_string());
    if !claim(&format!("site:{url}"), sp.id) {
        return;
    }
    send(Ev::Create(sp));
}

fn site_open_video(site: WinId, url: &str) {
    let geom = hwnd_of(site).map(|h| {
        let c = winapi::client_rect(h);
        [c.0, c.1, c.2, c.3]
    });
    let r = play(url, "", false, geom);
    if r.get("ok").is_some() {
        notify_main();
        if let Some(h) = hwnd_of(site) {
            winapi::close(h);
        }
    }
}

/// Вызывается, когда окно закрыто: убирает плеер и возвращает окна панелей, которые тоже нужно закрыть.
pub fn on_window_closed(id: WinId) -> Vec<WinId> {
    forget(id);
    let is_player = app().players.remove(&id).is_some();
    if is_player {
        return panels::destroy(id);
    }
    vec![]
}

// ---------- настройки ----------
fn windows_with(roles: &[&str]) -> Vec<(WinId, isize, Role)> {
    app()
        .wins
        .iter()
        .filter(|(_, w)| {
            let r = match &w.role {
                Role::Player => "player",
                Role::Tiktok => "tiktok",
                Role::Panel => "panel",
                _ => "",
            };
            roles.contains(&r)
        })
        .map(|(id, w)| (*id, w.hwnd, w.role.clone()))
        .collect()
}

pub fn update_ui(key: &str, value: Value) -> Value {
    let d = data::update(|d| {
        d["settings"]["ui"][key] = value.clone();
        d.clone()
    });
    if key == "autohide_bar" {
        app().autohide = value.as_bool().unwrap_or(true);
    }
    let ui = d["settings"]["ui"].clone();
    for (id, hwnd, role) in windows_with(&["player", "tiktok", "panel"]) {
        if key == "opacity" && role != Role::Panel {
            winapi::set_opacity(hwnd, value.as_f64().unwrap_or(1.0));
        }
        eval(id, &format!("window.__mpUi&&window.__mpUi({ui})"));
    }
    notify_main();
    d
}

pub fn update_tt(key: &str, value: Value) -> Value {
    let d = data::update(|d| {
        d["settings"]["tiktok"][key] = value.clone();
        d.clone()
    });
    for (id, _, role) in windows_with(&["tiktok", "panel"]) {
        if role == Role::Tiktok {
            eval(id, &tiktok_js());
        } else {
            eval(id, "window.__mpRefresh&&window.__mpRefresh()");
        }
    }
    notify_main();
    d
}

// ---------- API окна плеера / TikTok ----------
fn player_hwnd(id: WinId) -> Result<isize, String> {
    hwnd_of(id).ok_or_else(|| "окно закрыто".to_string())
}

fn open_in_browser(url: &str) {
    let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
}

pub fn p_pin(id: WinId) -> Value {
    let hwnd = match hwnd_of(id) {
        Some(h) => h,
        None => return json!(false),
    };
    let on = {
        let mut a = app();
        match a.players.get_mut(&id) {
            Some(p) => {
                p.on_top = !p.on_top;
                p.on_top
            }
            None => return json!(false),
        }
    };
    winapi::topmost(hwnd, on);
    json!(on)
}

fn p_size(id: WinId, key: &str) {
    let Some(hwnd) = hwnd_of(id) else { return };
    let vertical = app().players.get(&id).map(|p| p.info["vertical"].as_bool().unwrap_or(false)).unwrap_or(false);
    let (w, h) = size_of(vertical, key);
    let k = winapi::scale(hwnd);
    winapi::place(hwnd, None, None, Some((w as f64 * k) as i32), Some((h as f64 * k) as i32));
}

fn p_close_button(id: WinId) {
    let ui = data::load()["settings"]["ui"].clone();
    let is_yt = app().players.get(&id).map(|p| p.info["platform"] == "YouTube").unwrap_or(false);
    if is_yt && ui["yt_exit"].as_bool().unwrap_or(false) {
        if ui["yt_exit_to"] == "menu" {
            p_to_menu(id)
        } else {
            p_to_feed(id)
        }
    } else if let Some(h) = hwnd_of(id) {
        winapi::close(h);
    }
}

fn p_to_menu(id: WinId) {
    if let Some(m) = app().main.and_then(|m| hwnd_of_locked(m)) {
        if winapi::is_alive(m) {
            winapi::restore(m);
            winapi::topmost(m, true);
            winapi::topmost(m, false);
        }
    }
    if let Some(h) = hwnd_of(id) {
        winapi::close(h);
    }
}

fn hwnd_of_locked(id: WinId) -> Option<isize> {
    hwnd_of(id)
}

fn p_to_feed(id: WinId) {
    open_site("https://www.youtube.com", false);
    if let Some(h) = hwnd_of(id) {
        winapi::close(h);
    }
}

fn p_fullscreen(id: WinId) -> Value {
    let Some(hwnd) = hwnd_of(id) else { return json!(false) };
    let mut a = app();
    let Some(p) = a.players.get_mut(&id) else { return json!(false) };
    if !p.fs {
        p.saved = winapi::rect(hwnd);
        p.fs = true;
        let (x, y, w, h) = winapi::work_area(hwnd, true);
        winapi::topmost(hwnd, true);
        winapi::place(hwnd, Some(x), Some(y), Some(w), Some(h));
    } else {
        p.fs = false;
        winapi::place_rect(hwnd, p.saved);
        winapi::topmost(hwnd, p.on_top);
    }
    json!(p.fs)
}

fn p_drag(id: WinId) {
    let Some(hwnd) = hwnd_of(id) else { return };
    if app().players.get(&id).map(|p| p.fs).unwrap_or(false) {
        return;
    }
    winapi::drag(hwnd, || panels::reposition(id));
}

fn p_fit(id: WinId, vw: f64, vh: f64, inner_w: f64, inner_h: f64) {
    let Some(hwnd) = hwnd_of(id) else { return };
    if vw <= 0.0 {
        return;
    }
    let (_, _, ww, hh) = winapi::rect(hwnd);
    let k = winapi::scale(hwnd);
    let mut target = (inner_w * vh / vw) as i32 + (hh - inner_h as i32);
    let max = winapi::work_area(hwnd, false).3 - (20.0 * k) as i32;
    target = target.min(max).max((260.0 * k) as i32);
    if (target - hh).abs() > 4 {
        winapi::place(hwnd, None, None, Some(ww), Some(target));
    }
}

fn player_api(id: WinId, name: &str, a: &[Value]) -> Result<Value, String> {
    let _ = player_hwnd(id)?;
    Ok(match name {
        "is_pinned" => json!(app().players.get(&id).map(|p| p.on_top).unwrap_or(false)),
        "pin" => p_pin(id),
        "size" => {
            p_size(id, &arg_s(a, 0));
            Value::Null
        }
        "browser" => {
            if let Some(p) = app().players.get(&id) {
                open_in_browser(p.info["canonical"].as_str().unwrap_or(""));
            }
            Value::Null
        }
        "close" | "close_button" => {
            p_close_button(id);
            Value::Null
        }
        "to_menu" => {
            p_to_menu(id);
            Value::Null
        }
        "to_feed" => {
            p_to_feed(id);
            Value::Null
        }
        "minimize" => {
            winapi::minimize(player_hwnd(id)?);
            Value::Null
        }
        "fullscreen" => p_fullscreen(id),
        "ui_state" => {
            let d = data::load();
            json!({"ui": d["settings"]["ui"], "tt": d["settings"]["tiktok"], "pinned": app().players.get(&id).map(|p| p.on_top).unwrap_or(false)})
        }
        "ui_setting" => update_ui(&arg_s(a, 0), a.get(1).cloned().unwrap_or(Value::Null)),
        "tt_setting" => update_tt(&arg_s(a, 0), a.get(1).cloned().unwrap_or(Value::Null)),
        "drag" => {
            p_drag(id);
            Value::Null
        }
        "save_progress" => {
            let url = app().players.get(&id).map(|p| p.info["canonical"].as_str().unwrap_or("").to_string()).unwrap_or_default();
            if !url.is_empty() {
                let (t, dur) = (arg_f(a, 0) as i64, arg_f(a, 1) as i64);
                data::update(|d| {
                    if let Some(h) = d["history"].as_array_mut().and_then(|h| h.iter_mut().find(|h| h["url"] == url)) {
                        h["pos"] = json!(if dur > 0 && t > dur - 10 { 0 } else { t });
                        h["dur"] = json!(dur);
                    }
                });
            }
            Value::Null
        }
        "get_pos" => data::load()["pos"][arg_s(a, 0)].clone().as_i64().map(|v| json!(v)).unwrap_or(json!(0)),
        "save_pos" => {
            let (vid, t) = (arg_s(a, 0), arg_f(a, 1) as i64);
            data::update(|d| {
                if t > 5 {
                    d["pos"][&vid] = json!(t);
                } else if let Some(o) = d["pos"].as_object_mut() {
                    o.remove(&vid);
                }
                if let Some(o) = d["pos"].as_object_mut() {
                    if o.len() > 300 {
                        let keys: Vec<String> = o.keys().take(o.len() - 200).cloned().collect();
                        for k in keys {
                            o.remove(&k);
                        }
                    }
                }
            });
            Value::Null
        }
        "fit" => {
            p_fit(id, arg_f(a, 0), arg_f(a, 1), arg_f(a, 2), arg_f(a, 3));
            Value::Null
        }
        _ => return Err(format!("нет метода {name}")),
    })
}

// ---------- API панелей ----------
fn panel_api(id: WinId, name: &str, a: &[Value]) -> Result<Value, String> {
    let (player, _role) = app().panel_owner.get(&id).cloned().ok_or("нет окна плеера")?;
    Ok(match name {
        "state" => {
            let d = data::load();
            let aa = app();
            let ps = aa.panels.iter().find(|p| p.player == player);
            json!({"ui": d["settings"]["ui"], "tt": d["settings"]["tiktok"],
                   "pinned": aa.players.get(&player).map(|p| p.on_top).unwrap_or(false),
                   "fs": aa.players.get(&player).map(|p| p.fs).unwrap_or(false),
                   "title": ps.map(|p| p.title.clone()).unwrap_or_default(), "platform": ps.map(|p| p.platform.clone()).unwrap_or_default(),
                   "kind": ps.map(|p| p.kind.clone()).unwrap_or_default(), "gearOpen": ps.map(|p| p.gear_open).unwrap_or(false)})
        }
        "pin" => p_pin(player),
        "minimize" => {
            if let Some(h) = hwnd_of(player) {
                winapi::minimize(h);
            }
            Value::Null
        }
        "fullscreen" => p_fullscreen(player),
        "close" => {
            p_close_button(player);
            Value::Null
        }
        "browser" => {
            if let Some(p) = app().players.get(&player) {
                open_in_browser(p.info["canonical"].as_str().unwrap_or(""));
            }
            Value::Null
        }
        "size" => {
            p_size(player, &arg_s(a, 0));
            Value::Null
        }
        "drag" => {
            p_drag(player);
            Value::Null
        }
        "nav" => {
            eval(player, &format!("location.href='https://www.tiktok.com{}'", arg_s(a, 0).replace('\'', "")));
            Value::Null
        }
        "cur_path" => json!(eval_value(player, "location.pathname").and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default()),
        "act" => eval_value(player, &format!("window.__mpAct&&__mpAct('{}')", arg_s(a, 0).replace('\'', ""))).unwrap_or(Value::Null),
        "ui_setting" => update_ui(&arg_s(a, 0), a.get(1).cloned().unwrap_or(Value::Null)),
        "tt_setting" => update_tt(&arg_s(a, 0), a.get(1).cloned().unwrap_or(Value::Null)),
        "gear" => {
            let mut aa = app();
            match aa.panels.iter_mut().find(|p| p.player == player) {
                Some(ps) => {
                    ps.gear_open = !ps.gear_open;
                    json!(ps.gear_open)
                }
                None => json!(false),
            }
        }
        "set_height" => {
            let (role, h) = (arg_s(a, 0), (arg_f(a, 1) as i32).clamp(24, 900));
            if let Some(ps) = app().panels.iter_mut().find(|p| p.player == player) {
                ps.heights.insert(role, h);
            }
            Value::Null
        }
        _ => return Err(format!("нет метода {name}")),
    })
}

/// Результат скрипта как JSON-значение (строки, возвращённые скриптом как JSON.stringify, разбираются повторно).
pub fn eval_value(win: WinId, js: &str) -> Option<Value> {
    let r = eval_result(win, js, 4000)?;
    let v: Value = serde_json::from_str(&r).ok()?;
    if let Value::String(s) = &v {
        if let Ok(inner) = serde_json::from_str::<Value>(s) {
            if inner.is_object() || inner.is_array() {
                return Some(inner);
            }
        }
    }
    Some(v)
}

// ---------- API главного окна ----------
pub fn play(link: &str, mode: &str, resume: bool, geom: Option<[i32; 4]>) -> Value {
    let mut link = link.trim().to_string();
    if mode == "twitch" && !link.is_empty() && !link.contains('.') {
        link = format!("https://www.twitch.tv/{}", link.trim_start_matches(['@', '/']));
    }
    if mode == "netflix" || link.to_lowercase().contains("netflix.com") {
        return netflix(&link);
    }
    if mode == "tiktok" && link.starts_with('@') {
        open_site(&format!("https://www.tiktok.com/{link}"), true);
        return json!({"ok": true, "state": data::load()});
    }
    let mut info = net::parse_with(&link, false); // без обращения к сети: плеер открывается сразу
    if info.is_null() {
        return json!({"error": "Вставь ссылку"});
    }
    let canon = info["canonical"].as_str().unwrap_or("").to_string();
    let d = data::update(|d| {
        let hist = d["history"].as_array().cloned().unwrap_or_default();
        let old = hist.iter().find(|h| h["url"] == canon).cloned().unwrap_or(json!({}));
        if resume && old["pos"].as_i64().unwrap_or(0) > 5 {
            info["start"] = json!(old["pos"].as_i64().unwrap_or(0));
        }
        let mut h2: Vec<Value> = hist.into_iter().filter(|h| h["url"] != canon).collect();
        h2.insert(
            0,
            json!({"url": canon, "title": info["title"], "platform": info["platform"], "thumb": info.get("thumb").cloned().unwrap_or(json!("")),
                   "fav": old["fav"].as_bool().unwrap_or(false), "ts": data::now(), "pos": old["pos"].as_i64().unwrap_or(0),
                   "dur": old["dur"].as_i64().unwrap_or(0), "pinned": old["pinned"].as_bool().unwrap_or(false)}),
        );
        let h3: Vec<Value> = h2.into_iter().enumerate().filter(|(i, h)| *i < 200 || h["fav"].as_bool().unwrap_or(false) || h["pinned"].as_bool().unwrap_or(false)).map(|(_, h)| h).collect();
        d["history"] = Value::Array(h3);
        d.clone()
    });
    open_player(&info, geom);
    let info2 = info.clone();
    std::thread::spawn(move || enrich_later(info2));
    json!({"ok": true, "state": d})
}

/// Название и автор подгружаются уже после открытия окна: обновляем историю и подпись панели.
fn enrich_later(info: Value) {
    let e = net::enrich(&info);
    let title = e["title"].as_str().unwrap_or("").to_string();
    if title.is_empty() {
        return;
    }
    let canon = info["canonical"].as_str().unwrap_or("").to_string();
    data::update(|d| {
        if let Some(h) = d["history"].as_array_mut().and_then(|h| h.iter_mut().find(|h| h["url"] == canon)) {
            h["title"] = json!(title);
            if info["platform"] == "TikTok" {
                if let Some(t) = e["thumb"].as_str().filter(|t| !t.is_empty()) {
                    h["thumb"] = json!(t);
                }
            }
        }
    });
    let mut to_refresh = vec![];
    {
        let mut a = app();
        let player = a.players.iter().find(|(_, p)| p.info["canonical"] == canon.as_str()).map(|(id, _)| *id);
        if let Some(pid) = player {
            if let Some(p) = a.players.get_mut(&pid) {
                p.info["title"] = json!(title);
            }
            if let Some(ps) = a.panels.iter_mut().find(|p| p.player == pid) {
                ps.title = title.clone();
                to_refresh.extend(ps.wins.values().copied());
            }
        }
    }
    for w in to_refresh {
        eval(w, "window.__mpRefresh&&window.__mpRefresh()");
    }
    notify_main();
}

fn netflix(link: &str) -> Value {
    let mut link = link.trim().to_string();
    if link.is_empty() {
        link = "https://www.netflix.com/browse".into();
    } else if !link.starts_with("http") {
        link = format!("https://{link}");
    }
    if !link.splitn(4, '/').nth(2).unwrap_or("").to_lowercase().contains("netflix.com") {
        return json!({"error": "Это не ссылка на Netflix"});
    }
    let mut canon = link.split('#').next().unwrap_or("").to_string();
    if regex::Regex::new(r"/(title|watch)/\d+").unwrap().is_match(&canon) {
        canon = canon.split('?').next().unwrap_or("").to_string();
        let tail = canon.rsplit('/').next().unwrap_or("").to_string();
        data::update(|d| {
            let hist = d["history"].as_array().cloned().unwrap_or_default();
            let old = hist.iter().find(|h| h["url"] == canon).cloned().unwrap_or(json!({}));
            let mut h2: Vec<Value> = hist.into_iter().filter(|h| h["url"] != canon).collect();
            h2.insert(0, json!({"url": canon, "title": format!("Netflix {tail}"), "platform": "Netflix", "thumb": "",
                "fav": old["fav"].as_bool().unwrap_or(false), "ts": data::now(), "pos": 0, "dur": 0, "pinned": old["pinned"].as_bool().unwrap_or(false)}));
            d["history"] = Value::Array(h2);
        });
    }
    open_site(&link, false);
    json!({"ok": true, "state": data::load()})
}

fn history_toggle(url: &str, key: &str) -> Value {
    data::update(|d| {
        if let Some(h) = d["history"].as_array_mut().and_then(|h| h.iter_mut().find(|h| h["url"] == url)) {
            let cur = h[key].as_bool().unwrap_or(false);
            h[key] = json!(!cur);
        }
        d.clone()
    })
}

static TT_SEARCH: Mutex<()> = Mutex::new(());
static OPEN: Mutex<Vec<(String, WinId, std::time::Instant)>> = Mutex::new(Vec::new());

/// Одно окно на одну ссылку: повторный клик поднимает открытое окно вместо нового. -> true, если окно нужно создать.
fn claim(key: &str, id: WinId) -> bool {
    let mut o = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, ex, t)) = o.iter().find(|(k, _, _)| k == key).cloned() {
        let hw = hwnd_of(ex);
        if let Some(h) = hw.filter(|h| winapi::is_alive(*h)) {
            drop(o);
            let pinned = app().players.get(&ex).map(|p| p.on_top).unwrap_or(false);
            winapi::restore(h);
            winapi::topmost(h, true);
            winapi::topmost(h, pinned);
            return false;
        }
        if hw.is_none() && t.elapsed() < std::time::Duration::from_secs(8) {
            return false; // окно ещё создаётся: повторный клик ничего не открывает
        }
    }
    o.retain(|(k, _, _)| k != key);
    o.push((key.to_string(), id, std::time::Instant::now()));
    true
}

fn forget(id: WinId) {
    OPEN.lock().unwrap_or_else(|e| e.into_inner()).retain(|(_, w, _)| *w != id);
}

const TT_SEARCH_JS: &str = r#"JSON.stringify((()=>{const seen=new Set(),out=[];
 for(const a of document.querySelectorAll('a[href*="/video/"]')){const h=a.href.split('?')[0];if(seen.has(h))continue;seen.add(h);
  const im=a.querySelector('img');out.push({url:h,alt:im?im.alt:''});}
 return out.slice(0,10)})())"#;

/// Поиск TikTok: страница поиска читается в скрытом окне (с вашим входом), обложки через oEmbed.
fn search_tiktok(q: &str) -> Value {
    let Ok(_g) = TT_SEARCH.lock() else { return Value::Null };
    let win = hidden_window(&format!("https://www.tiktok.com/search/video?q={}", urlencoding::encode(q)));
    let mut items: Vec<Value> = vec![];
    for _ in 0..14 {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        items = eval_value(win, TT_SEARCH_JS).and_then(|v| v.as_array().cloned()).unwrap_or_default();
        if items.len() >= 3 {
            break;
        }
    }
    send(Ev::Close(win));
    if items.is_empty() {
        return json!([]);
    }
    let res: Mutex<Vec<(usize, Value)>> = Mutex::new(vec![]);
    std::thread::scope(|s| {
        for (i, it) in items.iter().enumerate() {
            let res = &res;
            s.spawn(move || {
                let card = net::tiktok_card(it["url"].as_str().unwrap_or(""), it["alt"].as_str().unwrap_or(""));
                res.lock().unwrap().push((i, card));
            });
        }
    });
    let mut r = res.into_inner().unwrap();
    r.sort_by_key(|(i, _)| *i);
    Value::Array(r.into_iter().map(|(_, v)| v).collect())
}

/// Скрытое окно с сайтом (поиск TikTok, чтение данных аккаунта).
pub fn hidden_window(url: &str) -> WinId {
    let mut sp = Spec::new(Role::Hidden, url.to_string());
    sp.size = (900, 700);
    sp.visible = false;
    sp.title = "hidden".into();
    let id = sp.id;
    send(Ev::Create(sp));
    id
}

/// Небольшое окно «Подождите…» по центру экрана, пока включается обход.
pub fn open_splash() {
    let mut sp = Spec::new(Role::Splash, format!("http://127.0.0.1:{}/splash", port()));
    sp.title = "Mini Player".into();
    sp.size = (380, 120);
    sp.decorations = false;
    sp.on_top = true;
    sp.min = (100, 60);
    send(Ev::Create(sp));
}

fn bypass_info() -> Value {
    bypass::info()
}

fn main_api(name: &str, a: &[Value]) -> Result<Value, String> {
    Ok(match name {
        "bypass_info" => bypass_info(),
        "bypass_set" => bypass::set(arg_b(a, 0)),
        "bypass_retest" => {
            bypass::retest();
            json!(true)
        }
        "state" => data::load(),
        "clipboard" => json!(winapi::clipboard_text()),
        "play" => {
            let geom = a.get(3).and_then(|v| v.as_array()).and_then(|g| {
                let n: Vec<i32> = g.iter().filter_map(|x| x.as_i64()).map(|x| x as i32).collect();
                if n.len() == 4 { Some([n[0], n[1], n[2], n[3]]) } else { None }
            });
            play(&arg_s(a, 0), &arg_s(a, 1), arg_b(a, 2), geom)
        }
        "site" => {
            open_site(&arg_s(a, 0), arg_b(a, 1));
            Value::Null
        }
        "fav" => history_toggle(&arg_s(a, 0), "fav"),
        "pin_item" => history_toggle(&arg_s(a, 0), "pinned"),
        "remove" => {
            let url = arg_s(a, 0);
            data::update(|d| {
                let h: Vec<Value> = d["history"].as_array().cloned().unwrap_or_default().into_iter().filter(|h| h["url"] != url).collect();
                d["history"] = Value::Array(h);
                d.clone()
            })
        }
        "clear" => {
            let favs_too = arg_b(a, 0);
            data::update(|d| {
                let h: Vec<Value> = if favs_too {
                    vec![]
                } else {
                    d["history"].as_array().cloned().unwrap_or_default().into_iter().filter(|h| h["fav"].as_bool().unwrap_or(false) || h["pinned"].as_bool().unwrap_or(false)).collect()
                };
                d["history"] = Value::Array(h);
                d.clone()
            })
        }
        "search" => match arg_s(a, 1).as_str() {
            "yt" => net::search_youtube(&arg_s(a, 0)),
            "tt" => search_tiktok(&arg_s(a, 0)),
            _ => Value::Null,
        },
        "login_state" => accounts::login_state(),
        "accounts" => accounts::accounts(),
        "acc_switch" => accounts::acc_switch_api(&arg_s(a, 0), &arg_s(a, 1)),
        "acc_add" => accounts::acc_add(&arg_s(a, 0)),
        "acc_remove" => accounts::acc_remove(&arg_s(a, 0), &arg_s(a, 1)),
        "logout" => accounts::logout(&arg_s(a, 0)),
        "tt_setting" => update_tt(&arg_s(a, 0), a.get(1).cloned().unwrap_or(Value::Null)),
        "ui_setting" => update_ui(&arg_s(a, 0), a.get(1).cloned().unwrap_or(Value::Null)),
        "preview" => {
            let info = net::parse(&arg_s(a, 0));
            if info.is_null() {
                Value::Null
            } else {
                json!({"title": info["title"], "channel": info.get("channel").cloned().unwrap_or(json!("")), "platform": info["platform"],
                       "vertical": info["vertical"], "url": info["canonical"], "thumb": info.get("thumb").cloned().unwrap_or(json!(""))})
            }
        }
        "fav_link" => {
            let info = net::parse(&arg_s(a, 0));
            data::update(|d| {
                if !info.is_null() {
                    let canon = info["canonical"].clone();
                    if let Some(h) = d["history"].as_array_mut().and_then(|h| h.iter_mut().find(|h| h["url"] == canon)) {
                        let cur = h["fav"].as_bool().unwrap_or(false);
                        h["fav"] = json!(!cur);
                    } else if let Some(h) = d["history"].as_array_mut() {
                        h.insert(0, json!({"url": canon, "title": info["title"], "platform": info["platform"],
                            "thumb": info.get("thumb").cloned().unwrap_or(json!("")), "fav": true, "ts": data::now()}));
                    }
                }
                d.clone()
            })
        }
        "setting" => {
            let (k, v) = (arg_s(a, 0), a.get(1).cloned().unwrap_or(Value::Null));
            data::update(|d| {
                d["settings"][k.as_str()] = v;
                d.clone()
            })
        }
        _ => return Err(format!("нет метода {name}")),
    })
}

pub fn _unused() {
    let _ = shim::SHIM;
}
