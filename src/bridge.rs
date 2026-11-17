use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use std::{
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("application.h");
        type QGuiApplication = cxx_qt_lib::QGuiApplication;
        fn create_application(arguments: Vec<String>) -> UniquePtr<QGuiApplication>;
    }
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, response)]
        type Backend = super::BackendRust;
        #[qinvokable]
        fn request(self: Pin<&mut Self>, command: &QString);
        #[qinvokable]
        fn poll(self: Pin<&mut Self>);
        #[qsignal]
        fn finished(self: Pin<&mut Self>);
    }
}

pub struct BackendRust {
    response: QString,
    sender: mpsc::SyncSender<String>,
    receiver: mpsc::Receiver<String>,
    pause_guard: Arc<AtomicBool>,
}
impl Default for BackendRust {
    fn default() -> Self {
        let (sender, commands) = mpsc::sync_channel::<String>(8);
        let (results, receiver) = mpsc::channel();
        let pause_guard = Arc::new(AtomicBool::new(false));
        let worker_guard = pause_guard.clone();
        std::thread::spawn(move || {
            let mut engine = crate::core::Engine::open_default();
            if let Ok(engine) = &mut engine {
                engine.set_pause_guard(worker_guard);
            }
            let interval = Duration::from_secs(15);
            let mut next_tick = Instant::now() + interval;
            loop {
                let command = match commands
                    .recv_timeout(next_tick.saturating_duration_since(Instant::now()))
                {
                    Ok(command) => command,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        next_tick = Instant::now() + interval;
                        if engine.is_err() {
                            continue;
                        }
                        "{\"op\":\"tick\"}".to_string()
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let operation =
                    serde_json::from_str::<serde_json::Value>(&command).unwrap_or_default();
                let response = match &mut engine {
                    Ok(engine) => engine.command(&command),
                    Err(error) => {
                        let request =
                            serde_json::from_str::<serde_json::Value>(&command).unwrap_or_default();
                        serde_json::json!({"error": format!("{error:#}"), "request_id":request["request_id"], "operation":request["op"]}).to_string()
                    }
                };
                if results.send(response).is_err() {
                    break;
                }
                if matches!(operation["op"].as_str(), Some("tick" | "scan" | "init")) {
                    next_tick = Instant::now() + interval;
                }
            }
        });
        Self {
            response: QString::default(),
            sender,
            receiver,
            pause_guard,
        }
    }
}
impl ffi::Backend {
    pub fn request(mut self: Pin<&mut Self>, command: &QString) {
        let request =
            serde_json::from_str::<serde_json::Value>(&command.to_string()).unwrap_or_default();
        if request["op"] == "set_mode" && request["mode"] != "automatic" {
            self.rust().pause_guard.store(true, Ordering::Release);
        }
        if self.rust().sender.try_send(command.to_string()).is_err() {
            let reply = serde_json::json!({"error":"Очередь операций занята. Повторите действие.", "request_id":request["request_id"], "operation":request["op"]}).to_string();
            self.as_mut().set_response(QString::from(reply.as_str()));
            self.as_mut().finished();
        }
    }
    pub fn poll(mut self: Pin<&mut Self>) {
        loop {
            let Ok(value) = self.rust().receiver.try_recv() else {
                break;
            };
            self.as_mut().set_response(QString::from(value.as_str()));
            self.as_mut().finished();
        }
    }
}
