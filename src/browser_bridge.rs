use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const HOST: &str = "net.skyarse.downloads_triage";
const FIREFOX_ID: &str = "download-origins@downloads-triage.skyarse.net";
const CHROMIUM_ID: &str = "bkleepapemahlodlkpmndjnlgjcjkcke";
const WRAPPER_MARKER: &str = "# downloads-triage-browser-bridge";
use crate::core::{download_origin, reverse_download};

const MAX_MESSAGE_SIZE: usize = 256 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DownloadMessage {
    path: PathBuf,
    page: String,
    file: String,
    size: Option<u64>,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Query {
    ListDownloads {
        page: String,
        #[serde(default)]
        candidates: Vec<reverse_download::Candidate>,
    },
    ShowDownload {
        page: String,
        id: i64,
    },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Request {
    Query(Query),
    Download(DownloadMessage),
}

pub fn extension_directory() -> Result<PathBuf> {
    Ok(data_directory()?.join("browser-extensions"))
}

fn data_directory() -> Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .context("Не найдена папка данных")?
        .join("downloads-triage"))
}

fn registrations(home: &Path, config: &Path, wrapper: &Path) -> [(PathBuf, Value); 3] {
    let manifest = |permission: Value| {
        let mut document = json!({
            "name": HOST,
            "description": "Downloads Triage",
            "path": wrapper,
            "type": "stdio",
        });
        document
            .as_object_mut()
            .unwrap()
            .extend(permission.as_object().unwrap().clone());
        document
    };
    let native_name = format!("{HOST}.json");
    let chromium =
        manifest(json!({"allowed_origins": [format!("chrome-extension://{CHROMIUM_ID}/")]}));
    [
        (
            home.join(".mozilla/native-messaging-hosts")
                .join(&native_name),
            manifest(json!({"allowed_extensions": [FIREFOX_ID]})),
        ),
        (
            config
                .join("chromium/NativeMessagingHosts")
                .join(&native_name),
            chromium.clone(),
        ),
        (
            config
                .join("google-chrome/NativeMessagingHosts")
                .join(&native_name),
            chromium,
        ),
    ]
}

pub fn install(executable: &Path) -> Result<()> {
    let home = dirs::home_dir().context("Не найдена домашняя папка")?;
    let config = dirs::config_dir().context("Не найдена папка настроек")?;
    install_in(&home, &config, &data_directory()?, executable)
}

fn install_in(home: &Path, config: &Path, data: &Path, executable: &Path) -> Result<()> {
    let wrapper = data.join("browser-bridge");
    let executable = executable
        .to_str()
        .context("Путь приложения должен быть в UTF-8")?;
    ensure!(
        !executable.contains(['\n', '\r']),
        "Недопустимый путь приложения"
    );
    let quote = executable.replace('\'', "'\"'\"'");
    let script = format!("#!/bin/sh\n{WRAPPER_MARKER}\nexec '{quote}' --record-download\n");
    let manifests = registrations(home, config, &wrapper);
    for (path, manifest) in &manifests {
        if path_exists(path)? {
            ensure!(
                fs::symlink_metadata(path)?.is_file()
                    && serde_json::from_slice::<Value>(&fs::read(path)?)
                        .is_ok_and(|value| value == *manifest),
                "Файл для связи с браузером уже занят"
            );
        }
    }
    if path_exists(&wrapper)? {
        ensure!(
            fs::symlink_metadata(&wrapper)?.is_file()
                && fs::read_to_string(&wrapper)?
                    .lines()
                    .any(|line| line == WRAPPER_MARKER),
            "Файл для запуска приложения из браузера уже занят"
        );
    }
    let extensions = data.join("browser-extensions");
    let background = include_bytes!("../browser-extension/background.js");
    for (browser, manifest) in [
        (
            "firefox",
            include_bytes!("../browser-extension/firefox.json").as_slice(),
        ),
        (
            "chromium",
            include_bytes!("../browser-extension/chromium.json").as_slice(),
        ),
    ] {
        let directory = extensions.join(browser);
        write_atomic(&directory.join("manifest.json"), manifest, 0o600)?;
        write_atomic(&directory.join("background.js"), background, 0o600)?;
    }
    for (name, content) in [
        (
            "popup.html",
            include_bytes!("../browser-extension/popup.html").as_slice(),
        ),
        (
            "popup.js",
            include_bytes!("../browser-extension/popup.js").as_slice(),
        ),
        (
            "popup.css",
            include_bytes!("../browser-extension/popup.css").as_slice(),
        ),
    ] {
        write_atomic(&extensions.join("firefox").join(name), content, 0o600)?;
    }
    write_atomic(&wrapper, script.as_bytes(), 0o700)?;
    for (path, manifest) in manifests {
        write_atomic(&path, &serde_json::to_vec_pretty(&manifest)?, 0o600)?;
    }
    Ok(())
}

