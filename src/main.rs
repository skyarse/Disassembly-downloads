mod bridge;
mod core;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl};
fn main() {
    if std::env::var_os("APPIMAGE").is_some() {
        std::env::set_var("QT_QUICK_CONTROLS_STYLE", "Fusion");
    }
    let mut app = QGuiApplication::new();
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
