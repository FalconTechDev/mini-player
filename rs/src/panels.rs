//! Панели управления: отдельные окна снаружи окна плеера, выезжают у края при наведении.
use crate::state::*;
use crate::winapi::{self, Rect};
use std::time::{Duration, Instant};

const HOT_ZONE: f64 = 64.0; // зона у края окна, где выезжают панели (логические пиксели)
const HYSTERESIS: Duration = Duration::from_millis(450);

fn roles(kind: &str) -> Vec<&'static str> {
    if kind == "tt" {
        vec!["top", "bottom", "gear"]
    } else {
        vec!["top", "gear"]
    }
}

/// Создаёт описание окон панелей и записывает набор панелей в состояние.
pub fn make_specs(player: WinId, kind: &str, platform: &str, title: &str) -> Vec<Spec> {
    let mut ps = PanelSet {
        player,
        kind: kind.to_string(),
        platform: platform.to_string(),
        title: title.to_string(),
        wins: Default::default(),
        heights: Default::default(),
        vis: Default::default(),
        last_want: Default::default(),
        gear_open: false,
        ready: false,
        pos: Default::default(),
        zstate: Default::default(),
    };
    ps.heights.insert("top".into(), if kind == "player" { 42 } else { 66 });
    ps.heights.insert("bottom".into(), 56);
    ps.heights.insert("gear".into(), 420);
    let mut specs = vec![];
    for role in roles(kind) {
        let url = format!(
            "http://127.0.0.1:{}/panel?role={}&kind={}&platform={}&title={}",
            port(),
            role,
            kind,
            urlencoding::encode(platform),
            urlencoding::encode(title)
        );
        let mut sp = Spec::new(Role::Panel, url);
        sp.title = "panel".into();
        sp.size = (300, ps.heights[role]);
        sp.decorations = false;
        sp.visible = false;
        sp.min = (10, 10);
        sp.bg = Some((0x26, 0x21, 0x1c));
        ps.wins.insert(role.to_string(), sp.id);
        ps.vis.insert(role.to_string(), false);
        app_panel_owner(sp.id, player, role);
        specs.push(sp);
    }
    app().panels.push(ps);
    specs
}

fn app_panel_owner(panel: WinId, player: WinId, role: &str) {
    app().panel_owner.insert(panel, (player, role.to_string()));
}

pub fn set_ready(player: WinId) {
    for ps in app().panels.iter_mut() {
        if ps.player == player {
            ps.ready = true;
        }
    }
}

/// Убирает панели закрытого плеера, возвращает идентификаторы окон панелей, которые нужно закрыть.
pub fn destroy(player: WinId) -> Vec<WinId> {
    let mut a = app();
    let mut out = vec![];
    a.panels.retain(|ps| {
        if ps.player == player {
            out.extend(ps.wins.values().copied());
            false
        } else {
            true
        }
    });
    for id in &out {
        a.panel_owner.remove(id);
    }
    out
}

fn layout(a: &AppState, ps: &PanelSet) -> Option<Vec<(String, Rect)>> {
    let phwnd = a.wins.get(&ps.player)?.hwnd;
    let (x, y, w, h) = winapi::rect(phwnd);
    let (mx, my, mw, mh) = winapi::work_area(phwnd, false);
    let fs = a.players.get(&ps.player).map(|p| p.fs).unwrap_or(false);
    let k = winapi::scale(phwnd);
    let hh = |r: &str| (*ps.heights.get(r).unwrap_or(&40) as f64 * k) as i32;
    let (ht, hb, hg) = (hh("top"), hh("bottom"), hh("gear"));
    let top_y = if fs || y - ht < my { y } else { y - ht };
    let bot_y = if fs || y + h + hb > my + mh { y + h - hb } else { y + h };
    let gw = (270.0 * k) as i32;
    let mut gx = x + w + (6.0 * k) as i32;
    if gx + gw > mx + mw {
        gx = x - gw - (6.0 * k) as i32;
    }
    let gh = hg.min(mh - (20.0 * k) as i32);
    Some(vec![
        ("top".into(), (x, top_y, w, ht)),
        ("bottom".into(), (x, bot_y, w, hb)),
        ("gear".into(), (gx.max(mx), y.max(my), gw, gh)),
    ])
}

/// Сразу переставляет видимые панели за окном (во время перетаскивания).
pub fn reposition(player: WinId) {
    let mut a = app();
    let idx = match a.panels.iter().position(|p| p.player == player) {
        Some(i) => i,
        None => return,
    };
    let rects = match layout(&a, &a.panels[idx]) {
        Some(r) => r,
        None => return,
    };
    let hw: Vec<(String, isize)> = rects
        .iter()
        .filter_map(|(role, _)| a.panels[idx].wins.get(role).and_then(|id| a.wins.get(id)).map(|w| (role.clone(), w.hwnd)))
        .collect();
    let ps = &mut a.panels[idx];
    for (role, rect) in rects {
        if ps.vis.get(&role).copied().unwrap_or(false) && ps.pos.get(&role) != Some(&rect) {
            if let Some((_, hwnd)) = hw.iter().find(|(r, _)| *r == role) {
                ps.pos.insert(role.clone(), rect);
                winapi::place_rect(*hwnd, rect);
            }
        }
    }
}

