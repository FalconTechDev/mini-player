//! Режим обхода блокировок на основе zapret (bol-van, MIT) и драйвера WinDivert (LGPL/GPL).
//!
//! Обход действует только внутри приложения: окна браузера и запросы идут через локальный прокси, прокси открывает
//! исходящие соединения только с зарезервированных портов PORT_LO..PORT_HI, а winws фильтрует пакеты (WinDivert)
//! только с этих портов. Остальные программы и браузеры не затрагиваются.
//! winws требует прав администратора: приложение запускает свою копию с ключом --bypass-helper через UAC (один запрос),
//! помощник следит за winws и завершает его, когда приложение закрыто.
use crate::{api, data, net, state::*};
use serde_json::{json, Value};
use socket2::{Domain, Protocol, Socket, Type};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

pub const HELPER_FLAG: &str = "--bypass-helper";
const PORT_LO: u16 = 52000;
const PORT_HI: u16 = 52999;

const STRATEGIES: [(&str, &str); 6] = [
    ("Разделение TLS", "--dpi-desync=multisplit --dpi-desync-split-pos=1,midsld"),
    ("Разделение с перекрытием", "--dpi-desync=multisplit --dpi-desync-split-seqovl=681 --dpi-desync-split-pos=1"),
    ("Подмена и разделение", "--dpi-desync=fake,multisplit --dpi-desync-fooling=badseq --dpi-desync-split-pos=1,midsld --dpi-desync-repeats=4"),
    ("Подмена и обратный порядок", "--dpi-desync=fake,multidisorder --dpi-desync-split-pos=1,midsld --dpi-desync-fooling=md5sig,badseq"),
    ("Ложное разделение", "--dpi-desync=fakedsplit --dpi-desync-split-pos=1,midsld --dpi-desync-fooling=ts"),
    ("Подмена с малым TTL", "--dpi-desync=fake --dpi-desync-ttl=5 --dpi-desync-fooling=md5sig"),
];

const PROBE_HOSTS: [&str; 2] = ["www.youtube.com", "redirector.googlevideo.com"];

/// Эти сайты идут напрямую, без обхода; всё остальное проходит через обход.
const DIRECT_DOMAINS: [&str; 21] = [
    "tiktok.com", "tiktokcdn.com", "tiktokcdn-us.com", "tiktokv.com", "tiktokw.us", "byteoversea.com", "ibytedtos.com", "ibyteimg.com",
    "muscdn.com", "bytedance.com", "ttwstatic.com", "isnssdk.com", "netflix.com", "nflxvideo.net", "nflximg.net", "nflximg.com",
    "nflxso.net", "nflxext.com", "127.0.0.1", "localhost", "msedge.net",
];

fn scoped(host: &str) -> bool {
    let h = host.to_lowercase();
    let h = h.trim_end_matches('.');
    !DIRECT_DOMAINS.iter().any(|d| h == *d || h.ends_with(&format!(".{d}")))
}

fn win_filter() -> String {
    format!(
        "!impostor and !loopback and ((outbound and tcp.DstPort==443 and tcp.SrcPort>={lo} and tcp.SrcPort<={hi}) or \
         (inbound and tcp.SrcPort==443 and tcp.DstPort>={lo} and tcp.DstPort<={hi}))",
        lo = PORT_LO,
        hi = PORT_HI
    )
}

fn bypass_dir() -> PathBuf {
    data::data_dir().join("bypass")
}

fn log(msg: &str) {
    let p = bypass_dir().join("proxy.log");
    let _ = std::fs::create_dir_all(bypass_dir());
    if std::fs::metadata(&p).map(|m| m.len() > 200_000).unwrap_or(false) {
        let _ = std::fs::remove_file(&p);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{} {msg}", data::now() % 86400);
    }
}

// ---------------------------------------------------------------- прокси
pub struct Proxy {
    pub port: u16,
    desync: Arc<AtomicBool>,
}

struct ProxyCtx {
    desync: Arc<AtomicBool>,
    next: AtomicU16,
}

impl Proxy {
    pub fn start() -> Proxy {
        let listener = TcpListener::bind("127.0.0.1:0").expect("прокси: порт занят");
        let port = listener.local_addr().unwrap().port();
        let desync = Arc::new(AtomicBool::new(false));
        let ctx = Arc::new(ProxyCtx { desync: desync.clone(), next: AtomicU16::new(PORT_LO) });
        std::thread::spawn(move || {
            for c in listener.incoming().flatten() {
                let ctx = ctx.clone();
                std::thread::spawn(move || serve(c, ctx));
            }
        });
        Proxy { port, desync }
    }
    pub fn set_desync(&self, on: bool) {
        self.desync.store(on, Ordering::SeqCst);
    }
}

