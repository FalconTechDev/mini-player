//! Хранилище настроек и истории: тот же файл и формат, что у версии на Python (%APPDATA%\MiniPlayer\data.json).
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;

static CFG: Mutex<()> = Mutex::new(());

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("MiniPlayer")
}

pub fn data_file() -> PathBuf {
    data_dir().join("data.json")
}

fn tt_defaults() -> Value {
    json!({"hide_desc": false, "hide_author": false, "hide_ads": true, "remember_window": true,
           "no_frame": true, "fill_crop": false, "fit_window": false, "hide_titlebar": true, "auto_scroll": true})
}

fn ui_defaults() -> Value {
    json!({"yt_exit": false, "yt_exit_to": "feed", "bypass": false, "layout": "tabs", "default_search": false,
           "engine": "google", "autohide_bar": true, "opacity": 1.0, "op_frame": 1.0, "op_video": 1.0, "auto_paste": false})
}

fn fill(target: &mut Value, defaults: Value) {
    if !target.is_object() {
        *target = json!({});
    }
    if let (Some(t), Some(d)) = (target.as_object_mut(), defaults.as_object()) {
        for (k, v) in d {
            t.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
}

fn normalize(mut d: Value) -> Value {
    if !d.is_object() {
        d = json!({});
    }
    let o = d.as_object_mut().unwrap();
    o.entry("history").or_insert_with(|| json!([]));
    o.entry("settings").or_insert_with(|| json!({"on_top": true, "size": "M"}));
    o.entry("windows").or_insert_with(|| json!({}));
    o.entry("pos").or_insert_with(|| json!({}));
    let s = o.get_mut("settings").unwrap();
    if !s.is_object() {
        *s = json!({"on_top": true, "size": "M"});
    }
    let so = s.as_object_mut().unwrap();
    fill(so.entry("tiktok").or_insert_with(|| json!({})), tt_defaults());
    fill(so.entry("ui").or_insert_with(|| json!({})), ui_defaults());
    d
}

fn read() -> Value {
    let raw = std::fs::read_to_string(data_file()).ok().and_then(|s| serde_json::from_str::<Value>(s.trim_start_matches('\u{feff}')).ok());
    normalize(raw.unwrap_or_else(|| json!({})))
}

fn write(d: &Value) {
    let _ = std::fs::create_dir_all(data_dir());
    let tmp = data_dir().join("data.json.tmp");
    if let Ok(s) = serde_json::to_string_pretty(d) {
        if std::fs::write(&tmp, s).is_ok() {
            let _ = std::fs::rename(&tmp, data_file());
        }
    }
}

pub fn load() -> Value {
    let _g = CFG.lock().unwrap_or_else(|e| e.into_inner());
    read()
}

/// Читает, меняет и сохраняет данные под одной блокировкой.
pub fn update<R>(f: impl FnOnce(&mut Value) -> R) -> R {
    let _g = CFG.lock().unwrap_or_else(|e| e.into_inner());
    let mut d = read();
    let r = f(&mut d);
    write(&d);
    r
}

pub fn ui(key: &str) -> Value {
    load()["settings"]["ui"][key].clone()
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
