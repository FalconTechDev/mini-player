//! Аккаунты платформ: состояние входа по cookie, до четырёх аккаунтов на платформу, переключение между ними.
//! Сеансы (cookie) хранятся в accounts.bin, зашифрованные DPAPI Windows: расшифровать может только ваша учётная запись.
use crate::state::*;
use crate::{api, data, rawcookie};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::channel;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

struct Login {
    sites: &'static [&'static str],
    need: &'static [&'static str],
    name_cookie: Option<&'static str>,
}

fn login_sites(pid: &str) -> Option<Login> {
    Some(match pid {
        "yt" => Login { sites: &["https://www.youtube.com", "https://www.google.com"], need: &["SAPISID", "__Secure-3PAPISID", "LOGIN_INFO"], name_cookie: None },
        "tt" => Login { sites: &["https://www.tiktok.com"], need: &["sessionid", "sid_tt"], name_cookie: None },
        "tw" => Login { sites: &["https://www.twitch.tv"], need: &["auth-token"], name_cookie: Some("login") },
        _ => return None,
    })
}

fn acc_sites(pid: &str) -> &'static [&'static str] {
    match pid {
        "yt" => &["https://www.youtube.com", "https://www.google.com", "https://accounts.google.com", "https://myaccount.google.com", "https://www.googleapis.com"],
        "tt" => &["https://www.tiktok.com"],
        "tw" => &["https://www.twitch.tv"],
        _ => &[],
    }
}

fn login_page(pid: &str) -> &'static str {
    match pid {
        "yt" => "https://accounts.google.com/ServiceLogin?service=youtube&continue=https://www.youtube.com/",
        "tt" => "https://www.tiktok.com/login",
        _ => "https://www.twitch.tv/login",
    }
}

const TT_JS: &str = r#"(()=>{const a=document.querySelector('[data-e2e="nav-profile"]');if(!a)return null;
 const l=a.closest('a')||a.querySelector('a')||a;const href=l.getAttribute&&l.getAttribute('href')||'';
 if(!/^\/@/.test(href))return null;const im=a.querySelector('img');
 return JSON.stringify({name:'',handle:href.slice(1),avatar:im?im.src:''})})()"#;
const YT_START: &str = r#"(async()=>{try{window.__acc=null;const c=window.ytcfg;const key=c.get('INNERTUBE_API_KEY'),ctx=c.get('INNERTUBE_CONTEXT');
 const sid=(document.cookie.match(/(?:^|; )SAPISID=([^;]+)/)||[])[1];const ts=Math.floor(Date.now()/1000);
 const buf=new TextEncoder().encode(ts+' '+sid+' https://www.youtube.com');
 const h=[...new Uint8Array(await crypto.subtle.digest('SHA-1',buf))].map(b=>b.toString(16).padStart(2,'0')).join('');
 const r=await fetch('/youtubei/v1/account/account_menu?key='+key,{method:'POST',credentials:'include',headers:{'Content-Type':'application/json','Authorization':'SAPISIDHASH '+ts+'_'+h,'X-Origin':'https://www.youtube.com','X-Goog-AuthUser':'0'},body:JSON.stringify({context:ctx})});
 const j=await r.json();const hd=j.actions[0].openPopupAction.popup.multiPageMenuRenderer.header.activeAccountHeaderRenderer;
 window.__acc=JSON.stringify({name:hd.accountName.simpleText,handle:(hd.channelHandle||{}).simpleText||'',avatar:hd.accountPhoto.thumbnails[0].url})}catch(e){window.__acc='ERR '+e}})();1"#;
const TW_JS: &str = r#"(()=>{const b=document.querySelector('[data-a-target="user-menu-toggle"]');if(!b)return null;
 const im=b.querySelector('img');return JSON.stringify({name:'',handle:'',avatar:im?im.src:''})})()"#;

static COOKIE_LOCK: Mutex<()> = Mutex::new(());
static ACC_LOCK: Mutex<()> = Mutex::new(());
static PENDING: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static BUSY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static REG_BUSY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static CACHE: OnceLock<Mutex<HashMap<String, Value>>> = OnceLock::new();

