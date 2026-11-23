use super::{bytes, download_origin, from_bytes, Snapshot, Stamp};
use anyhow::{ensure, Context, Result};
use cxx_qt_lib::QUrl;
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Candidate {
    path: PathBuf,
    url: String,
}

struct Origin {
    id: i64,
    url: String,
    snapshot: Snapshot,
    path: Option<PathBuf>,
}

#[derive(Serialize)]
struct Entry {
    id: i64,
    name: String,
    available: bool,
}

#[derive(Serialize)]
pub(crate) struct PageDownloads {
    files: Vec<Entry>,
    legacy_urls: Vec<String>,
}

struct Located {
    id: i64,
    url: String,
    hash: String,
    path: Option<PathBuf>,
    available: bool,
}

fn same_file(path: &Path, expected: &Snapshot) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    let current = Stamp::of(&metadata);
    let saved = &expected.stamp;
    metadata.is_file()
        && current.dev == saved.dev
        && current.ino == saved.ino
        && current.size == saved.size
        && current.mtime == saved.mtime
        && current.mtime_ns == saved.mtime_ns
        && (current == *saved
            || Snapshot::read(path).is_ok_and(|observed| expected.same_content(&observed)))
}

fn history() -> Result<Option<Connection>> {
    let path = dirs::data_local_dir()
        .context("Не найдена папка данных")?
        .join("downloads-triage/history.sqlite3");
    if !crate::browser_bridge::path_exists(&path)? {
        return Ok(None);
    }
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.busy_timeout(Duration::from_secs(2))?;
    Ok(Some(db))
}

fn origins(db: &Connection, page: &str) -> Result<Vec<Origin>> {
    let mut query = db.prepare(
        "SELECT o.rowid,o.url,o.hash,o.device,o.inode,o.size,o.mtime,o.ns,o.ctime,o.cns,l.path
         FROM download_origins o LEFT JOIN download_locations l ON l.origin=o.rowid
         WHERE o.page=?1 ORDER BY o.rowid DESC",
    )?;
    let mut rows = query.query([page])?;
    let mut origins = Vec::new();
    while let Some(row) = rows.next()? {
        origins.push(Origin {
            id: row.get(0)?,
            url: row.get(1)?,
            snapshot: Snapshot {
                hash: row.get(2)?,
                stamp: Stamp {
                    dev: row.get::<_, String>(3)?.parse()?,
                    ino: row.get::<_, String>(4)?.parse()?,
                    size: row.get(5)?,
                    mtime: row.get(6)?,
                    mtime_ns: row.get(7)?,
                    ctime: row.get(8)?,
                    ctime_ns: row.get(9)?,
                },
                directory: false,
            },
            path: row.get::<_, Option<Vec<u8>>>(10)?.map(from_bytes),
        });
    }
    Ok(origins)
}

fn legacy_locations(
    history: &Connection,
    hashes: &HashSet<&str>,
) -> Result<Vec<(PathBuf, Snapshot)>> {
    let mut query = history.prepare(
        "SELECT CASE state WHEN 'undone' THEN source ELSE destination END,snapshot
         FROM operations WHERE state IN ('moved','undone') ORDER BY id DESC",
    )?;
    let mut rows = query.query([])?;
    let mut locations = Vec::new();
    while let Some(row) = rows.next()? {
        let encoded: String = row.get(1)?;
        if let Ok(snapshot) = serde_json::from_str::<Snapshot>(&encoded) {
            if !snapshot.directory && hashes.contains(snapshot.hash.as_str()) {
                locations.push((from_bytes(row.get(0)?), snapshot));
            }
        }
    }
    Ok(locations)
}

fn follow_transfers(
    history: &Connection,
    path: &mut PathBuf,
    snapshot: &mut Snapshot,
) -> Result<()> {
    let mut last_id = 0i64;
    let mut query = history.prepare(
        "SELECT id,destination,before,after FROM origin_transfers
         WHERE source=?1 AND hash=?2 AND id>?3 ORDER BY id",
    )?;
    loop {
        let mut rows = query.query(params![bytes(path), snapshot.hash, last_id])?;
        let mut found = false;
        while let Some(row) = rows.next()? {
            let before: Snapshot = serde_json::from_str(&row.get::<_, String>(2)?)?;
            if snapshot.same_content(&before) {
                last_id = row.get(0)?;
                *path = from_bytes(row.get(1)?);
                *snapshot = serde_json::from_str(&row.get::<_, String>(3)?)?;
                found = true;
                break;
            }
        }
        if !found {
            return Ok(());
        }
    }
}

