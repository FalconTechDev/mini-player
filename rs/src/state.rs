//! Общее состояние приложения: окна, плееры, панели. Доступно из любого потока.
use crate::winapi::Rect;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;
use tao::event_loop::EventLoopProxy;

pub type WinId = u64;

#[derive(Clone, Debug, PartialEq)]
pub enum Role {
    Main,
    Player,
    Tiktok,
    Site(String), // "yt" | "tw"
    Panel,
    Hidden,
    Splash,
    Plain,
}

/// Что выполнить после загрузки страницы.
#[derive(Clone, Debug, PartialEq)]
pub enum OnLoad {
    None,
    Tiktok,
    Player,
    Site(String),
}

pub struct WinInfo {
    pub hwnd: isize,
    pub role: Role,
    pub geom_key: Option<String>,
    pub geom: Option<(i32, i32, i32, i32)>, // логические x, y, ширина, высота
    pub last_write: Instant,
}

pub struct Player {
    pub info: Value,
    pub on_top: bool,
    pub fs: bool,
    pub saved: Rect,
    pub closed: bool,
}

pub struct PanelSet {
    pub player: WinId,
    pub kind: String,     // "player" | "tt"
    pub platform: String,
    pub title: String,
    pub wins: HashMap<String, WinId>,
    pub heights: HashMap<String, i32>, // CSS-пиксели
    pub vis: HashMap<String, bool>,
    pub last_want: HashMap<String, Instant>,
    pub gear_open: bool,
    pub ready: bool,
    pub pos: HashMap<String, Rect>,
    pub zstate: HashMap<String, bool>,
}

#[derive(Default)]
pub struct AppState {
    pub wins: HashMap<WinId, WinInfo>,
    pub players: HashMap<WinId, Player>,
    pub panels: Vec<PanelSet>,
    pub panel_owner: HashMap<WinId, (WinId, String)>,
    pub main: Option<WinId>,
    pub autohide: bool,
}

static APP: OnceLock<Mutex<AppState>> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
static PROXY: OnceLock<Mutex<EventLoopProxy<Ev>>> = OnceLock::new();
pub static PORT: OnceLock<u16> = OnceLock::new();

pub fn app() -> MutexGuard<'static, AppState> {
    APP.get_or_init(|| Mutex::new(AppState { autohide: true, ..Default::default() })).lock().unwrap_or_else(|e| e.into_inner())
}

pub fn new_id() -> WinId {
    NEXT.fetch_add(1, Ordering::SeqCst)
}

pub fn port() -> u16 {
    *PORT.get().unwrap_or(&0)
}

pub fn set_proxy(p: EventLoopProxy<Ev>) {
    let _ = PROXY.set(Mutex::new(p));
}

pub fn send(ev: Ev) {
    if let Some(p) = PROXY.get() {
        let _ = p.lock().unwrap_or_else(|e| e.into_inner()).send_event(ev);
    }
}

pub fn hwnd_of(id: WinId) -> Option<isize> {
    app().wins.get(&id).map(|w| w.hwnd)
}

pub struct Spec {
    pub title: String,
    pub url: String,
    pub role: Role,
    pub size: (i32, i32),            // логический внешний размер
    pub pos: Option<(i32, i32)>,     // логическая позиция
    pub on_top: bool,
    pub min: (i32, i32),
    pub icon: Option<String>,
    pub visible: bool,
    pub frameless_caption: bool,     // убрать системный заголовок, оставив рамку
    pub decorations: bool,
    pub opacity: Option<f64>,
    pub keepalive: bool,
    pub geom_key: Option<String>,
    pub bg: Option<(u8, u8, u8)>,
    pub align: Option<Rect>,
    pub on_load: OnLoad,
    pub panels: Option<(String, String, String)>, // kind, platform, title
    pub id: WinId,
}

impl Spec {
    pub fn new(role: Role, url: String) -> Spec {
        Spec {
            title: "Mini Player".into(),
            url,
            role,
            size: (560, 380),
            pos: None,
            on_top: false,
            min: (300, 220),
            icon: None,
            visible: true,
            frameless_caption: false,
            decorations: true,
            opacity: None,
            keepalive: false,
            geom_key: None,
            bg: None,
            align: None,
            on_load: OnLoad::None,
            panels: None,
            id: new_id(),
        }
    }
}

pub enum Ev {
    Eval { win: WinId, js: String },
    With { win: WinId, f: Box<dyn FnOnce(&wry::WebView) + Send> },
    Create(Spec),
    Close(WinId),
    Exit,
    Wake,
}

/// Выполняет скрипт в окне и возвращает результат (строка JSON) либо None по таймауту.
pub fn eval_result(win: WinId, js: &str, timeout_ms: u64) -> Option<String> {
    let (tx, rx): (Sender<String>, _) = std::sync::mpsc::channel();
    let js = js.to_string();
    send(Ev::With {
        win,
        f: Box::new(move |wv| {
            let tx2 = tx.clone();
            if wv.evaluate_script_with_callback(&js, move |r| {
                let _ = tx2.send(r);
            })
            .is_err()
            {
                let _ = tx.send("null".into());
            }
        }),
    });
    rx.recv_timeout(std::time::Duration::from_millis(timeout_ms)).ok()
}

pub fn eval(win: WinId, js: &str) {
    send(Ev::Eval { win, js: js.to_string() });
}
