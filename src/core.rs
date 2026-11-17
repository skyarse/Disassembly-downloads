use anyhow::{bail, ensure, Context, Result};
use fs2::FileExt;
use globset::GlobBuilder;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    ffi::{CString, OsString},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub extensions: Vec<String>,
    pub pattern: String,
}
impl Condition {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.extensions.is_empty() || !self.pattern.is_empty(),
            "Укажите расширение или шаблон"
        );
        for ext in &self.extensions {
            ensure!(
                !ext.is_empty() && !ext.contains(['/', '\\', '*', '?', '.']),
                "Расширения указываются без точки и шаблонов"
            );
        }
        if !self.pattern.is_empty() {
            self.glob()?;
        }
        Ok(())
    }
    fn glob(&self) -> Result<globset::GlobMatcher> {
        Ok(GlobBuilder::new(&self.pattern)
            .case_insensitive(true)
            .literal_separator(true)
            .build()?
            .compile_matcher())
    }
    pub fn matches(&self, path: &Path) -> bool {
        let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("");
        let ext_ok = self.extensions.is_empty()
            || self.extensions.iter().any(|x| x.eq_ignore_ascii_case(ext));
        let name_ok = self.pattern.is_empty()
            || self
                .glob()
                .is_ok_and(|g| g.is_match(path.file_name().unwrap_or_default()));
        ext_ok && name_ok
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub folder: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub condition: Condition,
    pub category_id: String,
    pub action: String,
    pub active: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub root: PathBuf,
    pub categories: Vec<Category>,
    pub rules: Vec<Rule>,
    pub exclusions: Vec<Condition>,
    #[serde(default = "manual_mode")]
    pub mode: String,
    #[serde(default = "system_theme")]
    pub theme: String,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default = "legacy_setup_complete")]
    pub setup_complete: bool,
}
fn manual_mode() -> String {
    "manual".into()
}
fn system_theme() -> String {
    "system".into()
}
fn legacy_setup_complete() -> bool {
    true
}
impl Config {
    fn initial() -> Self {
        Self {
            version: 1,
            mode: manual_mode(),
            theme: system_theme(),
            autostart: false,
            setup_complete: false,
            root: dirs::download_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Downloads")),
            categories: vec![
                Category {
                    id: "pictures".into(),
                    name: "Картинки".into(),
                    folder: "Картинки".into(),
                },
                Category {
                    id: "documents".into(),
                    name: "Документы".into(),
                    folder: "Документы".into(),
                },
                Category {
                    id: "music".into(),
                    name: "Музыка".into(),
                    folder: "Музыка".into(),
                },
            ],
            rules: vec![
                Rule {
                    id: "pictures".into(),
                    name: "Изображения".into(),
                    condition: Condition {
                        extensions: vec![
                            "png".into(),
                            "jpg".into(),
                            "jpeg".into(),
                            "webp".into(),
                            "avif".into(),
                        ],
                        pattern: String::new(),
                    },
                    category_id: "pictures".into(),
                    action: "move".into(),
                    active: true,
                },
                Rule {
                    id: "documents".into(),
                    name: "Документы".into(),
                    condition: Condition {
                        extensions: vec!["pdf".into(), "docx".into(), "odt".into(), "txt".into()],
                        pattern: String::new(),
                    },
                    category_id: "documents".into(),
                    action: "move".into(),
                    active: true,
                },
                Rule {
                    id: "music".into(),
                    name: "Музыка".into(),
                    condition: Condition {
                        extensions: vec!["flac".into(), "mp3".into(), "ogg".into(), "opus".into()],
                        pattern: String::new(),
                    },
                    category_id: "music".into(),
                    action: "move".into(),
                    active: true,
                },
            ],
            exclusions: vec![Condition {
                extensions: Vec::new(),
                pattern: ".*".into(),
            }],
        }
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "Неподдерживаемая версия настроек");
        ensure!(
            ["manual", "automatic", "paused"].contains(&self.mode.as_str()),
            "Неизвестный режим сортировки"
        );
        ensure!(
            ["system", "light", "dark"].contains(&self.theme.as_str()),
            "Неизвестная тема оформления"
        );
        ensure!(
            self.root.is_absolute() && self.root.is_dir(),
            "Выберите существующую папку загрузок"
        );
        let root = self.root.canonicalize()?;
        let mut ids = HashSet::new();
        for category in &self.categories {
            ensure!(
                !category.id.is_empty() && ids.insert(&category.id),
                "Повторяющийся ID категории"
            );
            ensure!(
                !category.name.trim().is_empty(),
                "У категории должно быть название"
            );
            safe_destination(&root, Path::new(&category.folder), false)?;
        }
        let mut rules = HashSet::new();
        for rule in &self.rules {
            ensure!(
                !rule.id.is_empty() && rules.insert(&rule.id),
                "Повторяющийся ID правила"
            );
            ensure!(
                !rule.name.trim().is_empty(),
                "У правила должно быть название"
            );
            ensure!(
                ids.contains(&rule.category_id),
                "Категория правила не существует"
            );
            ensure!(
                ["move", "suggest"].contains(&rule.action.as_str()),
                "Неизвестное действие"
            );
            rule.condition.validate()?;
        }
        for exclusion in &self.exclusions {
            exclusion.validate()?;
        }
        Ok(())
    }
    fn decision(&self, path: &Path) -> Option<(&Rule, &Category)> {
        if self.exclusions.iter().any(|c| c.matches(path)) {
            return None;
        }
        let rule = self
            .rules
            .iter()
            .find(|r| r.active && r.condition.matches(path))?;
        let category = self.categories.iter().find(|c| c.id == rule.category_id)?;
        Some((rule, category))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: i64,
    mtime_ns: i64,
    ctime: i64,
    ctime_ns: i64,
}
impl Stamp {
    fn of(meta: &fs::Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            size: meta.len(),
            mtime: meta.mtime(),
            mtime_ns: meta.mtime_nsec(),
            ctime: meta.ctime(),
            ctime_ns: meta.ctime_nsec(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Snapshot {
    stamp: Stamp,
    hash: String,
}
impl Snapshot {
    fn read(path: &Path) -> Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let before = file.metadata()?;
        ensure!(before.is_file(), "Разрешены только обычные файлы");
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 262144];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        let after = file.metadata()?;
        ensure!(
            Stamp::of(&before) == Stamp::of(&after),
            "Файл изменился во время проверки"
        );
        let stamp = Stamp::of(&after);
        ensure!(
            Stamp::of(&fs::symlink_metadata(path)?) == stamp,
            "Файл был подменён"
        );
        Ok(Self {
            stamp,
            hash: format!("{:x}", digest.finalize()),
        })
    }
    fn verify(&self, path: &Path) -> Result<()> {
        let current = Self::read(path)?;
        ensure!(
            current.stamp == self.stamp && current.hash == self.hash,
            "Файл изменился после предпросмотра. Обновите очередь"
        );
        Ok(())
    }
    fn same_content(&self, other: &Self) -> bool {
        self.hash == other.hash
            && self.stamp.dev == other.stamp.dev
            && self.stamp.ino == other.stamp.ino
            && self.stamp.size == other.stamp.size
            && self.stamp.mtime == other.stamp.mtime
            && self.stamp.mtime_ns == other.stamp.mtime_ns
    }
}
struct Pending {
    path: PathBuf,
    snapshot: Option<Snapshot>,
}
pub struct Engine {
    pub config: Config,
    config_path: PathBuf,
    state_directory: PathBuf,
    db: Connection,
    _lock: File,
    pending: HashMap<String, Pending>,
    observed: HashMap<PathBuf, (Stamp, Instant)>,
    snapshots: HashMap<PathBuf, Snapshot>,
    tokens: HashMap<PathBuf, (Stamp, String)>,
    pause_guard: Arc<AtomicBool>,
    queue: Vec<Value>,
    sequence: u64,
    message: String,
    rule_test: Vec<String>,
    first_run: bool,
}
fn bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_bytes()
}
fn from_bytes(value: Vec<u8>) -> PathBuf {
    OsString::from_vec(value).into()
}
fn present(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}
fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
fn temporary(path: &Path) -> bool {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    [
        ".part",
        ".crdownload",
        ".download",
        ".tmp",
        ".partial",
        ".aria2",
    ]
    .iter()
    .any(|e| name.ends_with(e))
}
fn safe_destination(root: &Path, input: &Path, create: bool) -> Result<PathBuf> {
    let relative = if input.is_absolute() {
        input
            .strip_prefix(root)
            .context("В первой версии назначение должно находиться внутри выбранной папки")?
    } else {
        input
    };
    ensure!(
        relative.components().next().is_some(),
        "Выберите подпапку назначения"
    );
    let mut path = root.to_path_buf();
    for part in relative.components() {
        let Component::Normal(name) = part else {
            bail!("Недопустимый путь назначения")
        };
        path.push(name);
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                ensure!(
                    meta.is_dir() && !meta.file_type().is_symlink(),
                    "Путь назначения содержит ссылку или файл"
                );
                ensure!(
                    meta.dev() == fs::metadata(root)?.dev(),
                    "Перенос между дисками появится на третьем этапе"
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if create {
                    fs::create_dir(&path)?;
                    sync_dir(path.parent().unwrap())?;
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(path)
}
fn rename_noreplace(source: &Path, destination: &Path) -> Result<()> {
    let source_parent = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(source.parent().context("Нет исходной папки")?)?;
    let destination_parent = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(destination.parent().context("Нет целевой папки")?)?;
    let source_name = CString::new(source.file_name().context("Нет имени")?.as_bytes())?;
    let destination_name = CString::new(destination.file_name().context("Нет имени")?.as_bytes())?;
    let result = unsafe {
        libc::renameat2(
            source_parent.as_raw_fd(),
            source_name.as_ptr(),
            destination_parent.as_raw_fd(),
            destination_name.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    destination_parent.sync_all()?;
    source_parent.sync_all()?;
    Ok(())
}
fn next_name(directory: &Path, source: &Path) -> PathBuf {
    let name = source.file_name().unwrap();
    let mut result = directory.join(name);
    let mut count = 2;
    while present(&result) {
        let mut name = source.file_stem().unwrap_or(name).to_os_string();
        name.push(format!(" ({count})"));
        if let Some(ext) = source.extension() {
            name.push(".");
            name.push(ext);
        }
        result = directory.join(name);
        count += 1;
    }
    result
}
impl Engine {
    pub fn open_default() -> Result<Self> {
        let config = dirs::config_dir()
            .context("Не найдена папка настроек")?
            .join("downloads-triage");
        let state = dirs::data_local_dir()
            .context("Не найдена папка данных")?
            .join("downloads-triage");
        let mut engine = Self::open(&config, &state)?;
        engine.config.autostart = crate::autostart::enabled(&engine.autostart_directory()?);
        Ok(engine)
    }
    fn open(config_directory: &Path, state: &Path) -> Result<Self> {
        fs::create_dir_all(config_directory)?;
        fs::create_dir_all(state)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(state.join("instance.lock"))?;
        lock.try_lock_exclusive()
            .context("Приложение уже запущено")?;
        let config_path = config_directory.join("settings.json");
        let first_run = !config_path.exists();
        let config: Config = if first_run {
            Config::initial()
        } else {
            serde_json::from_slice(&fs::read(&config_path)?)
                .context("Настройки повреждены; исходный файл сохранён")?
        };
        let first_run = first_run || !config.setup_complete;
        let db = Connection::open(state.join("history.sqlite3"))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS operations(id INTEGER PRIMARY KEY, created TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, source BLOB NOT NULL, destination BLOB NOT NULL, snapshot TEXT NOT NULL, state TEXT NOT NULL, message TEXT NOT NULL DEFAULT '');
            CREATE TABLE IF NOT EXISTS manual_only(path BLOB PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS deferred(path BLOB PRIMARY KEY);")?;
        let mut engine = Self {
            config,
            config_path,
            state_directory: state.canonicalize()?,
            db,
            _lock: lock,
            pending: HashMap::new(),
            observed: HashMap::new(),
            snapshots: HashMap::new(),
            tokens: HashMap::new(),
            pause_guard: Arc::new(AtomicBool::new(false)),
            queue: Vec::new(),
            sequence: 0,
            message: String::new(),
            rule_test: Vec::new(),
            first_run,
        };
        engine.recover()?;
        Ok(engine)
    }
    fn validate_watch_root(&self, root: &Path) -> Result<()> {
        let config_directory = self.config_path.parent().unwrap().canonicalize()?;
        ensure!(
            !root.starts_with(config_directory) && !root.starts_with(&self.state_directory),
            "Нельзя сортировать папки настроек и журнала приложения"
        );
        Ok(())
    }
    fn autostart_directory(&self) -> Result<PathBuf> {
        Ok(self
            .config_path
            .parent()
            .and_then(Path::parent)
            .context("Не найдена папка автозапуска")?
            .join("autostart"))
    }
    fn persist(&mut self, mut config: Config) -> Result<()> {
        config.validate()?;
        config.root = config.root.canonicalize()?;
        self.validate_watch_root(&config.root)?;
        if self.first_run || self.config.root != config.root {
            config.mode = manual_mode();
        }
        config.setup_complete = true;
        self.write_settings(&config)?;
        if self.config.root != config.root {
            self.observed.clear();
            self.snapshots.clear();
            self.tokens.clear();
        }
        self.config = config;
        self.first_run = false;
        self.pending.clear();
        Ok(())
    }
    fn write_settings(&self, config: &Config) -> Result<()> {
        let temporary = self.config_path.with_extension("json.new");
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .custom_flags(libc::O_NOFOLLOW)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(config)?)?;
        file.sync_all()?;
        fs::rename(&temporary, &self.config_path)?;
        sync_dir(self.config_path.parent().unwrap())?;
        Ok(())
    }
    fn snapshot(&mut self, path: &Path, stamp: &Stamp) -> Result<Snapshot> {
        if let Some(value) = self.snapshots.get(path) {
            if value.stamp == *stamp {
                return Ok(value.clone());
            }
        }
        let value = Snapshot::read(path)?;
        ensure!(
            value.stamp == *stamp,
            "Файл изменился во время предпросмотра"
        );
        self.snapshots.insert(path.to_path_buf(), value.clone());
        Ok(value)
    }
    fn scan_at(&mut self, now: Instant) -> Result<()> {
        self.config.validate()?;
        let root = self.config.root.canonicalize()?;
        self.validate_watch_root(&root)?;
        self.pending.clear();
        self.queue.clear();
        let mut paths = fs::read_dir(&root)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.sort();
        let mut seen = HashSet::new();
        for path in paths {
            let meta = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if !meta.is_file() || meta.file_type().is_symlink() || temporary(&path) {
                continue;
            }
            seen.insert(path.clone());
            if self.config.exclusions.iter().any(|c| c.matches(&path)) {
                continue;
            }
            let deferred = self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM deferred WHERE path=?1)",
                params![bytes(&path)],
                |r| r.get::<_, bool>(0),
            )?;
            if deferred {
                continue;
            }
            let stamp = Stamp::of(&meta);
            let observation = self
                .observed
                .entry(path.clone())
                .or_insert((stamp.clone(), now));
            if observation.0 != stamp {
                *observation = (stamp, now);
            }
            let stable = now.saturating_duration_since(observation.1) >= Duration::from_secs(60);
            let restored = self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM manual_only WHERE path=?1)",
                params![bytes(&path)],
                |r| r.get::<_, bool>(0),
            )?;
            let decision = if restored {
                None
            } else {
                self.config.decision(&path)
            };
            let destination = decision.map(|(_, c)| c.folder.clone()).unwrap_or_default();
            let action = decision.map(|(r, _)| r.action.clone()).unwrap_or_default();
            let mut reason = if restored {
                "Требуется ручное решение после отмены или ошибки".into()
            } else if let Some((rule, _)) = decision {
                format!(
                    "{} · {}",
                    rule.name,
                    if rule.action == "suggest" {
                        "Предложить"
                    } else {
                        "Переместить"
                    }
                )
            } else {
                "Нет подходящего правила".into()
            };
            let snapshot = if stable {
                match self.snapshot(&path, &Stamp::of(&meta)) {
                    Ok(value) => Some(value),
                    Err(error) => {
                        reason = format!("{error:#}");
                        None
                    }
                }
            } else {
                reason = "Ожидание: минута без изменений с момента обнаружения".into();
                None
            };
            let current_stamp = Stamp::of(&meta);
            let token = match self.tokens.get(&path) {
                Some((previous, token)) if previous == &current_stamp => token.clone(),
                _ => {
                    self.sequence += 1;
                    let token = self.sequence.to_string();
                    self.tokens
                        .insert(path.clone(), (current_stamp, token.clone()));
                    token
                }
            };
            self.queue.push(json!({"id":token, "name":path.file_name().unwrap().to_string_lossy(), "path":path.to_string_lossy(), "size":meta.len(), "destination":destination, "reason":reason, "ready":snapshot.is_some(), "action":action, "manual_only":restored}));
            self.pending.insert(token, Pending { path, snapshot });
        }
        self.observed.retain(|path, _| seen.contains(path));
        self.snapshots.retain(|path, _| seen.contains(path));
        self.tokens.retain(|path, _| seen.contains(path));
        Ok(())
    }
    pub fn set_pause_guard(&mut self, guard: Arc<AtomicBool>) {
        self.pause_guard = guard;
    }
    fn last_operation_id(&self) -> Result<i64> {
        Ok(self
            .db
            .query_row("SELECT coalesce(max(id),0) FROM operations", [], |row| {
                row.get(0)
            })?)
    }
    fn automatic_moves(&mut self) -> Result<usize> {
        if self.first_run || self.config.mode != "automatic" {
            return Ok(0);
        }
        let candidates = self
            .queue
            .iter()
            .filter(|row| {
                row["ready"] == true && row["action"] == "move" && row["manual_only"] != true
            })
            .map(|row| {
                (
                    row["id"].as_str().unwrap().to_string(),
                    row["destination"].as_str().unwrap().to_string(),
                )
            })
            .collect::<Vec<_>>();
        let mut moved = 0;
        for (token, folder) in candidates {
            if self.pause_guard.load(Ordering::Acquire) {
                break;
            }
            let Some(pending) = self.pending.get(&token) else {
                continue;
            };
            let source = pending.path.clone();
            let snapshot = pending.snapshot.clone();
            let last_id = self.last_operation_id()?;
            match self.move_one(&token, &folder) {
                Ok(()) => moved += 1,
                Err(error) => {
                    if self.last_operation_id()? == last_id {
                        let target = self
                            .config
                            .root
                            .join(&folder)
                            .join(source.file_name().unwrap());
                        self.db.execute("INSERT INTO operations(source,destination,snapshot,state,message) VALUES(?1,?2,?3,'error',?4)",
                            params![bytes(&source), bytes(&target), serde_json::to_string(&snapshot)?, format!("{error:#}")])?;
                    }
                    self.db.execute(
                        "INSERT OR IGNORE INTO manual_only(path) VALUES(?1)",
                        params![bytes(&source)],
                    )?;
                }
            }
        }
        Ok(moved)
    }
    fn tick_at(&mut self, now: Instant) -> Result<()> {
        if self.first_run {
            return Ok(());
        }
        self.scan_at(now)?;
        let before = self.pending.len();
        let moved = self.automatic_moves()?;
        if self.pending.len() != before {
            self.scan_at(now)?;
        }
        if moved > 0 {
            self.message = format!("Автоматически перемещено: {moved}");
        }
        Ok(())
    }
    fn move_one(&mut self, token: &str, folder: &str) -> Result<()> {
        let pending = self
            .pending
            .remove(token)
            .context("Предпросмотр устарел. Обновите очередь")?;
        let snapshot = pending
            .snapshot
            .context("Файл ещё не готов к перемещению")?;
        let root = self.config.root.canonicalize()?;
        ensure!(
            pending.path.parent() == Some(root.as_path()),
            "Исходник за пределами папки загрузок"
        );
        let directory = safe_destination(&root, Path::new(folder), true)?;
        snapshot.verify(&pending.path)?;
        ensure!(
            fs::metadata(&directory)?.dev() == snapshot.stamp.dev,
            "Перенос между дисками пока недоступен"
        );
        let destination = next_name(&directory, &pending.path);
        self.db.execute(
            "INSERT INTO operations(source,destination,snapshot,state) VALUES(?1,?2,?3,'planned')",
            params![
                bytes(&pending.path),
                bytes(&destination),
                serde_json::to_string(&snapshot)?
            ],
        )?;
        let id = self.db.last_insert_rowid();
        let result = (|| -> Result<()> {
            snapshot.verify(&pending.path)?;
            rename_noreplace(&pending.path, &destination)?;
            let moved = Snapshot::read(&destination)?;
            ensure!(
                snapshot.same_content(&moved),
                "Файл изменился во время переноса; требуется проверка"
            );
            self.db.execute(
                "UPDATE operations SET state='moved',snapshot=?2 WHERE id=?1",
                params![id, serde_json::to_string(&moved)?],
            )?;
            self.db.execute(
                "DELETE FROM manual_only WHERE path=?1",
                params![bytes(&pending.path)],
            )?;
            Ok(())
        })();
        if let Err(error) = result {
            self.db.execute(
                "UPDATE operations SET state='needs_review',message=?2 WHERE id=?1",
                params![id, format!("{error:#}")],
            )?;
            return Err(error);
        }
        Ok(())
    }
    fn undo(&mut self, id: i64) -> Result<()> {
        let (source, destination, snapshot, state): (Vec<u8>, Vec<u8>, String, String) =
            self.db.query_row(
                "SELECT source,destination,snapshot,state FROM operations WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
        ensure!(state == "moved", "Эта операция недоступна для отмены");
        let source = from_bytes(source);
        let destination = from_bytes(destination);
        ensure!(
            !present(&source),
            "Исходный путь уже занят; отмена ничего не перезаписала"
        );
        let snapshot: Snapshot = serde_json::from_str(&snapshot)?;
        snapshot.verify(&destination)?;
        ensure!(
            source.parent().is_some_and(Path::is_dir),
            "Исходная папка недоступна"
        );
        let transaction = self.db.transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO manual_only(path) VALUES(?1)",
            params![bytes(&source)],
        )?;
        transaction.execute(
            "UPDATE operations SET state='undo_planned' WHERE id=?1",
            [id],
        )?;
        transaction.commit()?;
        let result = (|| -> Result<()> {
            snapshot.verify(&destination)?;
            rename_noreplace(&destination, &source)?;
            let restored = Snapshot::read(&source)?;
            ensure!(
                snapshot.same_content(&restored),
                "Файл изменился во время отмены; требуется проверка"
            );
            self.db
                .execute("UPDATE operations SET state='undone' WHERE id=?1", [id])?;
            Ok(())
        })();
        if let Err(error) = result {
            self.db.execute(
                "UPDATE operations SET state='needs_review',message=?2 WHERE id=?1",
                params![id, format!("{error:#}")],
            )?;
            return Err(error);
        }
        Ok(())
    }
    fn recover(&mut self) -> Result<()> {
        let rows = self.db.prepare("SELECT id,source,destination,snapshot,state FROM operations WHERE state IN ('planned','undo_planned')")?
            .query_map([], |r| Ok((r.get::<_,i64>(0)?, r.get::<_,Vec<u8>>(1)?, r.get::<_,Vec<u8>>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, source, destination, encoded, state) in rows {
            let source = from_bytes(source);
            let destination = from_bytes(destination);
            let snapshot: Snapshot = serde_json::from_str(&encoded)?;
            let (old, new, completed) = if state == "planned" {
                (&source, &destination, "moved")
            } else {
                (&destination, &source, "undone")
            };
            let current = Snapshot::read(new).ok();
            let (new_state, message) =
                if !present(old) && current.as_ref().is_some_and(|s| snapshot.same_content(s)) {
                    (completed, "Операция восстановлена по журналу")
                } else if present(old) && !present(new) {
                    (
                        if state == "planned" { "error" } else { "moved" },
                        "Перенос не завершился; исходный файл сохранён",
                    )
                } else {
                    (
                        "needs_review",
                        "Неоднозначное состояние после сбоя. Автоматическое удаление отключено",
                    )
                };
            let current = if new_state == "moved" && state == "planned" {
                current.unwrap_or(snapshot)
            } else {
                snapshot
            };
            self.db.execute(
                "UPDATE operations SET state=?2,message=?3,snapshot=?4 WHERE id=?1",
                params![id, new_state, message, serde_json::to_string(&current)?],
            )?;
        }
        Ok(())
    }
    fn history(&self) -> Result<Vec<Value>> {
        Ok(self.db.prepare("SELECT id,created,source,destination,state,message FROM operations ORDER BY id DESC LIMIT 500")?.query_map([], |r| {
            let source = from_bytes(r.get(2)?);
            let destination = from_bytes(r.get(3)?);
            Ok(json!({"id":r.get::<_,i64>(0)?, "created":r.get::<_,String>(1)?, "source":source.to_string_lossy(), "destination":destination.to_string_lossy(), "state":r.get::<_,String>(4)?, "message":r.get::<_,String>(5)?}))
        })?.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    fn execute(&mut self, request: Value) -> Result<()> {
        self.rule_test.clear();
        match request["op"].as_str().unwrap_or("") {
            "init" | "scan" | "tick" => self.tick_at(Instant::now())?,
            "set_theme" => {
                let theme = request["theme"].as_str().context("Не указана тема")?;
                ensure!(
                    ["system", "light", "dark"].contains(&theme),
                    "Неизвестная тема оформления"
                );
                let mut config = self.config.clone();
                config.theme = theme.into();
                self.write_settings(&config)?;
                self.config = config;
            }
            "set_mode" => {
                ensure!(
                    !self.first_run,
                    "Сначала выберите папку и просмотрите файлы"
                );
                let mut config = self.config.clone();
                config.mode = request["mode"].as_str().context("Не указан режим")?.into();
                self.persist(config)?;
                self.pause_guard
                    .store(self.config.mode != "automatic", Ordering::Release);
                self.tick_at(Instant::now())?;
            }
            "set_autostart" => {
                ensure!(!self.first_run, "Сначала сохраните папку загрузок");
                let enabled = request["enabled"]
                    .as_bool()
                    .context("Не указан режим автозапуска")?;
                let executable = match std::env::var_os("APPIMAGE") {
                    Some(path) => PathBuf::from(path),
                    None => std::env::current_exe()?,
                };
                let directory = self.autostart_directory()?;
                let previous = self.config.autostart;
                crate::autostart::set(&directory, &executable, enabled)?;
                let mut config = self.config.clone();
                config.autostart = enabled;
                if let Err(error) = self.persist(config) {
                    crate::autostart::set(&directory, &executable, previous).context(format!(
                        "Настройки не сохранены: {error:#}; не удалось восстановить автозапуск"
                    ))?;
                    return Err(error);
                }
                self.message = if enabled {
                    "Приложение будет запускаться при входе в систему"
                } else {
                    "Автозапуск отключён"
                }
                .into();
            }
            "save" => {
                let mut config: Config = serde_json::from_value(request["config"].clone())?;
                config.autostart = self.config.autostart;
                self.persist(config)?;
                self.scan_at(Instant::now())?;
                self.message = "Настройки сохранены".into();
            }
            "test" => {
                let condition: Condition = serde_json::from_value(request["condition"].clone())?;
                condition.validate()?;
                for entry in fs::read_dir(&self.config.root)? {
                    let entry = entry?;
                    if entry.file_type()?.is_file()
                        && !temporary(&entry.path())
                        && condition.matches(&entry.path())
                    {
                        self.rule_test
                            .push(entry.file_name().to_string_lossy().into_owned());
                    }
                }
                self.message = format!("Условию соответствуют файлов: {}. Исключения и правила выше могут иметь приоритет.", self.rule_test.len());
            }
            "move" => {
                let items = request["items"].as_array().context("Не выбраны файлы")?;
                ensure!(!items.is_empty(), "Не выбраны файлы");
                let mut errors = Vec::new();
                let mut success = 0;
                for item in items {
                    match self.move_one(
                        item["id"].as_str().context("Нет ID")?,
                        item["folder"].as_str().context("Нет назначения")?,
                    ) {
                        Ok(()) => success += 1,
                        Err(error) => errors.push(format!("{error:#}")),
                    }
                }
                self.scan_at(Instant::now())?;
                self.message = format!("Перемещено: {success}. {}", errors.join("; "));
            }
            "undo" => {
                self.undo(request["id"].as_i64().context("Нет операции")?)?;
                self.scan_at(Instant::now())?;
                self.message = "Файл восстановлен. Для него требуется ручное решение".into();
            }
            "defer" => {
                let token = request["id"].as_str().context("Нет ID")?;
                let item = self.pending.get(token).context("Предпросмотр устарел")?;
                self.db.execute(
                    "INSERT OR IGNORE INTO deferred(path) VALUES(?1)",
                    params![bytes(&item.path)],
                )?;
                self.scan_at(Instant::now())?;
            }
            "show_deferred" => {
                self.db.execute("DELETE FROM deferred", [])?;
                self.scan_at(Instant::now())?;
            }
            "import" => {
                let path = Path::new(request["path"].as_str().context("Нет файла")?);
                ensure!(
                    fs::metadata(path)?.len() <= 2_000_000,
                    "Файл настроек слишком большой"
                );
                let mut config: Config = serde_json::from_slice(&fs::read(path)?)?;
                config.mode = manual_mode();
                config.autostart = self.config.autostart;
                self.persist(config)?;
                self.scan_at(Instant::now())?;
                self.message = "Настройки импортированы; файлы не перемещались".into();
            }
            "export" => {
                let path = Path::new(request["path"].as_str().context("Нет файла")?);
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path)
                    .context("Экспорт требует нового имени файла")?;
                file.write_all(&serde_json::to_vec_pretty(&self.config)?)?;
                file.sync_all()?;
                self.message = "Настройки экспортированы".into();
            }
            _ => bail!("Неизвестная команда"),
        }
        Ok(())
    }
    pub fn command(&mut self, command: &str) -> String {
        self.message.clear();
        let request = serde_json::from_str::<Value>(command);
        let request_id = request
            .as_ref()
            .ok()
            .map(|r| r["request_id"].clone())
            .unwrap_or(Value::Null);
        let operation = request
            .as_ref()
            .ok()
            .map(|r| r["op"].clone())
            .unwrap_or(Value::Null);
        let result = request
            .map_err(anyhow::Error::from)
            .and_then(|r| self.execute(r));
        let error = result.err().map(|e| format!("{e:#}")).unwrap_or_default();
        json!({"config":self.config,"first_run":self.first_run,"queue":self.queue,"history":self.history().unwrap_or_default(),"message":self.message,"error":error,"matches":self.rule_test,"request_id":request_id,"operation":operation}).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    fn fixture() -> (TempDir, Engine) {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("Downloads");
        fs::create_dir(&root).unwrap();
        let mut engine =
            Engine::open(&temp.path().join("config"), &temp.path().join("state")).unwrap();
        engine.config.root = root;
        engine.persist(engine.config.clone()).unwrap();
        (temp, engine)
    }
    fn ready(engine: &mut Engine) -> String {
        let now = Instant::now();
        engine.scan_at(now).unwrap();
        engine.scan_at(now + Duration::from_secs(61)).unwrap();
        engine.queue.iter().find(|r| r["ready"] == true).unwrap()["id"]
            .as_str()
            .unwrap()
            .into()
    }
    #[test]
    fn precise_rules_win_and_conditions_are_combined() {
        let mut config = Config::initial();
        let mut rule = config.rules[0].clone();
        rule.name = "Screenshot".into();
        rule.condition.pattern = "Screenshot*".into();
        config.rules.insert(0, rule);
        assert_eq!(
            config
                .decision(Path::new("Screenshot01.PNG"))
                .unwrap()
                .0
                .name,
            "Screenshot"
        );
        assert!(config.decision(Path::new("Screenshot01.zip")).is_none());
        config.exclusions.push(Condition {
            extensions: vec![],
            pattern: "Screenshot*".into(),
        });
        assert!(config.decision(Path::new("Screenshot01.png")).is_none());
    }
    #[test]
    fn collision_undo_and_manual_suppression() {
        let (_temp, mut engine) = fixture();
        let source = engine.config.root.join("photo.png");
        fs::write(&source, b"new").unwrap();
        let dest = engine.config.root.join("Картинки");
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("photo.png"), b"existing").unwrap();
        let id = ready(&mut engine);
        engine.move_one(&id, "Картинки").unwrap();
        assert_eq!(fs::read(dest.join("photo.png")).unwrap(), b"existing");
        assert_eq!(fs::read(dest.join("photo (2).png")).unwrap(), b"new");
        engine.undo(1).unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"new");
        engine.scan_at(Instant::now()).unwrap();
        assert_eq!(engine.queue[0]["destination"], "");
    }
    #[test]
    fn changed_source_cannot_be_moved_even_at_same_size() {
        let (_temp, mut engine) = fixture();
        let source = engine.config.root.join("a.png");
        fs::write(&source, b"123").unwrap();
        let id = ready(&mut engine);
        fs::write(&source, b"456").unwrap();
        assert!(engine.move_one(&id, "Картинки").is_err());
        assert_eq!(fs::read(source).unwrap(), b"456");
    }
    #[test]
    fn temporary_files_and_recent_changes_stay_put() {
        let (_temp, mut engine) = fixture();
        fs::write(engine.config.root.join("a.png.crdownload"), b"x").unwrap();
        fs::write(engine.config.root.join("b.png.part"), b"x").unwrap();
        let source = engine.config.root.join("c.png");
        fs::write(&source, b"x").unwrap();
        let now = Instant::now();
        engine.scan_at(now).unwrap();
        assert_eq!(engine.queue.len(), 1);
        assert_eq!(engine.queue[0]["ready"], false);
        fs::write(source, b"xx").unwrap();
        engine.scan_at(now + Duration::from_secs(61)).unwrap();
        assert_eq!(engine.queue[0]["ready"], false);
    }
    #[test]
    fn undo_refuses_occupied_source_or_modified_destination() {
        let (_temp, mut engine) = fixture();
        let source = engine.config.root.join("a.png");
        fs::write(&source, b"original").unwrap();
        let id = ready(&mut engine);
        engine.move_one(&id, "Картинки").unwrap();
        fs::write(&source, b"occupied").unwrap();
        assert!(engine.undo(1).is_err());
        fs::remove_file(&source).unwrap();
        fs::write(engine.config.root.join("Картинки/a.png"), b"changed").unwrap();
        assert!(engine.undo(1).is_err());
        assert!(!source.exists());
    }
    #[test]
    fn paths_cannot_escape_or_follow_symlinks() {
        let (_temp, engine) = fixture();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), engine.config.root.join("link")).unwrap();
        assert!(safe_destination(&engine.config.root, Path::new("../escape"), true).is_err());
        assert!(safe_destination(&engine.config.root, Path::new("link/child"), true).is_err());
        assert!(safe_destination(&engine.config.root, outside.path(), true).is_err());
    }
    #[test]
    fn interrupted_rename_is_reconciled_without_deleting_files() {
        let (_temp, mut engine) = fixture();
        let source = engine.config.root.join("a.png");
        fs::write(&source, b"data").unwrap();
        let snapshot = Snapshot::read(&source).unwrap();
        let target = safe_destination(&engine.config.root, Path::new("Картинки"), true)
            .unwrap()
            .join("a.png");
        engine.db.execute("INSERT INTO operations(source,destination,snapshot,state) VALUES(?1,?2,?3,'planned')", params![bytes(&source), bytes(&target), serde_json::to_string(&snapshot).unwrap()]).unwrap();
        rename_noreplace(&source, &target).unwrap();
        engine.recover().unwrap();
        assert_eq!(engine.history().unwrap()[0]["state"], "moved");
        engine.undo(1).unwrap();
        assert_eq!(fs::read(source).unwrap(), b"data");
    }
    #[test]
    fn rename_does_not_overwrite_even_if_destination_appears_late() {
        let temp = TempDir::new().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        fs::write(&a, b"source").unwrap();
        fs::write(&b, b"target").unwrap();
        assert!(rename_noreplace(&a, &b).is_err());
        assert_eq!(fs::read(a).unwrap(), b"source");
        assert_eq!(fs::read(b).unwrap(), b"target");
    }
    #[test]
    fn malformed_import_does_not_replace_settings() {
        let (temp, mut engine) = fixture();
        let previous = fs::read(&engine.config_path).unwrap();
        let import = temp.path().join("bad.json");
        fs::write(&import, b"{\"version\":999}").unwrap();
        let output = engine.command(&json!({"op":"import","path":import}).to_string());
        assert!(!serde_json::from_str::<Value>(&output).unwrap()["error"]
            .as_str()
            .unwrap()
            .is_empty());
        assert_eq!(fs::read(&engine.config_path).unwrap(), previous);
    }
    #[test]
    fn internal_settings_and_journal_cannot_be_sorted() {
        let (temp, mut engine) = fixture();
        let mut config = engine.config.clone();
        config.root = temp.path().join("state");
        assert!(engine.persist(config).is_err());
        let mut config = engine.config.clone();
        config.root = temp.path().join("config");
        assert!(engine.persist(config).is_err());
    }
    #[test]
    fn hidden_files_are_controlled_by_editable_exclusions() {
        let (_temp, mut engine) = fixture();
        fs::write(engine.config.root.join(".hidden.png"), b"data").unwrap();
        engine.scan_at(Instant::now()).unwrap();
        assert!(engine.queue.is_empty());
        engine.config.exclusions.clear();
        let token = ready(&mut engine);
        engine.move_one(&token, "Картинки").unwrap();
        assert!(engine.config.root.join("Картинки/.hidden.png").exists());
    }
    #[test]
    fn another_instance_cannot_open_the_same_journal() {
        let (temp, _engine) = fixture();
        assert!(Engine::open(&temp.path().join("config"), &temp.path().join("state")).is_err());
    }
    #[test]
    fn automatic_moves_only_finished_known_files() {
        let (_temp, mut engine) = fixture();
        engine.config.mode = "automatic".into();
        let root = engine.config.root.clone();
        fs::write(root.join("image.png"), b"image").unwrap();
        fs::write(root.join("unknown.iso"), b"archive").unwrap();
        fs::write(root.join("unfinished.png.part"), b"partial").unwrap();
        let now = Instant::now();
        engine.tick_at(now).unwrap();
        assert!(root.join("image.png").exists());
        engine.tick_at(now + Duration::from_secs(59)).unwrap();
        assert!(root.join("image.png").exists());
        engine.tick_at(now + Duration::from_secs(61)).unwrap();
        assert!(root.join("Картинки/image.png").exists());
        assert!(root.join("unknown.iso").exists());
        assert!(root.join("unfinished.png.part").exists());
        assert_eq!(engine.history().unwrap().len(), 1);
    }
    #[test]
    fn suggested_files_and_exclusions_are_not_automatic() {
        let (_temp, mut engine) = fixture();
        engine.config.mode = "automatic".into();
        engine.config.rules[0].action = "suggest".into();
        fs::write(engine.config.root.join("image.png"), b"image").unwrap();
        fs::write(engine.config.root.join(".hidden.txt"), b"hidden").unwrap();
        let now = Instant::now();
        engine.tick_at(now).unwrap();
        engine.tick_at(now + Duration::from_secs(61)).unwrap();
        assert!(engine.config.root.join("image.png").exists());
        assert!(engine.config.root.join(".hidden.txt").exists());
        assert!(engine.history().unwrap().is_empty());
        assert_eq!(engine.queue.len(), 1);
        assert_eq!(engine.queue[0]["action"], "suggest");
    }
    #[test]
    fn paused_mode_and_pause_guard_stop_automatic_moves() {
        let (_temp, mut engine) = fixture();
        fs::write(engine.config.root.join("image.png"), b"image").unwrap();
        engine.config.mode = "paused".into();
        let now = Instant::now();
        engine.tick_at(now).unwrap();
        engine.tick_at(now + Duration::from_secs(61)).unwrap();
        assert!(engine.config.root.join("image.png").exists());
        engine.config.mode = "automatic".into();
        engine.pause_guard.store(true, Ordering::Release);
        engine.tick_at(now + Duration::from_secs(62)).unwrap();
        assert!(engine.config.root.join("image.png").exists());
        engine.pause_guard.store(false, Ordering::Release);
        engine.tick_at(now + Duration::from_secs(63)).unwrap();
        assert!(engine.config.root.join("Картинки/image.png").exists());
    }
    #[test]
    fn restored_and_deferred_files_stay_manual_in_auto_mode() {
        let (_temp, mut engine) = fixture();
        let source = engine.config.root.join("image.png");
        fs::write(&source, b"image").unwrap();
        let id = ready(&mut engine);
        engine.move_one(&id, "Картинки").unwrap();
        engine.undo(1).unwrap();
        engine.config.mode = "automatic".into();
        let now = Instant::now();
        engine.tick_at(now).unwrap();
        engine.tick_at(now + Duration::from_secs(61)).unwrap();
        assert!(source.exists());
        let other = engine.config.root.join("other.png");
        fs::write(&other, b"other").unwrap();
        engine
            .db
            .execute(
                "INSERT INTO deferred(path) VALUES(?1)",
                params![bytes(&other)],
            )
            .unwrap();
        engine.tick_at(now + Duration::from_secs(62)).unwrap();
        engine.tick_at(now + Duration::from_secs(123)).unwrap();
        assert!(other.exists());
        assert!(source.exists());
    }
    #[test]
    fn automatic_failure_is_recorded_and_is_not_retried_silently() {
        let (_temp, mut engine) = fixture();
        let source = engine.config.root.join("image.png");
        fs::write(&source, b"old").unwrap();
        let _id = ready(&mut engine);
        fs::write(&source, b"changed").unwrap();
        engine.config.mode = "automatic".into();
        assert_eq!(engine.automatic_moves().unwrap(), 0);
        assert_eq!(engine.history().unwrap()[0]["state"], "error");
        assert_eq!(fs::read(&source).unwrap(), b"changed");
        let now = Instant::now();
        engine.tick_at(now).unwrap();
        engine.tick_at(now + Duration::from_secs(61)).unwrap();
        assert!(source.exists());
        assert_eq!(engine.history().unwrap().len(), 1);
        assert_eq!(engine.queue[0]["manual_only"], true);
    }
    #[test]
    fn preview_tokens_survive_background_scans_but_not_file_changes() {
        let (_temp, mut engine) = fixture();
        let source = engine.config.root.join("image.png");
        fs::write(&source, b"old").unwrap();
        let now = Instant::now();
        engine.scan_at(now).unwrap();
        let id = engine.queue[0]["id"].clone();
        engine.scan_at(now + Duration::from_secs(15)).unwrap();
        assert_eq!(engine.queue[0]["id"], id);
        fs::write(source, b"different").unwrap();
        engine.scan_at(now + Duration::from_secs(16)).unwrap();
        assert_ne!(engine.queue[0]["id"], id);
        assert_eq!(engine.queue[0]["ready"], false);
    }
    #[test]
    fn import_and_folder_changes_require_fresh_auto_confirmation() {
        let (temp, mut engine) = fixture();
        let mut imported = engine.config.clone();
        imported.mode = "automatic".into();
        let file = temp.path().join("import.json");
        fs::write(&file, serde_json::to_vec(&imported).unwrap()).unwrap();
        let response = engine.command(&json!({"op":"import","path":file}).to_string());
        assert_eq!(
            serde_json::from_str::<Value>(&response).unwrap()["error"],
            ""
        );
        assert_eq!(engine.config.mode, "manual");
        let other = temp.path().join("Other downloads");
        fs::create_dir(&other).unwrap();
        let mut config = engine.config.clone();
        config.root = other;
        config.mode = "automatic".into();
        engine.persist(config).unwrap();
        assert_eq!(engine.config.mode, "manual");
    }
    #[test]
    fn first_run_cannot_enable_automatic_mode() {
        let temp = TempDir::new().unwrap();
        let mut engine =
            Engine::open(&temp.path().join("config"), &temp.path().join("state")).unwrap();
        let response: Value = serde_json::from_str(
            &engine.command("{\"op\":\"set_mode\",\"mode\":\"automatic\",\"request_id\":42}"),
        )
        .unwrap();
        assert!(!response["error"].as_str().unwrap().is_empty());
        assert_eq!(response["request_id"], 42);
        assert_eq!(response["operation"], "set_mode");
        assert_eq!(engine.config.mode, "manual");
    }
    #[test]
    fn old_json_defaults_to_manual_and_system_theme() {
        let (_temp, engine) = fixture();
        let mut json = serde_json::to_value(&engine.config).unwrap();
        for key in ["mode", "theme", "autostart", "setup_complete"] {
            json.as_object_mut().unwrap().remove(key);
        }
        let config: Config = serde_json::from_value(json).unwrap();
        assert_eq!(config.mode, "manual");
        assert_eq!(config.theme, "system");
        assert!(!config.autostart);
        assert!(config.setup_complete);
    }
    #[test]
    fn theme_before_setup_does_not_start_watching_or_enable_automation() {
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("config");
        let state = temp.path().join("state");
        let mut engine = Engine::open(&config, &state).unwrap();
        let response: Value =
            serde_json::from_str(&engine.command("{\"op\":\"set_theme\",\"theme\":\"dark\"}"))
                .unwrap();
        assert_eq!(response["error"], "");
        assert_eq!(response["first_run"], true);
        drop(engine);
        let mut restarted = Engine::open(&config, &state).unwrap();
        assert_eq!(restarted.config.theme, "dark");
        assert!(restarted.first_run);
        assert_eq!(restarted.config.mode, "manual");
        let reply: Value = serde_json::from_str(
            &restarted.command("{\"op\":\"set_mode\",\"mode\":\"automatic\"}"),
        )
        .unwrap();
        assert!(!reply["error"].as_str().unwrap().is_empty());
    }
}