fn show(ps: &mut PanelSet, role: &str, hwnd: isize, on: bool, rect: Rect, pinned: bool) {
    if on {
        if ps.pos.get(role) != Some(&rect) {
            ps.pos.insert(role.to_string(), rect);
            winapi::place_rect(hwnd, rect);
        }
        let reset = !pinned && ps.zstate.get(role).copied() != Some(false);
        winapi::panel_z(hwnd, pinned, reset);
        ps.zstate.insert(role.to_string(), pinned);
        if !ps.vis.get(role).copied().unwrap_or(false) {
            ps.vis.insert(role.to_string(), true);
            winapi::panel_visible(hwnd, true);
        }
    } else if ps.vis.get(role).copied().unwrap_or(false) {
        ps.vis.insert(role.to_string(), false);
        winapi::panel_visible(hwnd, false);
    }
}

fn tick() {
    let mut a = app();
    let autohide = a.autohide;
    let root = winapi::root_at_cursor();
    let (cx, cy) = winapi::cursor();
    let mut dead = vec![];
    for i in 0..a.panels.len() {
        let (player, ready) = (a.panels[i].player, a.panels[i].ready);
        if !ready {
            continue;
        }
        let Some(phwnd) = a.wins.get(&player).map(|w| w.hwnd) else {
            dead.push(player);
            continue;
        };
        if !winapi::is_alive(phwnd) {
            dead.push(player);
            continue;
        }
        let pinned = a.players.get(&player).map(|p| p.on_top).unwrap_or(false);
        let Some(rects) = layout(&a, &a.panels[i]) else { continue };
        let hw = |role: &str, a: &AppState, ps: &PanelSet| ps.wins.get(role).and_then(|id| a.wins.get(id)).map(|w| w.hwnd);
        let top_h = hw("top", &a, &a.panels[i]);
        let bot_h = hw("bottom", &a, &a.panels[i]);
        let gear_h = hw("gear", &a, &a.panels[i]);
        let gone = winapi::is_minimized(phwnd) || !winapi::is_visible(phwnd);
        let (x, y, w, h) = winapi::rect(phwnd);
        let k = winapi::scale(phwnd);
        let under_player = root == phwnd;
        let under_top = Some(root) == top_h;
        let under_bot = Some(root) == bot_h;
        let under_gear = Some(root) == gear_h;
        let known = under_player || under_top || under_bot || under_gear;
        let in_p = under_player && cx >= x && cx < x + w && cy >= y && cy < y + h;
        let always = !autohide;
        let ps = &mut a.panels[i];
        let hot = (HOT_ZONE * k) as i32;
        let vis = |ps: &PanelSet, r: &str| ps.vis.get(r).copied().unwrap_or(false);
        let mut want_top = always || ps.gear_open || (in_p && cy < y + hot) || (vis(ps, "top") && under_top);
        let mut want_bot = always || (in_p && cy >= y + h - hot) || (vis(ps, "bottom") && under_bot);
        let foreign = !known;
        let now = Instant::now();
        if gone || (foreign && !always && !ps.gear_open) {
            want_top = false;
            want_bot = false;
            ps.last_want.remove("top");
            ps.last_want.remove("bottom");
        }
        if want_top {
            ps.last_want.insert("top".into(), now);
        }
        if want_bot {
            ps.last_want.insert("bottom".into(), now);
        }
        let recent = |ps: &PanelSet, r: &str| ps.last_want.get(r).map(|t| now.duration_since(*t) < HYSTERESIS).unwrap_or(false);
        let rect_of = |r: &str| rects.iter().find(|(n, _)| n == r).map(|(_, rc)| *rc);
        if let (Some(hwnd), Some(rc)) = (top_h, rect_of("top")) {
            let on = recent(ps, "top");
            show(ps, "top", hwnd, on, rc, pinned);
        }
        if let (Some(hwnd), Some(rc)) = (bot_h, rect_of("bottom")) {
            let on = recent(ps, "bottom");
            show(ps, "bottom", hwnd, on, rc, pinned);
        }
        if let (Some(hwnd), Some(rc)) = (gear_h, rect_of("gear")) {
            let on = ps.gear_open && !gone && (!foreign || always || vis(ps, "gear"));
            show(ps, "gear", hwnd, on, rc, pinned);
        }
    }
    drop(a);
    for p in dead {
        for id in destroy(p) {
            send(Ev::Close(id));
        }
    }
}

pub fn start_ticker() {
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_millis(60));
        tick();
    });
}