fn connect_out(ctx: &ProxyCtx, host: &str, port: u16) -> std::io::Result<TcpStream> {
    let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs()?.filter(|a| a.is_ipv4()).collect();
    let use_range = ctx.desync.load(Ordering::SeqCst) && scoped(host);
    let mut last = std::io::Error::new(std::io::ErrorKind::Other, "нет адресов");
    for addr in addrs {
        if !use_range {
            match TcpStream::connect_timeout(&addr, Duration::from_secs(10)) {
                Ok(s) => {
                    let _ = s.set_nodelay(true);
                    return Ok(s);
                }
                Err(e) => {
                    last = e;
                    continue;
                }
            }
        }
        for _ in 0..60 {
            let sp = ctx
                .next
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(if v >= PORT_HI { PORT_LO } else { v + 1 }))
                .unwrap_or(PORT_LO);
            let sock = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
            let _ = sock.set_nodelay(true);
            let _ = sock.set_reuse_address(true);
            if sock.bind(&SocketAddr::from(([0, 0, 0, 0], sp)).into()).is_err() {
                continue;
            }
            match sock.connect_timeout(&addr.into(), Duration::from_secs(10)) {
                Ok(()) => return Ok(sock.into()),
                Err(e) => {
                    last = e;
                    break;
                }
            }
        }
    }
    log(&format!("connect {host}:{port} failed: {last}"));
    Err(last)
}

fn pipe(a: TcpStream, b: TcpStream) {
    let (mut a2, mut b2) = match (a.try_clone(), b.try_clone()) {
        (Ok(x), Ok(y)) => (x, y),
        _ => return,
    };
    let (mut a1, mut b1) = (a, b);
    let t = std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        loop {
            match a1.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if b1.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = b1.shutdown(Shutdown::Write);
    });
    let mut buf = [0u8; 65536];
    loop {
        match b2.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if a2.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
    let _ = a2.shutdown(Shutdown::Write);
    let _ = t.join();
}

fn serve(mut c: TcpStream, ctx: Arc<ProxyCtx>) {
    let _ = c.set_nodelay(true);
    let _ = c.set_read_timeout(Some(Duration::from_secs(15)));
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let pos = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        match c.read(&mut tmp) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
        if buf.len() > 65536 {
            return;
        }
    };
    let head = String::from_utf8_lossy(&buf[..pos]).to_string();
    let rest = buf[pos + 4..].to_vec();
    let _ = c.set_read_timeout(None);
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut it = first.splitn(3, ' ');
    let (method, target, ver) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or("HTTP/1.1"));
    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = target.rsplit_once(':').unwrap_or((target, "443"));
        match connect_out(&ctx, host.trim_matches(['[', ']']), port.parse().unwrap_or(443)) {
            Ok(mut up) => {
                if c.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").is_err() {
                    return;
                }
                if !rest.is_empty() {
                    let _ = up.write_all(&rest);
                }
                pipe(c, up);
            }
            Err(_) => {
                let _ = c.write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n");
            }
        }
    } else {
        // обычный http: absolute-form
        let rest_url = target.strip_prefix("http://").unwrap_or(target);
        let (hostport, path) = match rest_url.find('/') {
            Some(i) => (&rest_url[..i], &rest_url[i..]),
            None => (rest_url, "/"),
        };
        let (host, port) = hostport.rsplit_once(':').map(|(h, p)| (h, p.parse().unwrap_or(80))).unwrap_or((hostport, 80));
        if let Ok(mut up) = connect_out(&ctx, host, port) {
            let hdrs: Vec<&str> = lines.filter(|l| {
                let low = l.to_lowercase();
                !(low.starts_with("proxy-connection") || low.starts_with("connection"))
            }).collect();
            let req = format!("{method} {path} {ver}\r\n{}\r\nConnection: close\r\n\r\n", hdrs.join("\r\n"));
            let _ = up.write_all(req.as_bytes());
            let _ = up.write_all(&rest);
            pipe(c, up);
        }
    }
}

