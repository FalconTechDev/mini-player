//! Cookie WebView2 напрямую через COM: домен остаётся точно таким, каким его хранит браузер (с ведущей точкой),
//! иначе cookie «на весь домен» превращаются в cookie «только этого хоста» и вход в аккаунт теряется.
use serde_json::{json, Value};
use std::sync::mpsc;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{take_pwstr, GetCookiesCompletedHandler};
use windows::core::{Interface, BOOL, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::E_UNEXPECTED;
use wry::cookie::time::format_description::well_known::Rfc3339;
use wry::cookie::time::OffsetDateTime;
use wry::{WebView, WebViewExtWindows};

fn manager(wv: &WebView) -> Result<ICoreWebView2CookieManager, String> {
    let w2 = wv.webview().cast::<ICoreWebView2_2>().map_err(|e| e.to_string())?;
    unsafe { w2.CookieManager().map_err(|e| e.to_string()) }
}

unsafe fn row_of(c: &ICoreWebView2Cookie) -> windows::core::Result<Value> {
    let (mut name, mut value, mut domain, mut path) = (PWSTR::null(), PWSTR::null(), PWSTR::null(), PWSTR::null());
    c.Name(&mut name)?;
    c.Value(&mut value)?;
    c.Domain(&mut domain)?;
    c.Path(&mut path)?;
    let (mut http, mut secure, mut session): (BOOL, BOOL, BOOL) = (false.into(), false.into(), false.into());
    c.IsHttpOnly(&mut http)?;
    c.IsSecure(&mut secure)?;
    c.IsSession(&mut session)?;
    let mut same = COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX;
    c.SameSite(&mut same)?;
    let mut exp = 0.0f64;
    c.Expires(&mut exp)?;
    let expires = if session.as_bool() || exp < 0.0 {
        Value::Null
    } else {
        OffsetDateTime::from_unix_timestamp(exp as i64).ok().and_then(|t| t.format(&Rfc3339).ok()).map(Value::String).unwrap_or(Value::Null)
    };
    Ok(json!({"name": take_pwstr(name), "value": take_pwstr(value), "domain": take_pwstr(domain), "path": take_pwstr(path),
              "expires": expires, "http": http.as_bool(), "secure": secure.as_bool(),
              "same": if same == COREWEBVIEW2_COOKIE_SAME_SITE_KIND_NONE { 0 } else if same == COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT { 2 } else { 1 }}))
}

/// Cookie для адреса `uri`; с delete=true удаляет найденные из профиля.
pub fn read(wv: &WebView, uri: &str, delete: bool) -> Result<Vec<Value>, String> {
    let mgr = manager(wv)?;
    let (tx, rx) = mpsc::channel::<windows::core::Result<Vec<Value>>>();
    let mgr2 = mgr.clone();
    let uri = HSTRING::from(uri);
    unsafe {
        mgr.GetCookies(
            PCWSTR::from_raw(uri.as_ptr()),
            &GetCookiesCompletedHandler::create(Box::new(move |err, list| {
                let res = (|| -> windows::core::Result<Vec<Value>> {
                    err?;
                    let mut out = vec![];
                    if let Some(list) = list {
                        let mut count = 0;
                        list.Count(&mut count)?;
                        for i in 0..count {
                            let c = list.GetValueAtIndex(i)?;
                            if let Ok(r) = row_of(&c) {
                                out.push(r);
                            }
                            if delete {
                                let _ = mgr2.DeleteCookie(&c);
                            }
                        }
                    }
                    Ok(out)
                })();
                tx.send(res).map_err(|_| windows::core::Error::from(E_UNEXPECTED))
            })),
        )
        .map_err(|e| e.to_string())?;
    }
    webview2_com::wait_with_pump(rx).map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}

/// Записывает cookie (строки в том же виде, что возвращает `read`).
pub fn write(wv: &WebView, rows: &[Value]) -> Result<(), String> {
    let mgr = manager(wv)?;
    for r in rows {
        unsafe {
            let c = mgr
                .CreateCookie(
                    &HSTRING::from(r["name"].as_str().unwrap_or("")),
                    &HSTRING::from(r["value"].as_str().unwrap_or("")),
                    &HSTRING::from(r["domain"].as_str().unwrap_or("")),
                    &HSTRING::from(r["path"].as_str().unwrap_or("/")),
                )
                .map_err(|e| e.to_string())?;
            if let Some(e) = r["expires"].as_str() {
                if let Ok(t) = OffsetDateTime::parse(e, &Rfc3339) {
                    let _ = c.SetExpires(t.unix_timestamp() as f64);
                }
            }
            let _ = c.SetIsHttpOnly(r["http"].as_bool().unwrap_or(false));
            let _ = c.SetIsSecure(r["secure"].as_bool().unwrap_or(false));
            let _ = c.SetSameSite(match r["same"].as_i64().unwrap_or(1) {
                0 => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_NONE,
                2 => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT,
                _ => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX,
            });
            let _ = mgr.AddOrUpdateCookie(&c);
        }
    }
    Ok(())
}