pub(crate) fn path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let parent = path.parent().context("Не найдена папка назначения")?;
    fs::create_dir_all(parent)?;
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temporary = path.with_extension(format!("{}.{}.new", std::process::id(), timestamp));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temporary)?;
    let result = (|| -> Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn read_message(mut input: impl Read) -> Result<Request> {
    let mut size = [0; 4];
    input.read_exact(&mut size)?;
    let size = u32::from_ne_bytes(size) as usize;
    ensure!(
        size > 0 && size <= MAX_MESSAGE_SIZE,
        "Недопустимый размер сообщения браузера"
    );
    let mut payload = vec![0; size];
    input.read_exact(&mut payload)?;
    Ok(serde_json::from_slice(&payload)?)
}

pub fn serve(input: impl Read, mut output: impl Write) -> Result<()> {
    let response = match read_message(input).and_then(handle) {
        Ok(reply) => reply,
        Err(error) => json!({"ok": false, "message": error.to_string()}),
    };
    let payload = serde_json::to_vec(&response)?;
    output.write_all(&(payload.len() as u32).to_ne_bytes())?;
    output.write_all(&payload)?;
    output.flush()?;
    Ok(())
}

fn handle(request: Request) -> Result<Value> {
    match request {
        Request::Download(download) => {
            download_origin::record(
                &download.path,
                &download.page,
                &download.file,
                download.size,
            )?;
            Ok(json!({"ok": true}))
        }
        Request::Query(Query::ListDownloads { page, candidates }) => {
            let mut result = serde_json::to_value(reverse_download::list(&page, &candidates)?)?;
            result["ok"] = json!(true);
            Ok(result)
        }
        Request::Query(Query::ShowDownload { page, id }) => {
            Ok(json!({"ok": true, "available": reverse_download::show(&page, id)?}))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn browser_messages_are_framed_and_bounded() {
        let json = br#"{"path":"/mnt/drive/f.zip","page":"https://test.example/page","file":"https://test.example/f.zip","size":42}"#;
        let mut frame = (json.len() as u32).to_ne_bytes().to_vec();
        frame.extend_from_slice(json);
        let Request::Download(message) = read_message(frame.as_slice()).unwrap() else {
            panic!("expected a download");
        };
        assert_eq!(message.path, Path::new("/mnt/drive/f.zip"));
        assert_eq!(message.size, Some(42));
        assert!(read_message((MAX_MESSAGE_SIZE as u32 + 1).to_ne_bytes().as_slice()).is_err());
        assert!(read_message(&frame[..frame.len() - 1]).is_err());
    }

    #[test]
    fn installation_keeps_the_browser_ids_and_rejects_foreign_manifests() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let config = temp.path().join("config");
        let data = temp.path().join("data");
        let app = temp.path().join("Downloads Triage's AppImage");
        install_in(&home, &config, &data, &app).unwrap();
        install_in(&home, &config, &data, &app).unwrap();
        let wrapper = data.join("browser-bridge");
        let wrapper_text = fs::read_to_string(&wrapper).unwrap();
        assert!(wrapper_text.contains("Triage'\"'\"'s AppImage' --record-download"));
        assert_eq!(
            fs::metadata(&wrapper).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let [(firefox, _), (chromium, _), _] = registrations(&home, &config, &wrapper);
        let text = fs::read_to_string(&firefox).unwrap();
        assert!(text.contains(FIREFOX_ID));
        assert!(fs::read_to_string(&chromium).unwrap().contains(CHROMIUM_ID));
        assert!(data
            .join("browser-extensions/firefox/manifest.json")
            .is_file());
        fs::write(firefox, "{}").unwrap();
        assert!(install_in(&home, &config, &data, &app).is_err());
    }
}