fn locate(
    db: &Connection,
    history: Option<&Connection>,
    page: &str,
    candidates: &[Candidate],
) -> Result<Vec<Located>> {
    let mut origins = origins(db, page)?;
    let hashes = origins
        .iter()
        .filter(|item| item.path.is_none())
        .map(|item| item.snapshot.hash.as_str())
        .collect::<HashSet<_>>();
    let legacy = match history {
        Some(history) if !hashes.is_empty() => legacy_locations(history, &hashes)?,
        _ => Vec::new(),
    };
    let follow = history
        .map(|history| {
            history.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='origin_transfers')",
                [],
                |row| row.get::<_, bool>(0),
            )
        })
        .transpose()?
        .unwrap_or(false);

    let mut located = Vec::new();
    for origin in &mut origins {
        if origin.path.is_none() {
            origin.path = candidates
                .iter()
                .filter(|candidate| candidate.path.is_absolute() && candidate.url == origin.url)
                .find_map(|candidate| {
                    let actual = candidate.path.canonicalize().ok()?;
                    same_file(&actual, &origin.snapshot).then_some(actual)
                })
                .or_else(|| {
                    legacy
                        .iter()
                        .find(|(_, snapshot)| origin.snapshot.same_content(snapshot))
                        .map(|(path, _)| path.clone())
                });
            if let Some(path) = origin.path.as_ref() {
                db.execute(
                    "INSERT OR IGNORE INTO download_locations(origin,path) VALUES(?1,?2)",
                    params![origin.id, bytes(path)],
                )?;
            }
        }
        let mut snapshot = origin.snapshot.clone();
        if let (true, Some(history), Some(path)) = (follow, history, origin.path.as_mut()) {
            follow_transfers(history, path, &mut snapshot)?;
        }
        let available = origin
            .path
            .as_ref()
            .is_some_and(|path| same_file(path, &snapshot));
        located.push(Located {
            id: origin.id,
            url: origin.url.clone(),
            hash: snapshot.hash,
            path: origin.path.clone(),
            available,
        });
    }

    Ok(located)
}

fn unique_entries(mut located: Vec<Located>) -> Vec<Located> {
    let located_keys = located
        .iter()
        .filter(|item| item.path.is_some())
        .map(|item| (item.url.clone(), item.hash.clone()))
        .collect::<HashSet<_>>();
    let mut paths = HashSet::new();
    let mut unresolved = HashSet::new();
    located.retain(|item| {
        if let Some(path) = item.path.as_ref() {
            paths.insert((path.clone(), item.url.clone(), item.hash.clone()))
        } else {
            let key = (item.url.clone(), item.hash.clone());
            !located_keys.contains(&key) && unresolved.insert(key)
        }
    });
    located
}

fn validated(page: &str) -> Result<String> {
    ensure!(
        download_origin::enabled(),
        "Включите связь с браузером в настройках «Загрузок на разбор»."
    );
    let page = download_origin::web_url(page);
    ensure!(!page.is_empty(), "Страница недоступна для поиска");
    Ok(page)
}

