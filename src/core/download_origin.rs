use super::{bytes, Snapshot, Stamp};
use anyhow::{bail, ensure, Context, Result};
use cxx_qt_lib::QUrl;
use rusqlite::{params, Connection};
use std::{
    fs::{self, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const MENU_NAME: &str = "net.skyarse.DownloadsTriage.DownloadOrigin.desktop";
const MENU_MARKER: &str = "X-Downloads-Triage-DownloadOrigin=true";

#[derive(Debug, Eq, PartialEq)]
struct Links {
    page: String,
    file: String,
}

pub(super) fn web_url(value: &str) -> String {
    let url = QUrl::from(value);
    if !value.chars().any(char::is_control)
        && matches!(
            url.scheme().as_ref().map(ToString::to_string).as_deref(),
            Some("http" | "https")
        )
        && url.is_valid()
        && !url.host_or_default().to_string().is_empty()
        && url.user_info_or_default().to_string().is_empty()
    {
        value.to_owned()
    } else {
        String::new()
    }
}

fn links(page: &str, file: &str) -> Links {
    let file = web_url(file);
    let mut page = web_url(page);
    if page == file {
        page.clear();
    }
    Links { page, file }
}

pub(super) fn database() -> Result<Connection> {
    let directory = dirs::data_local_dir()
        .context("Не найдена папка данных")?
        .join("downloads-triage");
    fs::create_dir_all(&directory)?;
    let path = directory.join("download-origins.sqlite3");
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)?;
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(2))?;
    create_tables(&db)?;
    Ok(db)
}

pub(super) fn create_tables(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS download_origins (
        device TEXT NOT NULL, inode TEXT NOT NULL, size INTEGER NOT NULL,
        mtime INTEGER NOT NULL, ns INTEGER NOT NULL, ctime INTEGER NOT NULL, cns INTEGER NOT NULL,
        hash TEXT NOT NULL, page TEXT NOT NULL, url TEXT NOT NULL,
        PRIMARY KEY(device,inode,size,mtime,ns,ctime,cns,hash,page,url));
        CREATE INDEX IF NOT EXISTS download_origin_size_hash ON download_origins(size,hash);
        CREATE INDEX IF NOT EXISTS download_origin_page ON download_origins(page);
        CREATE TABLE IF NOT EXISTS download_locations (
            origin INTEGER PRIMARY KEY, path BLOB NOT NULL);",
    )
}

fn save(db: &Connection, snapshot: &Snapshot, links: &Links) -> Result<i64> {
    let s = &snapshot.stamp;
    Ok(db.query_row(
        "INSERT INTO download_origins VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
        ON CONFLICT DO UPDATE SET hash=excluded.hash RETURNING rowid",
        params![
            s.dev.to_string(),
            s.ino.to_string(),
            s.size,
            s.mtime,
            s.mtime_ns,
            s.ctime,
            s.ctime_ns,
            snapshot.hash,
            links.page,
            links.file
        ],
        |row| row.get(0),
    )?)
}

