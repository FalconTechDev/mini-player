//! Сеть: разбор ссылок, oEmbed, поиск YouTube / Twitch / TikTok. Запросы идут через прокси режима обхода, если он включён.
use regex::Regex;
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

static PROXY: OnceLock<Mutex<Option<String>>> = OnceLock::new();

pub fn set_proxy(url: Option<String>) {
    *PROXY.get_or_init(|| Mutex::new(None)).lock().unwrap() = url;
}

/// Системный прокси Windows (например, от VPN-клиента): как у браузера, чтобы запросы приложения шли тем же путём.
fn system_proxy() -> Option<String> {
    use std::process::Command;
    use std::os::windows::process::CommandExt;
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let q = |name: &str| -> Option<String> {
                let out = Command::new("reg")
                    .args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings", "/v", name])
                    .creation_flags(0x08000000)
                    .output()
                    .ok()?;
                let text = String::from_utf8_lossy(&out.stdout).to_string();
                text.lines().find(|l| l.trim_start().starts_with(name)).map(|l| l.split_whitespace().last().unwrap_or("").to_string())
            };
            if q("ProxyEnable")?.trim_start_matches("0x") == "0" {
                return None;
            }
            let srv = q("ProxyServer")?;
            if srv.is_empty() {
                return None;
            }
            let pick = if srv.contains('=') {
                srv.split(';').find_map(|p| p.strip_prefix("https=")).or_else(|| srv.split(';').find_map(|p| p.strip_prefix("http="))).unwrap_or("").to_string()
            } else {
                srv
            };
            if pick.is_empty() {
                None
            } else if pick.contains("://") {
                Some(pick)
            } else {
                Some(format!("http://{pick}"))
            }
        })
        .clone()
}

fn agent(timeout: u64) -> ureq::Agent {
    let mut b = ureq::AgentBuilder::new().timeout(Duration::from_secs(timeout)).user_agent(UA);
    let p = PROXY.get_or_init(|| Mutex::new(None)).lock().unwrap().clone().or_else(system_proxy);
    if let Some(p) = p {
        if let Ok(px) = ureq::Proxy::new(&p) {
            b = b.proxy(px);
        }
    }
    b.build()
}

pub fn get_text(url: &str, timeout: u64, lang: bool) -> Option<String> {
    let mut r = agent(timeout).get(url);
    if lang {
        r = r.set("Accept-Language", "ru-RU,ru;q=0.9");
    }
    r.call().ok()?.into_string().ok()
}

/// Скачивает файл по адресу (с перенаправлениями) в `path`.
pub fn download(url: &str, path: &std::path::Path, timeout: u64) -> Result<(), String> {
    let resp = agent(timeout).get(url).call().map_err(|e| e.to_string())?;
    let mut file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    std::io::copy(&mut resp.into_reader(), &mut file).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn fetch_json(url: &str) -> Value {
    get_text(url, 5, false).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}))
}

