use anyhow::{ensure, Context, Result};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

const MARKER: &str = "X-Downloads-Triage-Managed=true";

pub fn enabled(directory: &Path) -> bool {
    fs::read_to_string(directory.join("net.skyarse.DownloadsTriage.desktop"))
        .is_ok_and(|text| text.lines().any(|line| line == MARKER))
}

fn executable_argument(path: &Path) -> Result<String> {
    let value = path
        .to_str()
        .context("Путь приложения должен быть в UTF-8")?;
    ensure!(
        !value.contains(['\n', '\r']),
        "Недопустимый путь приложения"
    );
    let mut quoted = String::from("\"");
    for character in value.chars() {
        match character {
            '%' => quoted.push_str("%%"),
            '\\' => quoted.push_str("\\\\\\\\"),
            '"' | '$' | '`' => {
                quoted.push_str("\\\\");
                quoted.push(character);
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    Ok(quoted)
}

pub fn set(directory: &Path, executable: &Path, enable: bool) -> Result<()> {
    fs::create_dir_all(directory)?;
    let target = directory.join("net.skyarse.DownloadsTriage.desktop");
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        ensure!(
            !metadata.file_type().is_symlink() && metadata.is_file(),
            "Запись автозапуска содержит ссылку или не является файлом"
        );
        ensure!(
            enabled(directory),
            "Существующая запись автозапуска не создана этим приложением"
        );
    }
    if !enable {
        if target.exists() {
            fs::remove_file(target)?;
            fs::File::open(directory)?.sync_all()?;
        }
        return Ok(());
    }
    ensure!(
        executable.is_absolute() && executable.is_file(),
        "Не найден исполняемый файл приложения"
    );
    let text = format!("[Desktop Entry]\nType=Application\nName=Downloads Triage\nName[ru]=Загрузки на разбор\nExec=/usr/bin/env -- {} --background\nIcon=net.skyarse.DownloadsTriage\nTerminal=false\n{MARKER}\n", executable_argument(executable)?);
    let temporary = directory.join(".downloads-triage.desktop.new");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    fs::rename(temporary, target)?;
    fs::File::open(directory)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn writes_owned_entry_and_preserves_unrelated_entries() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("Downloads Triage%.AppImage");
        fs::write(&executable, b"fixture").unwrap();
        let directory = temp.path().join("autostart");
        set(&directory, &executable, true).unwrap();
        assert!(enabled(&directory));
        let entry =
            fs::read_to_string(directory.join("net.skyarse.DownloadsTriage.desktop")).unwrap();
        assert!(entry.contains("Downloads Triage%%.AppImage\" --background"));
        set(&directory, &executable, false).unwrap();
        assert!(!enabled(&directory));
        fs::write(
            directory.join("net.skyarse.DownloadsTriage.desktop"),
            b"user entry",
        )
        .unwrap();
        assert!(set(&directory, &executable, true).is_err());
        assert!(set(&directory, &executable, false).is_err());
    }
    #[test]
    fn escapes_desktop_exec_field_and_rejects_newlines() {
        let escaped = executable_argument(Path::new("/home/u/a \\\"$`%.AppImage")).unwrap();
        assert!(escaped.contains("%%"));
        assert!(escaped.starts_with('"') && escaped.ends_with('"'));
        assert!(executable_argument(Path::new("/tmp/bad\nfile")).is_err());
    }
}