fn matching_current_stamp(db: &Connection, stamp: &Stamp) -> Result<Vec<Links>> {
    let mut statement = db.prepare(
        "SELECT DISTINCT page,url FROM download_origins WHERE device=?1 AND inode=?2 AND size=?3
        AND mtime=?4 AND ns=?5 AND ctime=?6 AND cns=?7",
    )?;
    let rows = statement.query_map(
        params![
            stamp.dev.to_string(),
            stamp.ino.to_string(),
            stamp.size,
            stamp.mtime,
            stamp.mtime_ns,
            stamp.ctime,
            stamp.ctime_ns
        ],
        |row| {
            Ok(Links {
                page: row.get(0)?,
                file: row.get(1)?,
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn saved(db: &Connection, snapshot: &Snapshot, exact: bool) -> Result<Vec<Links>> {
    let s = &snapshot.stamp;
    let mut statement = db.prepare(
        "SELECT DISTINCT page,url FROM download_origins WHERE size=?1 AND hash=?2 AND
        (NOT ?3 OR (device=?4 AND inode=?5 AND mtime=?6 AND ns=?7))",
    )?;
    let rows = statement.query_map(
        params![
            s.size,
            snapshot.hash,
            exact,
            s.dev.to_string(),
            s.ino.to_string(),
            s.mtime,
            s.mtime_ns
        ],
        |row| {
            Ok(Links {
                page: row.get(0)?,
                file: row.get(1)?,
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn record(path: &Path, page: &str, file: &str, size: Option<u64>) -> Result<()> {
    ensure!(
        enabled(),
        "Включите источники загрузок в настройках «Загрузок на разбор»."
    );
    ensure!(
        path.is_absolute(),
        "Браузер не передал абсолютный путь файла"
    );
    let path = path
        .canonicalize()
        .context("Скачанный файл уже перемещён или недоступен")?;
    let metadata = fs::symlink_metadata(&path)?;
    ensure!(
        metadata.is_file() && size.is_none_or(|size| metadata.len() == size),
        "Скачанный файл изменился или недоступен"
    );
    let links = links(page, file);
    ensure!(
        !links.page.is_empty() || !links.file.is_empty(),
        "Браузер не передал адрес загрузки"
    );
    let snapshot = Snapshot::read(&path)?;
    ensure!(
        snapshot.stamp == Stamp::of(&metadata) && !snapshot.directory,
        "Файл изменился при сохранении источника"
    );
    ensure!(enabled(), "Запоминание источников отключено");
    let mut db = database()?;
    let transaction = db.transaction()?;
    let origin = save(&transaction, &snapshot, &links)?;
    transaction.execute(
        "INSERT OR IGNORE INTO download_locations(origin,path) VALUES(?1,?2)",
        params![origin, bytes(&path)],
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn carry(source: &Snapshot, target: &Snapshot) -> Result<()> {
    if !source.directory && enabled() {
        let db = database()?;
        for links in saved(&db, source, true)? {
            save(&db, target, &links)?;
        }
    }
    Ok(())
}

fn lookup(db: &Connection, path: &Path, as_page: bool) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(metadata.is_file(), "Выберите обычный файл");
    let stamp = Stamp::of(&metadata);
    let mut known = matching_current_stamp(db, &stamp)?;
    if known.is_empty()
        && db.query_row(
            "SELECT EXISTS(SELECT 1 FROM download_origins WHERE size=?1)",
            [stamp.size],
            |row| row.get::<_, bool>(0),
        )?
    {
        let snapshot = Snapshot::read(path)?;
        known = saved(db, &snapshot, true)?;
        if let [links] = known.as_slice() {
            save(db, &snapshot, links)?;
        }
        if known.is_empty() {
            known = saved(db, &snapshot, false)?;
        }
    }
    let mut urls = known
        .into_iter()
        .map(|link| if as_page { link.page } else { link.file })
        .map(|url| web_url(&url))
        .collect::<Vec<_>>();
    urls.sort();
    urls.dedup();
    match urls.as_slice() {
        [url] if !url.is_empty() => Ok(url.clone()),
        [] | [_] if as_page => bail!("Страница загрузки для этого файла не найдена. Если сохранился прямой адрес, его можно открыть действием «Открыть ссылку на файл»."),
        [] | [_] => bail!("Ссылка на файл не найдена."),
        _ => bail!("Найдено несколько источников для файлов с одинаковым содержимым."),
    }
}

pub(crate) fn open(path: &Path, as_page: bool) -> Result<()> {
    let path = path.canonicalize()?;
    let url = lookup(&database()?, &path, as_page)?;
    ensure!(
        Command::new("/usr/bin/xdg-open")
            .arg(url)
            .status()?
            .success(),
        "Не удалось открыть браузер"
    );
    Ok(())
}

fn menu() -> Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .context("Не найдена папка данных")?
        .join("kio/servicemenus")
        .join(MENU_NAME))
}

fn managed(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
        && fs::read_to_string(path).is_ok_and(|text| text.lines().any(|line| line == MENU_MARKER))
}

pub(crate) fn enabled() -> bool {
    menu().is_ok_and(|path| managed(&path))
}

pub(crate) fn set_enabled(executable: &Path, enable: bool) -> Result<()> {
    let path = menu()?;
    if enable {
        ensure!(
            !crate::browser_bridge::path_exists(&path)? || managed(&path),
            "Существующая запись Dolphin создана другим приложением"
        );
        ensure!(
            executable.is_absolute() && executable.is_file(),
            "Не найден исполняемый файл приложения"
        );
        crate::browser_bridge::install(executable)?;
    }
    write_menu(&path, executable, enable)
}

fn write_menu(target: &Path, executable: &Path, enable: bool) -> Result<()> {
    if crate::browser_bridge::path_exists(target)? {
        ensure!(
            managed(target),
            "Существующая запись Dolphin создана другим приложением"
        );
        if !enable {
            return Ok(fs::remove_file(target)?);
        }
    }
    if !enable {
        return Ok(());
    }
    let executable = crate::autostart::executable_argument(executable)?;
    let text = format!(
        r#"[Desktop Entry]
Type=Service
MimeType=all/allfiles;
X-KDE-Protocols=file
X-KDE-RequiredNumberOfUrls=1
Actions=OpenDownloadPage;OpenDownloadFileURL;
{MENU_MARKER}

[Desktop Action OpenDownloadPage]
Name=Open Download Page
Name[ru]=Открыть страницу загрузки
Icon=internet-web-browser
Exec=/usr/bin/env -- {executable} --open-download-page %f

[Desktop Action OpenDownloadFileURL]
Name=Open File Download Link
Name[ru]=Открыть ссылку на файл
Icon=insert-link
Exec=/usr/bin/env -- {executable} --open-download-file-url %f
"#
    );
    crate::browser_bridge::write_atomic(target, text.as_bytes(), 0o700)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn moves_match_by_identity_or_content_without_guessing_ambiguous_sources() {
        let directory = tempfile::tempdir().unwrap();
        let db = Connection::open_in_memory().unwrap();
        create_tables(&db).unwrap();
        let original = directory.path().join("download.zip");
        let renamed = directory.path().join("renamed.zip");
        let copied = directory.path().join("copied.zip");
        let unidentified = directory.path().join("unidentified.zip");
        fs::write(&original, "downloaded bytes").unwrap();
        let link = links("https://one.example/item", "https://one.example/file.zip");
        save(&db, &Snapshot::read(&original).unwrap(), &link).unwrap();

        fs::rename(&original, &renamed).unwrap();
        fs::copy(&renamed, &copied).unwrap();
        fs::copy(&renamed, &unidentified).unwrap();
        assert_eq!(lookup(&db, &renamed, true).unwrap(), link.page);
        assert_eq!(lookup(&db, &copied, false).unwrap(), link.file);

        let other_download = directory.path().join("from-elsewhere.zip");
        fs::copy(&renamed, &other_download).unwrap();
        let other = links("https://two.example/item", "https://two.example/file.zip");
        save(&db, &Snapshot::read(&other_download).unwrap(), &other).unwrap();
        assert_eq!(lookup(&db, &other_download, true).unwrap(), other.page);
        assert!(lookup(&db, &copied, true).is_err());
        assert!(lookup(&db, &unidentified, true)
            .unwrap_err()
            .to_string()
            .contains("несколько"));

        fs::write(&copied, "changed data").unwrap();
        assert!(lookup(&db, &copied, true).is_err());
        assert!(web_url("file:///tmp/index.html").is_empty());
        assert!(web_url("https://user:password@example.test/path").is_empty());
    }

    #[test]
    fn service_menu_covers_single_local_files_and_respects_existing_entries() {
        let directory = tempfile::tempdir().unwrap();
        let desktop = directory.path().join(MENU_NAME);
        let application = directory.path().join("Downloads Triage %.AppImage");
        fs::write(&application, "").unwrap();
        write_menu(&desktop, &application, true).unwrap();
        let content = fs::read_to_string(&desktop).unwrap();
        assert!(content.contains("MimeType=all/allfiles;"));
        assert!(content.contains("X-KDE-Protocols=file"));
        assert!(content.contains("%%.AppImage\" --open-download-page %f"));
        assert_eq!(
            fs::metadata(&desktop).unwrap().permissions().mode() & 0o777,
            0o700
        );
        write_menu(&desktop, &application, false).unwrap();
        assert!(!desktop.exists());
        fs::write(&desktop, "user data").unwrap();
        assert!(write_menu(&desktop, &application, true).is_err());
    }
}