/// Конечный адрес после перенаправлений (короткие ссылки TikTok).
pub fn resolve_redirect(url: &str) -> String {
    match agent(6).get(url).call() {
        Ok(r) => r.get_url().to_string(),
        Err(_) => url.to_string(),
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// Разбор ссылки с названием и автором (запрос oEmbed) -> {platform, kind, src, vertical, canonical, title, ...} или null.
pub fn parse(link: &str) -> Value {
    parse_with(link, true)
}

/// Название и автор по готовому описанию (YouTube, TikTok): запрашивается отдельно, чтобы плеер открывался мгновенно.
pub fn enrich(info: &Value) -> Value {
    let canon = s(info, "canonical");
    let url = match s(info, "platform") {
        "YouTube" => format!("https://www.youtube.com/oembed?format=json&url={}", urlencoding::encode(canon)),
        "TikTok" => format!("https://www.tiktok.com/oembed?url={}", urlencoding::encode(canon)),
        _ => return json!({}),
    };
    let oe = fetch_json(&url);
    json!({"title": s(&oe, "title"), "channel": s(&oe, "author_name"), "thumb": s(&oe, "thumbnail_url")})
}

/// Разбор ссылки; fetch=false не обращается к сети за названием (короткие ссылки TikTok всё равно раскрываются).
pub fn parse_with(link: &str, fetch: bool) -> Value {
    let mut link = link.trim().to_string();
    if link.is_empty() {
        return Value::Null;
    }
    if !Regex::new(r"^https?://").unwrap().is_match(&link) {
        link = format!("https://{link}");
    }
    let host = link.splitn(4, '/').nth(2).unwrap_or("").to_lowercase();

    // YouTube
    if host.contains("youtu") {
        if let Some(m) = Regex::new(r"(?:v=|youtu\.be/|shorts/|embed/|live/)([\w-]{11})").unwrap().captures(&link) {
            let vid = m[1].to_string();
            let mut src = format!("https://www.youtube.com/embed/{vid}?autoplay=1&rel=0");
            let t = Regex::new(r"[?&]t=(\d+)").unwrap().captures(&link).map(|c| c[1].to_string());
            if let Some(t) = &t {
                src += &format!("&start={t}");
            }
            let pl = Regex::new(r"[?&]list=([\w-]+)").unwrap().captures(&link).map(|c| c[1].to_string());
            if let Some(p) = &pl {
                src += &format!("&list={p}");
            }
            let canon = format!("https://www.youtube.com/watch?v={vid}");
            let oe = if fetch { fetch_json(&format!("https://www.youtube.com/oembed?format=json&url={}", urlencoding::encode(&canon))) } else { json!({}) };
            let title = if s(&oe, "title").is_empty() { canon.clone() } else { s(&oe, "title").to_string() };
            return json!({"platform": "YouTube", "kind": "iframe", "src": src, "vertical": link.contains("/shorts/"), "vid": vid,
                "start": t.and_then(|x| x.parse::<i64>().ok()).unwrap_or(0), "list": pl.unwrap_or_default(),
                "canonical": canon, "title": title, "channel": s(&oe, "author_name"),
                "thumb": format!("https://i.ytimg.com/vi/{vid}/mqdefault.jpg")});
        }
    }

    // TikTok
    if host.contains("tiktok.com") {
        if !link.contains("/video/") {
            link = resolve_redirect(&link);
        }
        if let Some(m) = Regex::new(r"/video/(\d+)").unwrap().captures(&link) {
            let clean = link.split('?').next().unwrap_or("").to_string();
            let oe = if fetch { fetch_json(&format!("https://www.tiktok.com/oembed?url={}", urlencoding::encode(&clean))) } else { json!({}) };
            let title = if s(&oe, "title").is_empty() { clean.clone() } else { s(&oe, "title").to_string() };
            return json!({"platform": "TikTok", "kind": "iframe",
                "src": format!("https://www.tiktok.com/player/v1/{}?autoplay=1&loop=1&rel=0", &m[1]),
                "vertical": true, "canonical": clean, "title": title, "channel": s(&oe, "author_name"),
                "thumb": s(&oe, "thumbnail_url")});
        }
    }

    // Vimeo
    if let Some(m) = Regex::new(r"vimeo\.com/(?:.*/)?(\d+)").unwrap().captures(&link) {
        return json!({"platform": "Vimeo", "kind": "iframe", "src": format!("https://player.vimeo.com/video/{}?autoplay=1", &m[1]),
            "vertical": false, "canonical": link, "title": link});
    }

    // Twitch (канал)
    let nq = link.split('?').next().unwrap_or("");
    if let Some(m) = Regex::new(r"twitch\.tv/(\w+)/?$").unwrap().captures(nq) {
        let ch = m[1].to_string();
        return json!({"platform": "Twitch", "kind": "iframe",
            "src": format!("https://player.twitch.tv/?channel={ch}&parent=127.0.0.1&autoplay=true"),
            "vertical": false, "canonical": link, "title": format!("Twitch: {ch}"),
            "thumb": format!("https://static-cdn.jtvnw.net/previews-ttv/live_user_{}-440x248.jpg", ch.to_lowercase())});
    }

    // прямой видеофайл
    if Regex::new(r"(?i)\.(mp4|webm|mkv|mov|ogv)(\?|$)").unwrap().is_match(&link) {
        return json!({"platform": "Файл", "kind": "video", "src": link, "vertical": false, "canonical": link,
            "title": link.rsplit('/').next().unwrap_or("")});
    }

    // любой другой сайт
    json!({"platform": "Сайт", "kind": "web", "src": link, "vertical": false, "canonical": link, "title": host})
}

fn walk(o: &Value, key: &str, out: &mut Vec<Value>) {
    match o {
        Value::Object(m) => {
            if let Some(v) = m.get(key) {
                out.push(v.clone());
            }
            for v in m.values() {
                walk(v, key, out);
            }
        }
        Value::Array(a) => {
            for v in a {
                walk(v, key, out);
            }
        }
        _ => {}
    }
}

fn txt(x: &Value) -> String {
    if let Some(t) = x.get("simpleText").and_then(|t| t.as_str()) {
        return t.to_string();
    }
    x.get("runs").and_then(|r| r.as_array()).map(|r| r.iter().map(|i| s(i, "text")).collect::<String>()).unwrap_or_default()
}

pub fn search_youtube(q: &str) -> Value {
    let html = match get_text(&format!("https://www.youtube.com/results?hl=ru&search_query={}", urlencoding::encode(q)), 10, true) {
        Some(h) => h,
        None => return Value::Null,
    };
    let re = Regex::new(r"(?s)var ytInitialData = (\{.*?\});</script>").unwrap();
    let data: Value = match re.captures(&html).and_then(|m| serde_json::from_str(&m[1]).ok()) {
        Some(d) => d,
        None => return Value::Null,
    };
    let mut vr = vec![];
    walk(&data, "videoRenderer", &mut vr);
    let mut res: Vec<Value> = vec![];
    for v in vr {
        let vid = s(&v, "videoId").to_string();
        if vid.is_empty() || res.iter().any(|r| s(r, "url").ends_with(&vid)) {
            continue;
        }
        let live = v.get("badges").map(|b| b.to_string().contains("LIVE")).unwrap_or(false);
        let owner = if txt(&v["ownerText"]).is_empty() { txt(&v["longBylineText"]) } else { txt(&v["ownerText"]) };
        res.push(json!({"url": format!("https://www.youtube.com/watch?v={vid}"), "title": txt(&v["title"]), "channel": owner,
            "dur": if live { "LIVE".to_string() } else { txt(&v["lengthText"]) }, "views": txt(&v["viewCountText"]),
            "thumb": format!("https://i.ytimg.com/vi/{vid}/mqdefault.jpg"), "platform": "YouTube"}));
        if res.len() >= 12 {
            break;
        }
    }
    Value::Array(res)
}

const TW_GQL: &str = "query($q:String!){searchFor(userQuery:$q,platform:\"web\",options:{targets:[{index:CHANNEL}]}){channels{items{login displayName stream{viewersCount game{displayName}}}}}}";

pub fn search_twitch(q: &str) -> Value {
    let body = json!({"query": TW_GQL, "variables": {"q": q}});
    let r = agent(10).post("https://gql.twitch.tv/gql").set("Client-ID", "kimne78kd3eoqrcdpqpfg1rskidb3b").send_json(body);
    let j: Value = match r.ok().and_then(|r| r.into_json().ok()) {
        Some(j) => j,
        None => return Value::Null,
    };
    let items = j["data"]["searchFor"]["channels"]["items"].as_array().cloned().unwrap_or_default();
    Value::Array(
        items
            .iter()
            .take(12)
            .map(|c| {
                let st = &c["stream"];
                let live = !st.is_null();
                json!({"url": format!("https://www.twitch.tv/{}", s(c, "login")), "title": s(c, "displayName"),
                    "channel": if live { st["game"]["displayName"].as_str().unwrap_or("Не в эфире").to_string() } else { "Не в эфире".to_string() },
                    "dur": if live { "LIVE" } else { "" },
                    "views": if live { format!("{} зрителей", st["viewersCount"]) } else { String::new() },
                    "thumb": format!("https://static-cdn.jtvnw.net/previews-ttv/live_user_{}-440x248.jpg", s(c, "login").to_lowercase()),
                    "platform": "Twitch"})
            })
            .collect(),
    )
}

/// Данные карточки TikTok по ссылке на видео (oEmbed).
pub fn tiktok_card(url: &str, alt: &str) -> Value {
    let oe = fetch_json(&format!("https://www.tiktok.com/oembed?url={}", urlencoding::encode(url)));
    let cap = Regex::new(r"(?s)«(.*?)» от автора").unwrap().captures(alt).map(|c| c[1].to_string());
    let title = if !s(&oe, "title").is_empty() { s(&oe, "title").to_string() } else { cap.unwrap_or_else(|| url.to_string()) };
    json!({"url": url, "title": title, "channel": s(&oe, "author_name"), "dur": "", "views": "", "thumb": s(&oe, "thumbnail_url"), "platform": "TikTok"})
}