fn probe(host: &str, proxy_port: u16) -> bool {
    let px = match ureq::Proxy::new(format!("http://127.0.0.1:{proxy_port}")) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let agent = ureq::AgentBuilder::new().proxy(px).timeout(Duration::from_millis(3500)).redirects(0).build();
    match agent.head(&format!("https://{host}/")).call() {
        Ok(_) => true,
        Err(ureq::Error::Status(_, _)) => true,
        Err(_) => false,
    }
}

fn score(proxy_port: u16) -> (usize, Vec<(String, bool)>) {
    let res = Arc::new(Mutex::new(vec![]));
    let mut ths = vec![];
    for h in PROBE_HOSTS {
        let res = res.clone();
        ths.push(std::thread::spawn(move || {
            let ok = probe(h, proxy_port);
            res.lock().unwrap().push((h.to_string(), ok));
        }));
    }
    for t in ths {
        let _ = t.join();
    }
    let r = res.lock().unwrap().clone();
    (r.iter().filter(|(_, ok)| *ok).count(), r)
}

// ---------------------------------------------------------------- помощник с правами администратора
fn parent_alive(pid: u32) -> bool {
    unsafe {
        match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            Ok(h) => {
                let r = WaitForSingleObject(h, 0);
                let _ = CloseHandle(h);
                r.0 == 0x102 // WAIT_TIMEOUT: процесс ещё идёт
            }
            Err(_) => false,
        }
    }
}

fn read_json(p: &PathBuf) -> Value {
    std::fs::read_to_string(p).ok().and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok()).unwrap_or(json!({}))
}

/// Выполняется с правами администратора: запускает/перезапускает winws по командам приложения и следит за родителем.
pub fn helper_main(mut parent_pid: u32, dir: String) {
    let dir = PathBuf::from(dir);
    let (cmdf, statf, filt, parentf) = (dir.join("cmd.json"), dir.join("status.json"), dir.join("filter.txt"), dir.join("parent.txt"));
    let bindir = dir.join("bin");
    let mut child: Option<std::process::Child> = None;
    let mut cur: Option<usize> = None;
    let mut err = String::new();
    let mut last: Option<i64> = Some(-2);
    let mut dead_since: Option<Instant> = None;

    let write_status = |cur: Option<usize>, err: &str, running: bool, done: bool| {
        let v = json!({"ts": now_f(), "strategy": cur, "error": err, "running": running, "done": done});
        let tmp = statf.with_extension("tmp");
        if std::fs::write(&tmp, v.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, &statf);
        }
    };
    let stop = |child: &mut Option<std::process::Child>| {
        if let Some(mut c) = child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    };

    loop {
        if let Ok(s) = std::fs::read_to_string(&parentf) {
            if let Ok(p) = s.trim().parse::<u32>() {
                parent_pid = p;
            }
        }
        if parent_alive(parent_pid) {
            dead_since = None;
        } else {
            let t = *dead_since.get_or_insert_with(Instant::now);
            if t.elapsed() > Duration::from_secs(30) {
                break; // приложение закрыто и не вернулось
            }
        }
        let cmd = read_json(&cmdf);
        if cmd["quit"].as_bool().unwrap_or(false) {
            break;
        }
        let want: Option<i64> = cmd["strategy"].as_i64();
        let want_key = want.or(Some(-1));
        if want_key != last {
            last = want_key;
            err.clear();
            match want {
                None => {
                    stop(&mut child);
                    cur = None;
                }
                Some(i) => {
                    stop(&mut child);
                    let (_, args) = STRATEGIES[(i as usize).min(STRATEGIES.len() - 1)];
                    let mut c = std::process::Command::new(bindir.join("winws.exe"));
                    c.arg(format!("--wf-raw=@{}", filt.display())).args(args.split_whitespace()).current_dir(&bindir).creation_flags(0x08000000);
                    match c.spawn() {
                        Ok(ch) => {
                            child = Some(ch);
                            cur = Some(i as usize);
                        }
                        Err(e) => {
                            cur = None;
                            err = e.to_string();
                        }
                    }
                }
            }
        }
        if let Some(c) = child.as_mut() {
            if let Ok(Some(_)) = c.try_wait() {
                child = None;
            }
        }
        write_status(cur, &err, child.is_some(), false);
        std::thread::sleep(Duration::from_millis(500));
    }
    stop(&mut child);
    write_status(cur, &err, false, true);
}

fn now_f() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

