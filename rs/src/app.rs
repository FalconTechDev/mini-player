//! Цикл событий и окна. Окна и WebView создаются только здесь (в главном потоке); остальные потоки шлют события через `state::send`.
use crate::state::*;
use crate::{api, bypass, data, geometry, panels, shim, winapi};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopWindowTarget};
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Icon, Window, WindowBuilder, WindowId};
use wry::{NewWindowResponse, PageLoadEvent, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows};

struct Entry {
    webview: WebView, // порядок важен: сначала закрывается WebView, потом окно
    window: Window,
}

static EXTRA_ARGS: OnceLock<Mutex<String>> = OnceLock::new();

/// Дополнительные флаги Chromium (прокси режима обхода и т. п.); задаются до создания первого окна.
pub fn set_extra_args(s: &str) {
    *EXTRA_ARGS.get_or_init(|| Mutex::new(String::new())).lock().unwrap() = s.to_string();
}

fn browser_args() -> String {
    let extra = EXTRA_ARGS.get_or_init(|| Mutex::new(String::new())).lock().unwrap().clone();
    let extra = format!("{extra} {}", std::env::var("MP_EXTRA_ARGS").unwrap_or_default());
    format!(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,CalculateNativeWinOcclusion --disable-renderer-backgrounding \
         --disable-background-timer-throttling --disable-backgrounding-occluded-windows --autoplay-policy=no-user-gesture-required {extra}"
    )
}

fn icon(name: &str) -> Option<Icon> {
    let bytes: &[u8] = match name {
        "youtube" => include_bytes!("../../icons/youtube.png"),
        "twitch" => include_bytes!("../../icons/twitch.png"),
        "tiktok" => include_bytes!("../../icons/tiktok.png"),
        "netflix" => include_bytes!("../../icons/netflix.png"),
        "other" => include_bytes!("../../icons/other.png"),
        _ => include_bytes!("../../icons/app.png"),
    };
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let img = image::imageops::resize(&img, 48, 48, image::imageops::FilterType::Triangle);
    Icon::from_rgba(img.into_raw(), 48, 48).ok()
}

fn create(target: &EventLoopWindowTarget<Ev>, ctx: &mut WebContext, spec: Spec) -> Result<Entry, String> {
    let scale = winapi::system_scale();
    let (lw, lh) = spec.size;
    let mut wb = WindowBuilder::new()
        .with_title(spec.title.clone())
        .with_inner_size(LogicalSize::new(lw.max(10) as f64, lh.max(10) as f64))
        .with_min_inner_size(LogicalSize::new(spec.min.0 as f64, spec.min.1 as f64))
        .with_visible(false)
        .with_decorations(spec.decorations)
        .with_always_on_top(spec.on_top);
    if spec.role == Role::Panel {
        wb = wb.with_skip_taskbar(true).with_resizable(false).with_focused(false);
    }
    if !spec.decorations {
        wb = wb.with_undecorated_shadow(false);
    }
    if let Some(name) = &spec.icon {
        wb = wb.with_window_icon(icon(name));
    } else if spec.role != Role::Panel && spec.role != Role::Splash {
        wb = wb.with_window_icon(icon("app"));
    }
    let window = wb.build(target).map_err(|e| e.to_string())?;
    let hwnd = window.hwnd();

    // внешний размер и позиция в физических пикселях
    let (pw, ph) = ((lw as f64 * scale) as i32, (lh as f64 * scale) as i32);
    match spec.pos {
        Some((x, y)) => winapi::place(hwnd, Some((x as f64 * scale) as i32), Some((y as f64 * scale) as i32), Some(pw), Some(ph)),
        None => {
            if spec.role == Role::Splash || spec.role == Role::Main || spec.role == Role::Player || spec.role == Role::Tiktok || matches!(spec.role, Role::Site(_)) || spec.role == Role::Plain {
                let (mx, my, mw, mh) = winapi::work_area(hwnd, false);
                winapi::place(hwnd, Some(mx + (mw - pw) / 2), Some(my + (mh - ph) / 2), Some(pw), Some(ph));
            } else {
                winapi::place(hwnd, None, None, Some(pw), Some(ph));
            }
        }
    }
    if let Some(o) = spec.opacity {
        winapi::set_opacity(hwnd, o);
    }
    if spec.role == Role::Panel {
        winapi::panel_setup(hwnd);
    }

    let id = spec.id;
    let role = spec.role.clone();
    let port = port();
    let proxy_id = id;
    let on_load = spec.on_load.clone();
    let mut b = WebViewBuilder::new_with_web_context(ctx)
        .with_url(spec.url.clone())
        .with_additional_browser_args(browser_args())
        .with_initialization_script(shim::SHIM)
        .with_autoplay(true)
        .with_ipc_handler(move |req| {
            let origin_ok = req.uri().to_string().starts_with(&format!("http://127.0.0.1:{port}"));
            api::handle_ipc(id, role.clone(), origin_ok, req.body().to_string());
        })
        .with_new_window_req_handler(move |url, _| {
            // всплывающие окна (вход через Google и т. п.) открываем в том же окне
            send(Ev::With {
                win: proxy_id,
                f: Box::new(move |wv| {
                    let _ = wv.load_url(&url);
                }),
            });
            NewWindowResponse::Deny
        })
        .with_on_page_load_handler(move |ev, url| {
            if !matches!(ev, PageLoadEvent::Finished) {
                return;
            }
            match &on_load {
                OnLoad::Tiktok => eval(id, &api::tiktok_js()),
                OnLoad::Site(kind) => eval(id, &shim::intercept_js(kind)),
                OnLoad::Player => {
                    if url.contains("youtube.com/watch") {
                        eval(id, shim::WATCH_ONLY_JS);
                    }
                }
                OnLoad::None => {}
            }
        });
    if spec.keepalive {
        b = b.with_initialization_script(shim::KEEPALIVE);
    }
    if let Some((r, g, bl)) = spec.bg {
        b = b.with_background_color((r, g, bl, 255));
    }
    let webview = b.build(&window).map_err(|e| e.to_string())?;

    {
        let mut a = app();
        a.wins.insert(
            id,
            WinInfo { hwnd, role: spec.role.clone(), geom_key: spec.geom_key.clone(), geom: None, last_write: std::time::Instant::now() },
        );
    }
    if spec.visible {
        window.set_visible(true);
        if spec.role != Role::Panel {
            window.set_focus();
        }
    }
    if spec.frameless_caption {
        // заголовок снимаем после показа окна (раньше система возвращает его) и ещё раз чуть позже
        winapi::hide_caption(hwnd);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(250));
            winapi::hide_caption(hwnd);
        });
    }
    if let Some(target_rect) = spec.align {
        std::thread::spawn(move || {
            for _ in 0..3 {
                std::thread::sleep(std::time::Duration::from_millis(200));
                winapi::align_client(hwnd, target_rect);
            }
        });
    }
    Ok(Entry { webview, window })
}