fn set_of(c: &'static OnceLock<Mutex<HashSet<String>>>) -> &'static Mutex<HashSet<String>> {
    c.get_or_init(|| Mutex::new(HashSet::new()))
}

// ---------- cookie профиля (через главное окно) ----------
fn main_win() -> Option<WinId> {
    app().main
}

/// Cookie для сайтов; с delete=true удаляет их из профиля.
pub fn cookies(sites: &[&str], delete: bool) -> Result<Vec<Value>, String> {
    let _g = COOKIE_LOCK.lock().map_err(|_| "cookie заняты")?;
    let win = main_win().ok_or("нет окна")?;
    let (tx, rx) = channel::<Vec<Value>>();
    let sites: Vec<String> = sites.iter().map(|s| s.to_string()).collect();
    send(Ev::With {
        win,
        f: Box::new(move |wv| {
            let mut rows: HashMap<(String, String, String), Value> = HashMap::new();
            for s in &sites {
                if let Ok(list) = rawcookie::read(wv, s, delete) {
                    for r in list {
                        let key = (r["name"].as_str().unwrap_or("").to_string(), r["domain"].as_str().unwrap_or("").to_string(), r["path"].as_str().unwrap_or("").to_string());
                        rows.insert(key, r);
                    }
                }
            }
            let _ = tx.send(rows.into_values().collect());
        }),
    });
    rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "cookie не получены".to_string())
}

fn restore(items: &[Value]) -> Result<(), String> {
    let win = main_win().ok_or("нет окна")?;
    let (tx, rx) = channel::<Result<(), String>>();
    let items: Vec<Value> = items.to_vec();
    send(Ev::With {
        win,
        f: Box::new(move |wv| {
            let _ = tx.send(rawcookie::write(wv, &items));
        }),
    });
    rx.recv_timeout(Duration::from_secs(20)).map_err(|_| "cookie не записаны".to_string())?
}

// ---------- шифрование DPAPI ----------
pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
    crate::winapi::dpapi(data, true)
}
pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    crate::winapi::dpapi(data, false)
}

fn store_path() -> std::path::PathBuf {
    data::data_dir().join("accounts.bin")
}

fn store_load() -> Value {
    std::fs::read(store_path())
        .ok()
        .and_then(|b| unprotect(&b))
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .filter(|v| v.is_object())
        .unwrap_or(json!({}))
}

fn store_save(st: &Value) {
    let _ = std::fs::create_dir_all(data::data_dir());
    if let Some(enc) = protect(st.to_string().as_bytes()) {
        let _ = std::fs::write(store_path(), enc);
    }
}

// ---------- состояние входа ----------
pub fn login_state() -> Value {
    let mut res = serde_json::Map::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    for pid in ["yt", "tt", "tw"] {
        let l = login_sites(pid).unwrap();
        match cookies(l.sites, false) {
            Ok(rows) => {
                let names: HashMap<String, String> = rows.iter().map(|r| (r["name"].as_str().unwrap_or("").to_string(), r["value"].as_str().unwrap_or("").to_string())).collect();
                let v = json!({"in": l.need.iter().any(|n| names.contains_key(*n)),
                               "name": l.name_cookie.and_then(|c| names.get(c)).cloned().unwrap_or_default()});
                cache.lock().unwrap().insert(pid.to_string(), v.clone());
                res.insert(pid.to_string(), v);
            }
            Err(_) => {
                let mut v = cache.lock().unwrap().get(pid).cloned().unwrap_or(json!({"in": false, "name": ""}));
                v["unknown"] = json!(true);
                res.insert(pid.to_string(), v);
            }
        }
    }
    Value::Object(res)
}

fn acc_public(d: &mut Value, pid: &str) -> Value {
    if !d["acc"].is_object() {
        d["acc"] = json!({});
    }
    if !d["acc"][pid].is_object() {
        d["acc"][pid] = json!({"active": null, "list": []});
    }
    d["acc"][pid].clone()
}

