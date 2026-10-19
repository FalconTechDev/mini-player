//! Обновление с GitHub: проверка последнего выпуска, загрузка установщика и его тихий запуск.
//! Включается настройкой `settings.ui.auto_update` (её же выбирает установщик).
use crate::{data, net};
use serde_json::{json, Value};
use std::os::windows::process::CommandExt;
use std::sync::Mutex;

const API: &str = "https://api.github.com/repos/FalconTechDev/mini-player/releases/latest";
const ASSET: &str = "MiniPlayer_Setup.exe";
const CHECK_EVERY_SECS: u64 = 12 * 3600;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Что известно о последней проверке: показывается в настройках.
static LAST: Mutex<Option<Value>> = Mutex::new(None);

fn parts(v: &str) -> Vec<u64> {
    v.trim_start_matches('v')
        .split('.')
        .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0))
        .collect()
}

/// `remote` новее `local`? Короткие версии дополняются нулями: 2.1 == 2.1.0.
pub fn is_newer(remote: &str, local: &str) -> bool {
    let (mut a, mut b) = (parts(remote), parts(local));
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a > b
}

/// Запрос последнего выпуска -> {current, latest, available, notes, url, page, error}.
pub fn check() -> Value {
    let mut out = json!({"current": VERSION, "latest": VERSION, "available": false, "notes": "", "url": "", "page": "", "error": ""});
    match net::get_text(API, 10, false).and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(rel) if rel["tag_name"].is_string() => {
            let tag = rel["tag_name"].as_str().unwrap_or("").to_string();
            let url = rel["assets"]
                .as_array()
                .and_then(|a| a.iter().find(|x| x["name"] == ASSET))
                .and_then(|x| x["browser_download_url"].as_str())
                .unwrap_or("")
                .to_string();
            out["latest"] = json!(tag.trim_start_matches('v'));
            out["notes"] = rel["body"].clone();
            out["page"] = rel["html_url"].clone();
            out["url"] = json!(url);
            out["available"] = json!(is_newer(&tag, VERSION) && !url.is_empty());
        }
        _ => out["error"] = json!("Не удалось связаться с GitHub"),
    }
    data::update(|d| d["settings"]["ui"]["update_checked_at"] = json!(now_secs()));
    *LAST.lock().unwrap() = Some(out.clone());
    out
}

/// Состояние для окна настроек без обращения к сети.
pub fn status() -> Value {
    let mut v = LAST.lock().unwrap().clone().unwrap_or_else(|| json!({"current": VERSION, "latest": VERSION, "available": false, "error": ""}));
    v["current"] = json!(VERSION);
    v
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Скачивает установщик выпуска и запускает его тихо; он сам закроет приложение, заменит файлы и запустит новую версию.
pub fn install(url: &str) -> Result<(), String> {
    if !url.starts_with("https://github.com/FalconTechDev/mini-player/") {
        return Err("неизвестный адрес обновления".into());
    }
    let path = std::env::temp_dir().join("MiniPlayer_Setup_update.exe");
    net::download(url, &path, 180).map_err(|e| format!("не удалось скачать: {e}"))?;
    std::process::Command::new(&path)
        .args(["--silent"])
        .creation_flags(0x08000000) // без окна консоли
        .spawn()
        .map_err(|e| format!("не удалось запустить установщик: {e}"))?;
    Ok(())
}

/// Фоновая проверка: при включённой настройке раз в 12 часов смотрит GitHub и ставит новую версию.
pub fn start_background() {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(25)); // не мешаем запуску
        loop {
            if data::ui("auto_update").as_bool().unwrap_or(false) {
                let last = data::ui("update_checked_at").as_u64().unwrap_or(0);
                if now_secs().saturating_sub(last) >= CHECK_EVERY_SECS {
                    let r = check();
                    if r["available"].as_bool().unwrap_or(false) {
                        if install(r["url"].as_str().unwrap_or("")).is_ok() {
                            return; // установщик закроет это приложение
                        }
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_versions() {
        assert!(is_newer("v2.1.0", "2.0.1"));
        assert!(is_newer("2.0.10", "2.0.9"));
        assert!(is_newer("3", "2.9.9"));
        assert!(!is_newer("2.0.1", "2.0.1"));
        assert!(!is_newer("2.1", "2.1.0"));
        assert!(!is_newer("1.9.9", "2.0.0"));
    }
}