// ---------------------------------------------------------------- управление из приложения
#[derive(Default)]
struct St {
    state: String,
    msg: String,
    strategy: Option<usize>,
    busy: bool,
    want: bool,
    in_browser: bool,
}

static ST: OnceLock<Mutex<St>> = OnceLock::new();
static PROXY: OnceLock<Proxy> = OnceLock::new();

fn st() -> std::sync::MutexGuard<'static, St> {
    ST.get_or_init(|| Mutex::new(St { state: "off".into(), ..Default::default() })).lock().unwrap_or_else(|e| e.into_inner())
}

fn set_state(state: &str, msg: &str) {
    let mut s = st();
    s.state = state.to_string();
    s.msg = msg.to_string();
}

fn set_msg(msg: &str) {
    st().msg = msg.to_string();
}

fn proxy() -> &'static Proxy {
    PROXY.get_or_init(Proxy::start)
}

fn cmd(v: Value) {
    let _ = std::fs::create_dir_all(bypass_dir());
    let _ = std::fs::write(bypass_dir().join("cmd.json"), v.to_string());
}

fn helper_status() -> Value {
    read_json(&bypass_dir().join("status.json"))
}

fn helper_alive() -> bool {
    let s = helper_status();
    s.is_object() && s.get("ts").is_some() && !s["done"].as_bool().unwrap_or(false) && now_f() - s["ts"].as_f64().unwrap_or(0.0) < 4.0
}

fn resource_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("third_party").join("zapret"))).unwrap_or_default()
}

fn prepare_files() -> Result<(), String> {
    let bin = bypass_dir().join("bin");
    std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    for n in ["winws.exe", "cygwin1.dll", "WinDivert.dll", "WinDivert64.sys"] {
        let (src, dst) = (resource_dir().join(n), bin.join(n));
        let need = match (std::fs::metadata(&src), std::fs::metadata(&dst)) {
            (Ok(a), Ok(b)) => a.len() != b.len(),
            _ => true,
        };
        if need {
            std::fs::copy(&src, &dst).map_err(|e| format!("{n}: {e}"))?;
        }
    }
    std::fs::write(bypass_dir().join("filter.txt"), win_filter()).map_err(|e| e.to_string())?;
    for n in ["cmd.json", "status.json"] {
        let _ = std::fs::remove_file(bypass_dir().join(n));
    }
    Ok(())
}

fn write_parent() {
    let _ = std::fs::create_dir_all(bypass_dir());
    let _ = std::fs::write(bypass_dir().join("parent.txt"), std::process::id().to_string());
}

/// Флаги Chromium для окон браузера: весь трафик идёт через наш прокси, QUIC выключен. Вызывается до создания окон.
pub fn browser_args() -> String {
    st().in_browser = true;
    let p = proxy().port;
    net::set_proxy(Some(format!("http://127.0.0.1:{p}")));
    format!("--proxy-server=http://127.0.0.1:{p} --disable-quic")
}

pub fn launch() {
    write_parent();
    {
        let mut s = st();
        s.want = true;
        if s.busy {
            return;
        }
        s.busy = true;
        s.state = "starting".into();
        s.msg = "Запуск".into();
    }
    std::thread::spawn(|| {
        run();
        let mut s = st();
        s.busy = false;
        let want = s.want;
        drop(s);
        if !want {
            off();
        }
    });
}

fn off() {
    if let Some(p) = PROXY.get() {
        p.set_desync(false);
    }
    if helper_alive() {
        cmd(json!({"strategy": null}));
    }
    let mut s = st();
    s.state = "off".into();
    s.msg.clear();
    s.strategy = None;
}

pub fn disable() {
    let busy = {
        let mut s = st();
        s.want = false;
        s.busy
    };
    if !busy {
        off();
    }
}

