#![windows_subsystem = "windows"]
mod accounts;
mod api;
mod app;
mod bypass;
mod data;
mod geometry;
mod net;
mod panels;
mod rawcookie;
mod server;
mod shim;
mod state;
mod update;
mod winapi;

fn main() {
    // при аварии пишем причину в журнал (приложение собрано с panic=abort и иначе закрылось бы молча)
    std::panic::set_hook(Box::new(|info| {
        let _ = std::fs::create_dir_all(data::data_dir());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(data::data_dir().join("crash.log")) {
            use std::io::Write;
            let _ = writeln!(f, "{} {}", data::now(), info);
        }
    }));
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == bypass::HELPER_FLAG) {
        // копия с правами администратора: управляет winws
        let pid = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(0);
        bypass::helper_main(pid, args.get(i + 2).cloned().unwrap_or_default());
        return;
    }
    // одна копия приложения: порт-замок; после перезапуска ждём, пока старая копия закроется
    let tries = if std::env::var_os("MP_RESTART").is_some() { 40 } else { 1 };
    let mut lock = None;
    for t in 0..tries {
        match std::net::TcpListener::bind("127.0.0.1:47653") {
            Ok(l) => {
                lock = Some(l);
                break;
            }
            Err(_) if t + 1 < tries => std::thread::sleep(std::time::Duration::from_millis(250)),
            Err(_) => {}
        }
    }
    let Some(_lock) = lock else { return };

    let _ = state::PORT.set(server::start());
    let _ = std::fs::create_dir_all(data::data_dir().join("web"));
    state::app().autohide = data::ui("autohide_bar").as_bool().unwrap_or(true);

    if data::ui("bypass").as_bool().unwrap_or(false) {
        // без включённого обхода прокси нет: сеть работает как обычно
        app::set_extra_args(&bypass::browser_args());
        std::thread::spawn(|| {
            // запускаем уже после появления окна: запрос прав и проверки не задерживают его открытие
            std::thread::sleep(std::time::Duration::from_millis(1500));
            bypass::launch();
            api::open_splash();
        });
    }
    update::start_background();
    app::run();
    bypass::stop(false);
}