pub(crate) fn list(page: &str, candidates: &[Candidate]) -> Result<PageDownloads> {
    let page = validated(page)?;
    let history = history()?;
    let located = unique_entries(locate(
        &download_origin::database()?,
        history.as_ref(),
        &page,
        candidates,
    )?);
    let mut legacy_urls = located
        .iter()
        .filter(|file| file.path.is_none())
        .map(|file| file.url.clone())
        .collect::<Vec<_>>();
    legacy_urls.sort();
    legacy_urls.dedup();
    let files = located
        .into_iter()
        .map(|file| {
            let name = file
                .path
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .or_else(|| {
                    QUrl::from(file.url.as_str())
                        .path()
                        .to_string()
                        .rsplit('/')
                        .next()
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "Загрузка".into());
            Entry {
                id: file.id,
                name,
                available: file.available,
            }
        })
        .collect();
    Ok(PageDownloads { files, legacy_urls })
}

pub(crate) fn show(page: &str, id: i64) -> Result<bool> {
    let page = validated(page)?;
    let history = history()?;
    let located = locate(&download_origin::database()?, history.as_ref(), &page, &[])?;
    let Some(path) = located
        .into_iter()
        .find(|file| file.id == id && file.available)
        .and_then(|file| file.path)
    else {
        return Ok(false);
    };
    if Command::new("dolphin")
        .args(["--select", "--"])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
    {
        return Ok(true);
    }
    let folder = path.parent().context("Папка файла недоступна")?;
    Command::new("xdg-open")
        .arg(folder)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Не удалось открыть папку файла")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Engine;
    use std::os::unix::fs::MetadataExt;

    const PAGE: &str = "https://example.test/releases?version=1#files";
    const OTHER_PAGE: &str = "https://example.test/releases?version=2#files";

    fn add(db: &Connection, path: &Path, page: &str, url: &str, has_path: bool) -> i64 {
        let snapshot = Snapshot::read(path).unwrap();
        let s = snapshot.stamp;
        db.execute(
            "INSERT INTO download_origins VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                s.dev.to_string(),
                s.ino.to_string(),
                s.size,
                s.mtime,
                s.mtime_ns,
                s.ctime,
                s.ctime_ns,
                snapshot.hash,
                page,
                url
            ],
        )
        .unwrap();
        let id = db.last_insert_rowid();
        if has_path {
            db.execute(
                "INSERT INTO download_locations VALUES(?1,?2)",
                params![id, bytes(path)],
            )
            .unwrap();
        }
        id
    }

    #[test]
    fn page_keeps_both_downloads_and_follows_sorter_across_filesystems_and_undo() {
        let temp = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir_in("/dev/shm").unwrap();
        let first = temp.path().join("first.zip");
        let second = temp.path().join("second.zip");
        let target = other.path().join("first (2).zip");
        let same_name_dir = temp.path().join("other");
        fs::create_dir(&same_name_dir).unwrap();
        let same_name = same_name_dir.join("first.zip");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();
        fs::write(&same_name, b"first").unwrap();
        let db = Connection::open_in_memory().unwrap();
        download_origin::create_tables(&db).unwrap();
        let id = add(&db, &first, PAGE, "https://cdn.test/first", true);
        let second_id = add(&db, &second, PAGE, "https://cdn.test/second", true);
        let other_id = add(&db, &same_name, OTHER_PAGE, "https://cdn.test/first", true);

        let mut engine =
            Engine::open(&temp.path().join("config"), &temp.path().join("data")).unwrap();
        let snapshot = Snapshot::read(&first).unwrap();
        let device = fs::metadata(other.path()).unwrap().dev();
        assert_ne!(snapshot.stamp.dev, device);
        engine.db.execute("INSERT INTO operations(id,source,destination,snapshot,state) VALUES(1,?1,?2,?3,'planned')",
            params![bytes(&first), bytes(&target), serde_json::to_string(&snapshot).unwrap()]).unwrap();
        engine
            .db
            .execute(
                "INSERT INTO batch_members VALUES(1,1,1,?1)",
                [snapshot.stamp.dev.to_string()],
            )
            .unwrap();
        let after = engine
            .transfer(1, "move", &first, &target, &snapshot, device)
            .unwrap();
        engine
            .finish_transfer(1, &first, &target, &snapshot, &after, "moved", "")
            .unwrap();

        let files = unique_entries(locate(&db, Some(&engine.db), PAGE, &[]).unwrap());
        assert_eq!(files.len(), 2);
        assert!(files
            .iter()
            .find(|file| file.id == id)
            .is_some_and(|file| file.available && file.path.as_deref() == Some(target.as_path())));
        assert!(files
            .iter()
            .find(|file| file.id == second_id)
            .is_some_and(|file| file.available && file.path.as_deref() == Some(second.as_path())));
        let other_files = locate(&db, Some(&engine.db), OTHER_PAGE, &[]).unwrap();
        assert_eq!(other_files[0].id, other_id);
        assert_eq!(other_files[0].path.as_deref(), Some(same_name.as_path()));

        engine.undo(1).unwrap();
        let files = locate(&db, Some(&engine.db), PAGE, &[]).unwrap();
        assert!(files
            .iter()
            .find(|file| file.id == id)
            .is_some_and(|file| file.available && file.path.as_deref() == Some(first.as_path())));
        fs::remove_file(&first).unwrap();
        fs::write(&first, b"replacement").unwrap();
        let files = locate(&db, Some(&engine.db), PAGE, &[]).unwrap();
        assert!(files
            .iter()
            .find(|file| file.id == id)
            .is_some_and(|file| !file.available));
    }

    #[test]
    fn legacy_paths_are_recovered_from_browser_history_and_old_moves() {
        let temp = tempfile::tempdir().unwrap();
        let previous = temp.path().join("previous.zip");
        let sorted = temp.path().join("sorted.zip");
        fs::write(&previous, b"historic").unwrap();
        fs::write(&sorted, b"sorted").unwrap();
        let db = Connection::open_in_memory().unwrap();
        download_origin::create_tables(&db).unwrap();
        let previous_url = "https://cdn.test/previous";
        let previous_id = add(&db, &previous, PAGE, previous_url, false);
        let sorted_id = add(&db, &sorted, PAGE, "https://cdn.test/sorted", false);

        let history = Connection::open_in_memory().unwrap();
        history.execute("CREATE TABLE operations(id INTEGER,source BLOB,destination BLOB,snapshot TEXT,state TEXT)", []).unwrap();
        history
            .execute(
                "INSERT INTO operations VALUES(1,?1,?2,?3,'moved')",
                params![
                    bytes(&temp.path().join("old.zip")),
                    bytes(&sorted),
                    serde_json::to_string(&Snapshot::read(&sorted).unwrap()).unwrap()
                ],
            )
            .unwrap();
        let files = locate(
            &db,
            Some(&history),
            PAGE,
            &[Candidate {
                path: previous.clone(),
                url: previous_url.into(),
            }],
        )
        .unwrap();
        assert!(
            files
                .iter()
                .find(|file| file.id == previous_id)
                .is_some_and(
                    |file| file.available && file.path.as_deref() == Some(previous.as_path())
                )
        );
        assert!(files
            .iter()
            .find(|file| file.id == sorted_id)
            .is_some_and(|file| file.available && file.path.as_deref() == Some(sorted.as_path())));
        fs::remove_file(&previous).unwrap();
        let files = locate(&db, Some(&history), PAGE, &[]).unwrap();
        assert!(files
            .iter()
            .find(|file| file.id == previous_id)
            .is_some_and(
                |file| !file.available && file.path.as_deref() == Some(previous.as_path())
            ));
    }
}
