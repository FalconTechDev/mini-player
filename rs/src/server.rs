//! Локальный сервер страниц приложения (главное окно, плеер, панели). Страницы YouTube требуют обычный http-адрес.
use std::collections::HashMap;
use tiny_http::{Header, Response, Server};

pub const UI_HTML: &str = include_str!("../assets/ui.html");
pub const PLAYER_HTML: &str = include_str!("../assets/player.html");
pub const PANEL_HTML: &str = include_str!("../assets/panel.html");
pub const SPLASH_HTML: &str = include_str!("../assets/splash.html");
pub const TIKTOK_JS: &str = include_str!("../assets/tiktok_inject.js");

fn query(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            let dec = |s: &str| urlencoding::decode(&s.replace('+', " ")).map(|c| c.into_owned()).unwrap_or_default();
            (dec(k), dec(v))
        })
        .collect()
}

fn html(body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_data(body.into_bytes()).with_header(Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap())
}

fn player_page(q: &HashMap<String, String>) -> String {
    let src = q.get("src").cloned().unwrap_or_default().replace('"', "");
    let kind = q.get("kind").map(|s| s.as_str()).unwrap_or("iframe");
    let media = if kind == "video" {
        format!(r#"<video id="v" src="{src}" controls autoplay loop></video>"#)
    } else if q.get("platform").map(|s| s.as_str()) == Some("YouTube") && q.get("vid").map(|s| !s.is_empty()).unwrap_or(false) {
        r#"<div id="v"></div>"#.to_string()
    } else {
        format!(r#"<iframe id="v" src="{src}" allow="autoplay; encrypted-media; picture-in-picture; fullscreen" allowfullscreen></iframe>"#)
    };
    PLAYER_HTML.replace("__MEDIA__", &media)
}

/// Запускает сервер на свободном порту и возвращает порт.
pub fn start() -> u16 {
    let server = Server::http("127.0.0.1:0").expect("не удалось запустить локальный сервер");
    let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(0);
    std::thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            let (path, q) = url.split_once('?').unwrap_or((url.as_str(), ""));
            let body = match path {
                "/ui" => UI_HTML.to_string(),
                "/panel" => PANEL_HTML.to_string(),
                "/splash" => SPLASH_HTML.to_string(),
                _ => player_page(&query(q)),
            };
            let _ = req.respond(html(body));
        }
    });
    port
}