fn run() {
    let port = proxy().port;
    if helper_alive() {
        cmd(json!({"strategy": null}));
        std::thread::sleep(Duration::from_millis(500));
    } else {
        set_msg("Жду подтверждения прав администратора (UAC)…");
        if let Err(e) = prepare_files() {
            set_state("failed", &format!("Не найдены файлы обхода: {e}"));
            return;
        }
        let exe = std::env::current_exe().unwrap_or_default();
        let params = format!("{HELPER_FLAG} {} \"{}\"", std::process::id(), bypass_dir().display());
        let rc = unsafe {
            let w = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
            let (op, file, pr) = (w("runas"), w(&exe.to_string_lossy()), w(&params));
            ShellExecuteW(None, PCWSTR(op.as_ptr()), PCWSTR(file.as_ptr()), PCWSTR(pr.as_ptr()), PCWSTR::null(), SW_HIDE).0 as isize
        };
        log(&format!("runas rc={rc} exe={} params={params}", exe.display()));
        if rc <= 32 {
            set_state("denied", "Права администратора не выданы, обход выключен");
            return;
        }
        set_msg("Права подтверждены, запускаю помощника…");
        let t0 = Instant::now();
        while !helper_alive() {
            if t0.elapsed() > Duration::from_secs(40) {
                set_state("failed", "Помощник не запустился");
                return;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }
    set_msg("Проверяю доступность сайтов…");
    let (base, det) = score(port);
    log(&format!("baseline {base}/{} {det:?}", PROBE_HOSTS.len()));
    if !st().want {
        return;
    }
    if base == PROBE_HOSTS.len() {
        let mut s = st();
        s.state = "unneeded".into();
        s.msg = "Сайты и так доступны, обход не нужен".into();
        s.strategy = None;
        return;
    }
    select(base);
}

fn select(base: usize) {
    set_state("testing", "Подбор стратегии");
    let saved = data::ui("bypass_strategy").as_u64().map(|v| v as usize);
    let mut order: Vec<usize> = (0..STRATEGIES.len()).collect();
    if let Some(s) = saved.filter(|s| *s < order.len()) {
        order.retain(|x| *x != s);
        order.insert(0, s);
    }
    let (mut best, mut best_i) = (base, None);
    for i in order {
        if !st().want {
            return;
        }
        set_msg(&format!("Проверка: {}", STRATEGIES[i].0));
        cmd(json!({"strategy": i}));
        proxy().set_desync(true);
        std::thread::sleep(Duration::from_millis(800));
        let stt = helper_status();
        if !stt["running"].as_bool().unwrap_or(false) {
            log(&format!("strategy {i} not running: {stt}"));
            continue;
        }
        let (sc, det) = score(proxy().port);
        log(&format!("strategy {i} ({}): {sc}/{} {det:?}", STRATEGIES[i].0, PROBE_HOSTS.len()));
        if sc > best {
            best = sc;
            best_i = Some(i);
        }
        if sc == PROBE_HOSTS.len() {
            break;
        }
    }
    match best_i {
        None => {
            cmd(json!({"strategy": null}));
            proxy().set_desync(false);
            let mut s = st();
            s.state = "failed".into();
            s.msg = "Ни одна стратегия не помогла у вашего провайдера".into();
            s.strategy = None;
        }
        Some(i) => {
            cmd(json!({"strategy": i}));
            proxy().set_desync(true);
            data::update(|d| d["settings"]["ui"]["bypass_strategy"] = json!(i));
            let mut s = st();
            s.strategy = Some(i);
            s.state = "on".into();
            s.msg = format!("Работает: {} ({} из {} сайтов)", STRATEGIES[i].0, best, PROBE_HOSTS.len());
        }
    }
}

pub fn retest() {
    if st().busy {
        return;
    }
    data::update(|d| d["settings"]["ui"]["bypass_strategy"] = Value::Null);
    launch();
}

pub fn info() -> Value {
    let on = data::ui("bypass").as_bool().unwrap_or(false);
    let s = st();
    json!({"supported": true, "state": s.state, "msg": s.msg, "on": on,
           "strategy": s.strategy.map(|i| STRATEGIES[i].0).unwrap_or("")})
}

/// Переключатель в настройках: без перезапуска, если окна уже запущены с прокси; иначе один раз перезапускаемся.
pub fn set(on: bool) -> Value {
    api::update_ui("bypass", json!(on));
    if on {
        launch();
        api::open_splash();
        let in_browser = st().in_browser;
        if !in_browser {
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_secs(1));
                while st().busy {
                    std::thread::sleep(Duration::from_millis(300));
                }
                let state = st().state.clone();
                if state == "on" || state == "unneeded" {
                    std::thread::sleep(Duration::from_millis(2700)); // заставка успевает показать итог
                    restart_app();
                }
            });
        }
    } else {
        disable();
    }
    json!(true)
}

pub fn stop(keep_helper: bool) {
    if !keep_helper {
        cmd(json!({"quit": true}));
    }
}

pub fn restart_app() {
    stop(true);
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).env("MP_RESTART", "1").spawn();
    }
    std::process::exit(0);
}
