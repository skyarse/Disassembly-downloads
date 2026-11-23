mod autostart;
mod bridge;
mod browser_bridge;
mod core;
mod window_link;

use cxx_qt_lib::{QQmlApplicationEngine, QUrl};
fn main() {
    let mut args = std::env::args_os().skip(1);
    if let Some(command) = args.next() {
        if command == "--record-download" {
            if browser_bridge::serve(std::io::stdin().lock(), std::io::stdout().lock()).is_err() {
                std::process::exit(1);
            }
            return;
        }
        if let Some(as_page) = match command.to_str() {
            Some("--open-download-page") => Some(true),
            Some("--open-download-file-url") => Some(false),
            _ => None,
        } {
            let result = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("Выберите файл"))
                .and_then(|path| core::download_origin::open(std::path::Path::new(&path), as_page));
            if let Err(error) = result {
                let message = error.to_string();
                eprintln!("{message}");
                let status = std::process::Command::new("/usr/bin/kdialog")
                    .arg("--sorry")
                    .arg(&message)
                    .status();
                if !status.is_ok_and(|status| status.success()) {
                    let _ = std::process::Command::new("/usr/bin/notify-send")
                        .arg("Загрузки на разбор")
                        .arg(message)
                        .status();
                }
                std::process::exit(1);
            }
            return;
        }
    }
    if std::env::var_os("APPIMAGE").is_some() {
        std::env::set_var("QT_QUICK_CONTROLS_STYLE", "Basic");
    }
    let version = option_env!("DOWNLOADS_TRIAGE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
    let mut app = bridge::ffi::create_application(std::env::args().collect(), version);
    let mut engine = QQmlApplicationEngine::new();
    let Some(engine) = engine.as_mut() else {
        std::process::exit(1)
    };
    let entry = if std::env::args().any(|arg| arg == "--queue-window") {
        "qrc:/qt/qml/net/skyarse/DownloadsTriage/qml/Main.qml"
    } else {
        "qrc:/qt/qml/net/skyarse/DownloadsTriage/qml/TrayHost.qml"
    };
    engine.load(&QUrl::from(entry));
    if let Some(app) = app.as_mut() {
        app.exec();
    }
}
