mod autostart;
mod bridge;
mod core;

use cxx_qt_lib::{QQmlApplicationEngine, QUrl};
fn main() {
    if std::env::var_os("APPIMAGE").is_some() {
        std::env::set_var("QT_QUICK_CONTROLS_STYLE", "Basic");
    }
    let mut app = bridge::ffi::create_application(std::env::args().collect());
    let mut engine = QQmlApplicationEngine::new();
    let Some(engine) = engine.as_mut() else {
        std::process::exit(1)
    };
    engine.load(&QUrl::from(
        "qrc:/qt/qml/net/skyarse/DownloadsTriage/qml/Main.qml",
    ));
    if let Some(app) = app.as_mut() {
        app.exec();
    }
}
