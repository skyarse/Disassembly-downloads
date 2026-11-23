use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use std::{
    io::{BufRead, Write},
    pin::Pin,
    sync::mpsc,
};

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
        type WindowLink = super::WindowLinkRust;
        #[qinvokable]
        fn request(self: &Self, command: &QString);
        #[qinvokable]
        fn poll(self: Pin<&mut Self>);
        #[qsignal]
        fn received(self: Pin<&mut Self>);
    }
}

pub struct WindowLinkRust {
    response: QString,
    messages: mpsc::Receiver<String>,
}

impl Default for WindowLinkRust {
    fn default() -> Self {
        let (sender, messages) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    return;
                }
            }
            let _ = sender.send(r#"{"window_action":"quit"}"#.into());
        });
        Self {
            response: QString::default(),
            messages,
        }
    }
}

impl ffi::WindowLink {
    pub fn request(&self, command: &QString) {
        let _ = writeln!(std::io::stdout().lock(), "{}", command.to_string());
    }

    pub fn poll(mut self: Pin<&mut Self>) {
        while let Ok(value) = self.rust().messages.try_recv() {
            self.as_mut().set_response(QString::from(value.as_str()));
            self.as_mut().received();
        }
    }
}
