use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use std::{pin::Pin, sync::mpsc};

#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
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
    }
}

pub struct BackendRust {
    response: QString,
    sender: mpsc::SyncSender<String>,
    receiver: mpsc::Receiver<String>,
}
impl Default for BackendRust {
    fn default() -> Self {
        let (sender, commands) = mpsc::sync_channel::<String>(8);
        let (results, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut engine = crate::core::Engine::open_default();
            while let Ok(command) = commands.recv() {
                let response = match &mut engine {
                    Ok(engine) => engine.command(&command),
                    Err(error) => serde_json::json!({"error": format!("{error:#}")}).to_string(),
                };
                if results.send(response).is_err() {
                    break;
                }
            }
        });
        Self {
            response: QString::default(),
            sender,
            receiver,
        }
    }
}
impl ffi::Backend {
    pub fn request(mut self: Pin<&mut Self>, command: &QString) {
        if self.rust().sender.try_send(command.to_string()).is_err() {
            self.as_mut().set_response(QString::from(
                "{\"error\":\"Очередь операций занята. Повторите действие.\"}",
            ));
        }
    }
    pub fn poll(mut self: Pin<&mut Self>) {
        let mut latest = None;
        while let Ok(value) = self.rust().receiver.try_recv() {
            latest = Some(value);
        }
        if let Some(value) = latest {
            self.as_mut().set_response(QString::from(value.as_str()));
        }
    }
}