fn is_content(role: &Role) -> bool {
    matches!(role, Role::Main | Role::Player | Role::Tiktok | Role::Site(_) | Role::Plain)
}

pub fn run() {
    let event_loop = EventLoopBuilder::<Ev>::with_user_event().build();
    set_proxy(event_loop.create_proxy());
    let mut ctx = WebContext::new(Some(data::data_dir().join("web")));
    let mut entries: HashMap<WinId, Entry> = HashMap::new();
    let mut by_tao: HashMap<WindowId, WinId> = HashMap::new();

    api::open_main();
    panels::start_ticker();

    event_loop.run(move |event, target, cf| {
        *cf = ControlFlow::Wait;
        match event {
            Event::UserEvent(ev) => match ev {
                Ev::Create(spec) => {
                    let id = spec.id;
                    let panels_for = spec.panels.clone();
                    match create(target, &mut ctx, spec) {
                        Ok(e) => {
                            by_tao.insert(e.window.id(), id);
                            entries.insert(id, e);
                            if let Some((kind, platform, title)) = panels_for {
                                for sp in panels::make_specs(id, &kind, &platform, &title) {
                                    let pid = sp.id;
                                    if let Ok(pe) = create(target, &mut ctx, sp) {
                                        by_tao.insert(pe.window.id(), pid);
                                        entries.insert(pid, pe);
                                    }
                                }
                                panels::set_ready(id);
                            }
                        }
                        Err(e) => eprintln!("не удалось создать окно: {e}"),
                    }
                }
                Ev::Close(id) => {
                    if let Some(e) = entries.remove(&id) {
                        by_tao.remove(&e.window.id());
                    }
                    app().wins.remove(&id);
                    if !app().wins.values().any(|w| is_content(&w.role)) {
                        *cf = ControlFlow::Exit;
                    }
                }
                Ev::Eval { win, js } => {
                    if let Some(e) = entries.get(&win) {
                        let _ = e.webview.evaluate_script(&js);
                    }
                }
                Ev::With { win, f } => {
                    if let Some(e) = entries.get(&win) {
                        f(&e.webview);
                    }
                }
                Ev::Exit => *cf = ControlFlow::Exit,
                Ev::Wake => {}
            },
            Event::LoopDestroyed => bypass::stop(false),
            Event::WindowEvent { window_id, event, .. } => {
                let Some(&id) = by_tao.get(&window_id) else { return };
                match event {
                    WindowEvent::CloseRequested => {
                        geometry::flush(id);
                        let closed_panels = api::on_window_closed(id);
                        for p in closed_panels {
                            if let Some(e) = entries.remove(&p) {
                                by_tao.remove(&e.window.id());
                            }
                            app().wins.remove(&p);
                        }
                        if let Some(e) = entries.remove(&id) {
                            by_tao.remove(&e.window.id());
                        }
                        app().wins.remove(&id);
                        if !app().wins.values().any(|w| is_content(&w.role)) {
                            *cf = ControlFlow::Exit;
                        }
                    }
                    WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
                        geometry::track(id);
                        panels::reposition(id);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    });
}