pub fn accounts() -> Value {
    let state = login_state();
    // убрать пустые записи без имени и ника
    let blanks: Vec<(String, Vec<String>)> = {
        let mut d = data::load();
        let pids: Vec<String> = d["acc"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
        pids.into_iter()
            .map(|pid| {
                let pub0 = acc_public(&mut d, &pid);
                let ids: Vec<String> = pub0["list"].as_array().cloned().unwrap_or_default().iter()
                    .filter(|a| a["name"].as_str().unwrap_or("").is_empty() && a["handle"].as_str().unwrap_or("").is_empty())
                    .filter_map(|a| a["id"].as_str().map(|s| s.to_string())).collect();
                (pid, ids)
            })
            .filter(|(_, ids)| !ids.is_empty())
            .collect()
    };
    if !blanks.is_empty() {
        data::update(|d| {
            for (pid, ids) in &blanks {
                let pubd = &mut d["acc"][pid.as_str()];
                let list: Vec<Value> = pubd["list"].as_array().cloned().unwrap_or_default().into_iter().filter(|a| !ids.iter().any(|i| a["id"] == *i)).collect();
                let act_ok = pubd["active"].is_null() || list.iter().any(|a| a["id"] == pubd["active"]);
                pubd["list"] = Value::Array(list);
                if !act_ok {
                    pubd["active"] = Value::Null;
                }
            }
        });
        let mut st = store_load();
        for (pid, ids) in &blanks {
            for i in ids {
                if let Some(o) = st[pid.as_str()].as_object_mut() {
                    o.remove(i);
                }
            }
        }
        store_save(&st);
    }
    let mut d = data::load();
    let pending: Vec<String> = set_of(&PENDING).lock().unwrap().iter().cloned().collect();
    for pid in ["yt", "tt", "tw"] {
        let st = &state[pid];
        let pubd = acc_public(&mut d, pid);
        let is_pending = pending.iter().any(|p| p == pid);
        let busy = set_of(&BUSY).lock().unwrap().contains(pid);
        if st["in"].as_bool().unwrap_or(false) && pubd["active"].is_null() && !is_pending && !busy {
            let pid2 = pid.to_string();
            std::thread::spawn(move || {
                register_current(&pid2);
            });
        } else if !st["in"].as_bool().unwrap_or(false) && st["unknown"].is_null() && !pubd["active"].is_null() && !is_pending {
            d["acc"][pid]["active"] = Value::Null;
        }
    }
    let mut acc = serde_json::Map::new();
    if let Some(o) = d["acc"].as_object() {
        for (pid, v) in o {
            acc.insert(pid.clone(), json!({"active": v["active"], "list": v["list"]}));
        }
    }
    let mut pend = pending;
    pend.sort();
    json!({"login": state, "acc": acc, "pending": pend})
}

// ---------- данные аккаунта (ник и аватар) со страницы сайта ----------
fn fetch_account(pid: &str) -> Option<Value> {
    {
        let mut b = set_of(&BUSY).lock().unwrap();
        if b.contains(pid) {
            return None;
        }
        b.insert(pid.to_string());
    }
    let (url, start_js, poll_js): (&str, Option<&str>, &str) = match pid {
        "tt" => ("https://www.tiktok.com/foryou", None, TT_JS),
        "yt" => ("https://www.youtube.com", Some(YT_START), "window.__acc"),
        _ => ("https://www.twitch.tv", None, TW_JS),
    };
    let win = api::hidden_window(url);
    std::thread::sleep(Duration::from_secs(4));
    let mut result: Option<Value> = None;
    for i in 0..30 {
        if i == 0 {
            if let Some(js) = start_js {
                let _ = eval_result(win, js, 3000);
            }
        }
        if let Some(r) = eval_result(win, poll_js, 3000) {
            if let Ok(v) = serde_json::from_str::<Value>(&r) {
                if let Some(s) = v.as_str() {
                    if !s.starts_with("ERR") {
                        if let Ok(o) = serde_json::from_str::<Value>(s) {
                            result = Some(o);
                            break;
                        }
                    }
                } else if v.is_object() {
                    result = Some(v);
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(700));
    }
    set_of(&BUSY).lock().unwrap().remove(pid);
    send(Ev::Close(win));
    result
}

fn register_current(pid: &str) -> Option<String> {
    {
        let mut b = set_of(&REG_BUSY).lock().unwrap();
        if b.contains(pid) {
            return None;
        }
        b.insert(pid.to_string());
    }
    let r = register_inner(pid);
    set_of(&REG_BUSY).lock().unwrap().remove(pid);
    r
}

fn register_inner(pid: &str) -> Option<String> {
    let info = fetch_account(pid)?;
    let (name, handle, avatar) = (info["name"].as_str().unwrap_or("").to_string(), info["handle"].as_str().unwrap_or("").to_string(), info["avatar"].as_str().unwrap_or("").to_string());
    if name.is_empty() && handle.is_empty() {
        return None;
    }
    let rows = cookies(acc_sites(pid), false).ok()?;
    if rows.is_empty() {
        return None;
    }
    let _g = ACC_LOCK.lock().unwrap();
    let id = data::update(|d| {
        let mut pubd = acc_public(d, pid);
        let list = pubd["list"].as_array().cloned().unwrap_or_default();
        let ident = if handle.is_empty() { &name } else { &handle };
        let existing = list.iter().find(|a| !ident.is_empty() && a["handle"] == handle.as_str() && a["name"] == name.as_str()).cloned();
        let id = existing.as_ref().and_then(|a| a["id"].as_str().map(|s| s.to_string())).unwrap_or_else(|| format!("a{}", data::now() * 1000 + (std::process::id() as i64 % 1000)));
        let mut list: Vec<Value> = list.into_iter().filter(|a| a["id"] != id.as_str()).collect();
        list.push(json!({"id": id, "name": name, "handle": handle, "avatar": avatar}));
        pubd["list"] = Value::Array(list);
        pubd["active"] = json!(id);
        d["acc"][pid] = pubd;
        id
    });
    let mut st = store_load();
    st[pid][&id] = Value::Array(rows);
    store_save(&st);
    Some(id)
}

fn snapshot_active() {
    // вызывается под ACC_LOCK
}

fn snapshot_active_for(pid: &str) {
    let act = {
        let mut d = data::load();
        acc_public(&mut d, pid)["active"].as_str().map(|s| s.to_string())
    };
    let Some(act) = act else { return };
    if let Ok(rows) = cookies(acc_sites(pid), false) {
        if !rows.is_empty() {
            let mut st = store_load();
            st[pid][&act] = Value::Array(rows);
            store_save(&st);
        }
    }
}

fn switch_to(pid: &str, aid: &str) -> Result<(), String> {
    let _g = ACC_LOCK.lock().unwrap();
    let st = store_load();
    let target = st[pid][aid].as_array().cloned().ok_or("нет сохранённого сеанса")?;
    snapshot_active_for(pid);
    let _ = cookies(acc_sites(pid), true);
    std::thread::sleep(Duration::from_millis(600));
    restore(&target)?;
    data::update(|d| {
        let mut p = acc_public(d, pid);
        p["active"] = json!(aid);
        d["acc"][pid] = p;
    });
    snapshot_active();
    Ok(())
}

pub fn acc_switch_api(pid: &str, aid: &str) -> Value {
    match switch_to(pid, aid) {
        Ok(()) => accounts(),
        Err(e) => {
            let mut v = accounts();
            v["error"] = json!(e);
            v
        }
    }
}

fn watch_login(pid: String, fallback: Option<String>) {
    set_of(&PENDING).lock().unwrap().insert(pid.clone());
    for _ in 0..100 {
        std::thread::sleep(Duration::from_secs(3));
        if login_state()[pid.as_str()]["in"].as_bool().unwrap_or(false) {
            std::thread::sleep(Duration::from_secs(2));
            register_current(&pid);
            set_of(&PENDING).lock().unwrap().remove(&pid);
            return;
        }
    }
    if let Some(f) = fallback {
        let _ = switch_to(&pid, &f);
    }
    set_of(&PENDING).lock().unwrap().remove(&pid);
}

/// Начать добавление аккаунта: текущий сеанс сохраняется, окно входа открывается чистым.
pub fn acc_add(pid: &str) -> Value {
    let mut cur = {
        let mut d = data::load();
        acc_public(&mut d, pid)["active"].as_str().map(|s| s.to_string())
    };
    if cur.is_none() && login_state()[pid]["in"].as_bool().unwrap_or(false) {
        cur = register_current(pid);
    }
    let count = {
        let mut d = data::load();
        acc_public(&mut d, pid)["list"].as_array().map(|l| l.len()).unwrap_or(0)
    };
    if count >= 4 {
        let mut v = accounts();
        v["error"] = json!("Не больше 4 аккаунтов");
        return v;
    }
    {
        let _g = ACC_LOCK.lock().unwrap();
        snapshot_active_for(pid);
        let _ = cookies(acc_sites(pid), true);
    }
    let pid2 = pid.to_string();
    std::thread::spawn(move || watch_login(pid2, cur));
    api::open_site(login_page(pid), pid == "tt");
    accounts()
}

pub fn acc_remove(pid: &str, aid: &str) -> Value {
    {
        let _g = ACC_LOCK.lock().unwrap();
        let (was_active, first) = data::update(|d| {
            let mut p = acc_public(d, pid);
            let was = p["active"] == aid;
            let list: Vec<Value> = p["list"].as_array().cloned().unwrap_or_default().into_iter().filter(|a| a["id"] != aid).collect();
            let first = list.first().and_then(|a| a["id"].as_str().map(|s| s.to_string()));
            p["list"] = Value::Array(list);
            if was {
                p["active"] = Value::Null;
            }
            d["acc"][pid] = p;
            (was, first)
        });
        let mut st = store_load();
        if let Some(o) = st[pid].as_object_mut() {
            o.remove(aid);
        }
        store_save(&st);
        if was_active {
            let _ = cookies(acc_sites(pid), true);
            drop(_g);
            if let Some(f) = first {
                let _ = switch_to(pid, &f);
            }
        }
    }
    accounts()
}

pub fn logout(pid: &str) -> Value {
    {
        let _g = ACC_LOCK.lock().unwrap();
        let _ = cookies(acc_sites(pid), true);
        data::update(|d| {
            d["acc"][pid] = json!({"active": null, "list": []});
        });
        let mut st = store_load();
        if let Some(o) = st.as_object_mut() {
            o.remove(pid);
        }
        store_save(&st);
    }
    accounts()
}
