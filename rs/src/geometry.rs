//! Запоминание размера и положения окон. Сохраняются логические пиксели внешней рамки; полноэкранные, свёрнутые и невидимые
//! положения не запоминаются и при чтении отбрасываются.
use crate::state::*;
use crate::{data, winapi};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// Сохранённая геометрия годится, только если окно обычное: не полный экран, не свёрнуто, видно на экране.
pub fn ok(g: &Value) -> bool {
    let get = |k: &str| g.get(k).and_then(|v| v.as_f64());
    let (Some(w), Some(h), Some(x), Some(y)) = (get("width"), get("height"), get("x"), get("y")) else { return false };
    let k = winapi::system_scale();
    let (vx, vy, vw, vh) = winapi::virtual_screen();
    let (vx, vy, vw, vh) = (vx as f64 / k, vy as f64 / k, vw as f64 / k, vh as f64 / k);
    if w < 240.0 || h < 240.0 || w > vw || h > vh {
        return false;
    }
    if w >= vw * 0.97 && h >= vh * 0.9 {
        return false;
    }
    let (cx, cy) = (x + w / 2.0, y + 20.0);
    cx >= vx && cx < vx + vw && cy >= vy && cy < vy + vh
}

/// -> (размер, позиция) для нового окна.
pub fn restore(key: &str, w: i32, h: i32) -> ((i32, i32), Option<(i32, i32)>) {
    let d = data::load();
    if d["settings"]["tiktok"]["remember_window"].as_bool().unwrap_or(true) {
        if let Some(g) = d["windows"].get(key) {
            if ok(g) {
                let n = |k: &str| g[k].as_f64().unwrap_or(0.0) as i32;
                return ((n("width"), n("height")), Some((n("x"), n("y"))));
            }
        }
    }
    ((w, h), None)
}

fn write(key: &str, g: (i32, i32, i32, i32)) {
    if !data::load()["settings"]["tiktok"]["remember_window"].as_bool().unwrap_or(true) {
        return;
    }
    data::update(|d| {
        d["windows"][key] = json!({"x": g.0, "y": g.1, "width": g.2, "height": g.3});
    });
}

fn current(id: WinId) -> Option<(String, (i32, i32, i32, i32))> {
    let a = app();
    let w = a.wins.get(&id)?;
    let key = w.geom_key.clone()?;
    if a.players.get(&id).map(|p| p.fs).unwrap_or(false) {
        return None;
    }
    let hwnd = w.hwnd;
    drop(a);
    if winapi::is_minimized(hwnd) || !winapi::is_visible(hwnd) {
        return None;
    }
    let (x, y, ww, hh) = winapi::rect(hwnd);
    let k = winapi::scale(hwnd);
    let g = ((x as f64 / k) as i32, (y as f64 / k) as i32, (ww as f64 / k) as i32, (hh as f64 / k) as i32);
    if g.0 < -1000 || g.1 < -1000 || g.2 < 240 || g.3 < 240 {
        return None;
    }
    Some((key, g))
}

/// Вызывается при перемещении/изменении размера окна: запоминает не чаще раза в секунду.
pub fn track(id: WinId) {
    let Some((key, g)) = current(id) else { return };
    let mut a = app();
    if let Some(w) = a.wins.get_mut(&id) {
        w.geom = Some(g);
        if w.last_write.elapsed() < Duration::from_secs(1) {
            return;
        }
        w.last_write = Instant::now();
    }
    drop(a);
    write(&key, g);
}

/// При закрытии окна записывает последнее известное положение.
pub fn flush(id: WinId) {
    let (key, g) = match current(id) {
        Some(x) => x,
        None => {
            let a = app();
            match a.wins.get(&id) {
                Some(w) if w.geom.is_some() && w.geom_key.is_some() && !a.players.get(&id).map(|p| p.fs).unwrap_or(false) => (w.geom_key.clone().unwrap(), w.geom.unwrap()),
                _ => return,
            }
        }
    };
    write(&key, g);
}
