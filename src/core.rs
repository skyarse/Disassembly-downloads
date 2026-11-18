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
mod media;
mod moves;
mod payload;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<u64>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub seasons: bool,
}
fn is_false(value: &bool) -> bool {
    !*value
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub path: PathBuf,
    #[serde(default)]
    pub automatic: bool,
    #[serde(default)]
    pub only_new: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub root: PathBuf,
    #[serde(default = "default_root_automatic")]
    pub root_automatic: bool,
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub only_new: bool,
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
fn default_root_automatic() -> bool {
    true
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
            root_automatic: true,
            sources: Vec::new(),
            only_new: false,
            root: dirs::download_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Downloads")),
            categories: vec![
                Category {
                    id: "pictures".into(),
                    name: "Картинки".into(),
                    folder: "Картинки".into(),
                    device: None,
                },
                Category {
                    id: "documents".into(),
                    name: "Документы".into(),
                    folder: "Документы".into(),
                    device: None,
                },
                Category {
                    id: "music".into(),
                    name: "Музыка".into(),
                    folder: "Музыка".into(),
                    device: None,
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
                    source: None,
                    seasons: false,
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
                    source: None,
                    seasons: false,
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
                    source: None,
                    seasons: false,
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
            self.root.is_absolute() && self.sources.iter().all(|source| source.path.is_absolute()),
            "Выберите существующую папку загрузок"
        );
        let mut roots = vec![self.root.as_path()];
        for source in &self.sources {
            ensure!(
                roots
                    .iter()
                    .all(|root| !source.path.starts_with(root) && !root.starts_with(&source.path)),
                "Папки наблюдения должны быть разными и не могут находиться одна внутри другой"
            );
            roots.push(&source.path);
        }
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
            let folder = Path::new(&category.folder);
            if folder.is_absolute() && category.device.is_some() {
                ensure!(
                    folder.parent().is_some(),
                    "Нельзя использовать корень диска как папку назначения"
                );
                ensure!(
                    self.watched_folders().all(|source| folder != source.path),
                    "Папка назначения совпадает с входящей"
                );
            } else {
                relative_destination(&self.root, folder)?;
            }
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
            ensure!(
                rule.source
                    .as_ref()
                    .is_none_or(|source| source.is_absolute()),
                "Некорректная папка правила"
            );
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
        let rule = self.rules.iter().find(|r| {
            r.active
                && r.source
                    .as_deref()
                    .is_none_or(|source| path.parent() == Some(source))
                && r.condition.matches(path)
        })?;
        let category = self.categories.iter().find(|c| c.id == rule.category_id)?;
        Some((rule, category))
    }
    fn watched_folders(&self) -> impl Iterator<Item = Source> + '_ {
        std::iter::once(Source {
            path: self.root.clone(),
            automatic: self.root_automatic,
            only_new: self.only_new,
        })
        .chain(self.sources.iter().cloned())
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
    #[serde(default, skip_serializing_if = "is_false")]
    directory: bool,
}
impl Snapshot {
    fn read(path: &Path) -> Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.is_dir() {
            return payload::directory_snapshot(path, &metadata);
        }
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
            directory: false,
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
        self.same_payload(other)
            && self.stamp.dev == other.stamp.dev
            && self.stamp.ino == other.stamp.ino
            && self.stamp.size == other.stamp.size
            && self.stamp.mtime == other.stamp.mtime
            && self.stamp.mtime_ns == other.stamp.mtime_ns
    }
    fn same_payload(&self, other: &Self) -> bool {
        self.directory == other.directory
            && self.hash == other.hash
            && (self.directory || self.stamp.size == other.stamp.size)
    }
}
#[derive(Clone)]
struct Pending {
    path: PathBuf,
    snapshot: Option<Snapshot>,
    companions: Vec<PathBuf>,
}
#[derive(Clone)]
struct PlannedEntry {
    source: PathBuf,
    destination: PathBuf,
    snapshot: Snapshot,
    device: u64,
}
struct MoveGroup {
    token: String,
    folder: String,
    entries: Vec<PlannedEntry>,
}
struct MovePlan {
    id: u64,
    groups: Vec<MoveGroup>,
    bundle: bool,
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
    source_errors: Vec<String>,
    directories: HashSet<PathBuf>,
    preview: Option<MovePlan>,
    duplicates: Vec<String>,
    first_run: bool,
}
fn bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_bytes()
}
fn file_identity(metadata: &fs::Metadata) -> String {
    format!("{}:{}", metadata.dev(), metadata.ino())
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
fn relative_destination<'a>(root: &Path, input: &'a Path) -> Result<&'a Path> {
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
    ensure!(
        relative
            .components()
            .all(|part| matches!(part, Component::Normal(_))),
        "Недопустимый путь назначения"
    );
    Ok(relative)
}
fn safe_destination(root: &Path, input: &Path, create: bool) -> Result<PathBuf> {
    let relative = relative_destination(root, input)?;
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
            CREATE TABLE IF NOT EXISTS deferred(path BLOB PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS existing_files(root BLOB NOT NULL, path BLOB NOT NULL, identity TEXT NOT NULL, PRIMARY KEY(root,path));
            CREATE TABLE IF NOT EXISTS batches(id INTEGER PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS batch_members(operation INTEGER PRIMARY KEY, batch INTEGER NOT NULL, group_head INTEGER NOT NULL, original_device TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS member_groups ON batch_members(group_head);
            CREATE INDEX IF NOT EXISTS member_batches ON batch_members(batch);
            CREATE TABLE IF NOT EXISTS copies(operation INTEGER PRIMARY KEY, direction TEXT NOT NULL, temporary BLOB NOT NULL, holding BLOB NOT NULL, fingerprint TEXT);")?;
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
            source_errors: Vec::new(),
            directories: HashSet::new(),
            preview: None,
            duplicates: Vec::new(),
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
        let previous = self.config.watched_folders().collect::<Vec<_>>();
        config.root = self.normalize_source(&config.root, &previous)?;
        for source in &mut config.sources {
            source.path = self.normalize_source(&source.path, &previous)?;
        }
        for category in &mut config.categories {
            let folder = Path::new(&category.folder);
            if folder.is_absolute()
                && !folder.starts_with(&self.config.root)
                && !folder.starts_with(&config.root)
            {
                let previous = self
                    .config
                    .categories
                    .iter()
                    .find(|old| old.id == category.id && old.folder == category.folder);
                category.device = match previous
                    .and_then(|old| old.device)
                    .filter(|_| category.device.is_some())
                {
                    Some(device) => Some(device),
                    None => Some(self.external_device(folder, None)?),
                };
                continue;
            }
            category.device = None;
            let base = if !folder.starts_with(&config.root) && folder.starts_with(&self.config.root)
            {
                &self.config.root
            } else {
                &config.root
            };
            category.folder = relative_destination(base, folder)?
                .to_string_lossy()
                .into_owned();
            if config.root.is_dir() {
                safe_destination(&config.root, Path::new(&category.folder), false)?;
            }
        }
        config.validate()?;
        if self.first_run || self.config.root != config.root {
            config.mode = manual_mode();
            if self.config.root != config.root {
                config.only_new = false;
            }
        }
        let mut needs_preview = config.categories.iter().any(|category| {
            category.device.is_some()
                && !self.config.categories.iter().any(|previous| {
                    previous.id == category.id
                        && previous.folder == category.folder
                        && previous.device == category.device
                })
        });
        for source in config.watched_folders() {
            let old = previous.iter().find(|old| old.path == source.path);
            if source.only_new && (self.first_run || !old.is_some_and(|old| old.only_new)) {
                self.record_existing_files(&source.path)?;
            }
            if old.is_some_and(|old| old.only_new && !source.only_new)
                || (source.automatic && !old.is_some_and(|old| old.automatic))
            {
                needs_preview = true;
            }
        }
        if needs_preview {
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
        self.preview = None;
        Ok(())
    }
    fn external_device(&self, path: &Path, expected: Option<u64>) -> Result<u64> {
        ensure!(
            path.is_absolute() && path.parent().is_some(),
            "Выберите папку назначения"
        );
        ensure!(
            path.canonicalize()
                .with_context(|| format!("Папка назначения недоступна: {}", path.display()))?
                == path,
            "Путь назначения содержит ссылку или изменился"
        );
        self.validate_watch_root(path)?;
        let metadata = fs::metadata(path)?;
        ensure!(metadata.is_dir(), "Назначение не является папкой");
        ensure!(
            expected.is_none_or(|device| device == metadata.dev()),
            "Диск назначения отключён или изменился"
        );
        Ok(metadata.dev())
    }
    fn resolve_destination(
        &self,
        root: &Path,
        input: &Path,
        create: bool,
    ) -> Result<(PathBuf, u64)> {
        ensure!(
            self.config
                .watched_folders()
                .all(|source| if input.is_absolute() {
                    input != source.path
                } else {
                    root.join(input) != source.path
                }),
            "Нельзя перемещать файлы прямо во входящую папку"
        );
        if !input.is_absolute() || input.starts_with(root) {
            return Ok((
                safe_destination(root, input, create)?,
                fs::metadata(root)?.dev(),
            ));
        }
        let anchor = self
            .config
            .categories
            .iter()
            .filter_map(|category| {
                let base = Path::new(&category.folder);
                category
                    .device
                    .filter(|_| input.starts_with(base))
                    .map(|device| (base, device))
            })
            .max_by_key(|(path, _)| path.components().count());
        if let Some((base, expected)) = anchor {
            let device = self.external_device(base, Some(expected))?;
            let output = if base == input {
                base.to_path_buf()
            } else {
                safe_destination(base, input, create)?
            };
            Ok((output, device))
        } else {
            Ok((input.to_path_buf(), self.external_device(input, None)?))
        }
    }
    fn normalize_source(&self, path: &Path, previous: &[Source]) -> Result<PathBuf> {
        ensure!(path.is_absolute(), "Выберите существующую папку загрузок");
        if !self.first_run && previous.iter().any(|source| source.path == path) {
            self.validate_watch_root(path)?;
            return Ok(path.to_path_buf());
        }
        let root = path.canonicalize().context("Папка наблюдения недоступна")?;
        ensure!(root.is_dir(), "Выберите существующую папку загрузок");
        self.validate_watch_root(&root)?;
        Ok(root)
    }
    fn source_entries(&self, root: &Path) -> Result<Vec<PathBuf>> {
        ensure!(root.canonicalize()? == root, "Путь к папке изменился");
        self.validate_watch_root(root)?;
        let mut paths = fs::read_dir(root)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.sort();
        Ok(paths)
    }
    fn record_existing_files(&mut self, root: &Path) -> Result<()> {
        let paths = self.source_entries(root)?;
        let transaction = self.db.transaction()?;
        transaction.execute("DELETE FROM existing_files WHERE root=?1", [bytes(root)])?;
        {
            let mut insert = transaction
                .prepare("INSERT INTO existing_files(root,path,identity) VALUES(?1,?2,?3)")?;
            for path in paths {
                let metadata = match fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                };
                if metadata.is_file() {
                    insert.execute(params![bytes(root), bytes(&path), file_identity(&metadata)])?;
                }
            }
        }
        transaction.commit()?;
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
        self.pending.clear();
        self.queue.clear();
        self.source_errors.clear();
        let mut seen = HashSet::new();
        for source in self.config.watched_folders().collect::<Vec<_>>() {
            if let Err(error) = self.scan_source(&source, now, &mut seen) {
                self.source_errors
                    .push(format!("{}: {error:#}", source.path.display()));
                self.queue
                    .retain(|row| row["source"].as_str() != source.path.to_str());
                self.pending
                    .retain(|_, file| file.path.parent() != Some(&source.path));
                seen.retain(|path| path.parent() != Some(&source.path));
            }
        }
        self.observed.retain(|path, _| seen.contains(path));
        self.snapshots.retain(|path, _| seen.contains(path));
        self.tokens.retain(|path, _| seen.contains(path));
        Ok(())
    }
    fn scan_source(
        &mut self,
        source: &Source,
        now: Instant,
        seen: &mut HashSet<PathBuf>,
    ) -> Result<()> {
        let paths = self.source_entries(&source.path)?;
        let subtitle_groups = self.subtitle_catalog(&paths);
        let start = self.queue.len();
        for path in paths {
            let meta = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let directory = meta.is_dir() && self.directories.contains(&path);
            if (!meta.is_file() && !directory) || temporary(&path) {
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
            let existing = source.only_new && self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM existing_files WHERE root=?1 AND path=?2 AND identity=?3)",
                params![bytes(&source.path), bytes(&path), file_identity(&meta)],
                |row| row.get::<_, bool>(0),
            )?;
            let decision = if restored {
                None
            } else {
                self.config.decision(&path)
            };
            let mut destination = decision
                .map(|(_, c)| {
                    if c.device.is_some() {
                        Ok(Path::new(&c.folder))
                    } else {
                        relative_destination(&self.config.root, Path::new(&c.folder))
                    }
                })
                .transpose()?
                .map(|folder| folder.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut action = decision.map(|(r, _)| r.action.clone()).unwrap_or_default();
            let mut season_missing = false;
            if decision.is_some_and(|(rule, _)| rule.seasons) && media::video(&path) {
                if let Some(season) = media::season(&path) {
                    destination = Path::new(&destination)
                        .join(format!("Сезон {season}"))
                        .to_string_lossy()
                        .into_owned();
                } else {
                    season_missing = true;
                    action = "suggest".into();
                }
            }
            let mut reason = if existing {
                "Файл был в папке до включения режима «Только новые»".into()
            } else if restored {
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
            if directory {
                reason = "Папка комплекта · требуется подтверждение".into();
                action = "suggest".into();
            }
            if season_missing {
                reason = "Сезон не найден в имени · требуется подтверждение".into();
            }
            let snapshot = if stable {
                match self.snapshot(&path, &Stamp::of(&meta)) {
                    Ok(value) => Some(value),
                    Err(error) => {
                        reason = format!("{error:#}");
                        None
                    }
                }
            } else {
                if !existing {
                    reason = "Ожидание: минута без изменений с момента обнаружения".into();
                }
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
            self.queue.push(json!({"id":token, "name":path.file_name().unwrap().to_string_lossy(), "path":path.to_string_lossy(), "source":source.path.to_string_lossy(), "size":meta.len(), "destination":destination, "reason":reason, "ready":snapshot.is_some(), "action":action, "directory": directory, "manual_only":restored || existing || directory, "automatic":source.automatic}));
            self.pending.insert(
                token,
                Pending {
                    path,
                    snapshot,
                    companions: Vec::new(),
                },
            );
        }
        self.attach_subtitles(start, subtitle_groups);
        Ok(())
    }
    fn subtitle_catalog(&self, paths: &[PathBuf]) -> media::Subtitles {
        let candidates = paths
            .iter()
            .filter(|path| {
                fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
                    && !self
                        .config
                        .exclusions
                        .iter()
                        .any(|condition| condition.matches(path))
            })
            .cloned()
            .collect::<Vec<_>>();
        media::subtitles(&candidates)
    }
    fn attach_subtitles(&mut self, start: usize, groups: media::Subtitles) {
        let indices: HashMap<PathBuf, usize> = self.queue[start..]
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                let path = self.pending.get(row["id"].as_str()?)?.path.clone();
                Some((path, start + index))
            })
            .collect();
        for path in &groups.ambiguous {
            if let Some(&index) = indices.get(path) {
                self.queue[index]["manual_only"] = json!(true);
                self.queue[index]["reason"] =
                    json!("Несколько одноимённых видео · проверьте субтитры вручную");
            }
        }
        let mut combined = HashSet::new();
        for (video, subtitles) in groups.files {
            let Some(&index) = indices.get(&video) else {
                continue;
            };
            let mut ready = self.queue[index]["ready"] == true;
            let mut manual = self.queue[index]["manual_only"] == true;
            let mut companions = Vec::new();
            let mut waiting_for_temp = false;
            for subtitle in subtitles {
                if !fs::symlink_metadata(&subtitle).is_ok_and(|meta| meta.is_file()) {
                    continue;
                }
                if temporary(&subtitle) {
                    ready = false;
                    waiting_for_temp = true;
                    continue;
                }
                if let Some(&sub_index) = indices.get(&subtitle) {
                    ready &= self.queue[sub_index]["ready"] == true;
                    manual |= self.queue[sub_index]["manual_only"] == true;
                    companions.push(subtitle);
                    if let Some(id) = self.queue[sub_index]["id"].as_str() {
                        combined.insert(id.to_owned());
                    }
                } else {
                    manual = true;
                    self.queue[index]["reason"] =
                        json!("Связанные субтитры отложены · требуется решение");
                }
            }
            let row = &mut self.queue[index];
            if waiting_for_temp || !companions.is_empty() {
                let description = if ready {
                    format!(
                        "{} · субтитров: {}",
                        row["reason"].as_str().unwrap_or_default(),
                        companions.len()
                    )
                } else {
                    "Ожидание готовности видео и субтитров".into()
                };
                row["reason"] = json!(description);
                row["ready"] = json!(ready);
                row["companions"] = json!(companions
                    .iter()
                    .map(|path| path.file_name().unwrap().to_string_lossy())
                    .collect::<Vec<_>>());
            }
            row["manual_only"] = json!(manual);
            let pending = self.pending.get_mut(row["id"].as_str().unwrap()).unwrap();
            pending.companions = companions;
            if !ready {
                pending.snapshot = None;
            }
        }
        self.queue
            .retain(|row| !row["id"].as_str().is_some_and(|id| combined.contains(id)));
        self.pending.retain(|id, _| !combined.contains(id));
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
                row["ready"] == true
                    && row["automatic"] == true
                    && row["action"] == "move"
                    && row["manual_only"] != true
            })
            .map(|row| {
                (
                    row["id"].as_str().unwrap().to_string(),
                    row["destination"].as_str().unwrap().to_string(),
                )
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            return Ok(0);
        }
        let batch = self.new_batch()?;
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
            let held = pending.companions.clone();
            let last_id = self.last_operation_id()?;
            match self.move_one_in_batch(&token, &folder, batch) {
                Ok(()) => moved += 1,
                Err(error) => {
                    self.pending.remove(&token);
                    if self.last_operation_id()? == last_id {
                        let target = source
                            .parent()
                            .unwrap()
                            .join(&folder)
                            .join(source.file_name().unwrap());
                        self.db.execute("INSERT INTO operations(source,destination,snapshot,state,message) VALUES(?1,?2,?3,'error',?4)",
                            params![bytes(&source), bytes(&target), serde_json::to_string(&snapshot)?, format!("{error:#}")])?;
                    }
                    for path in std::iter::once(&source).chain(held.iter()) {
                        self.db.execute(
                            "INSERT OR IGNORE INTO manual_only(path) VALUES(?1)",
                            [bytes(path)],
                        )?;
                    }
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
    #[cfg(test)]
    fn move_one(&mut self, token: &str, folder: &str) -> Result<()> {
        let batch = self.new_batch()?;
        self.move_one_in_batch(token, folder, batch)
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
        let original_device = self.db.query_row(
            "SELECT original_device FROM batch_members WHERE operation=?1",
            [id],
            |row| row.get::<_, String>(0),
        );
        let device = match original_device {
            Ok(device) => device.parse::<u64>()?,
            Err(rusqlite::Error::QueryReturnedNoRows) => snapshot.stamp.dev,
            Err(error) => return Err(error.into()),
        };
        let original_parent = source.parent().context("Нет исходной папки")?;
        ensure!(
            original_parent.canonicalize()? == original_parent
                && fs::metadata(original_parent)?.dev() == device,
            "Диск исходной папки отключён или изменился"
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
            self.transfer(id, "undo", &destination, &source, &snapshot, device)?;
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
            let direction = if state == "planned" { "move" } else { "undo" };
            if let Some((done, current, message)) =
                self.recover_copy(id, direction, old, new, &snapshot)?
            {
                let updated = if done { completed } else { "needs_review" };
                self.db.execute(
                    "UPDATE operations SET state=?2,message=?3,snapshot=?4 WHERE id=?1",
                    params![
                        id,
                        updated,
                        message,
                        serde_json::to_string(&current.unwrap_or(snapshot))?
                    ],
                )?;
                if !done && direction == "move" {
                    self.db.execute(
                        "INSERT OR IGNORE INTO manual_only(path) VALUES(?1)",
                        [bytes(old)],
                    )?;
                }
                continue;
            }
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
            if state == "planned" && new_state != "moved" {
                self.db.execute(
                    "INSERT OR IGNORE INTO manual_only(path) VALUES(?1)",
                    [bytes(old)],
                )?;
            }
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
        Ok(self.db.prepare("SELECT o.id,o.created,o.source,o.destination,o.state,o.message,b.batch,b.group_head,(SELECT count(*) FROM batch_members WHERE group_head=b.group_head) FROM operations o LEFT JOIN batch_members b ON o.id=b.operation ORDER BY o.id DESC LIMIT 500")?.query_map([], |r| {
            let source = from_bytes(r.get(2)?);
            let destination = from_bytes(r.get(3)?);
            Ok(json!({"id":r.get::<_,i64>(0)?, "created":r.get::<_,String>(1)?, "source":source.to_string_lossy(), "destination":destination.to_string_lossy(), "state":r.get::<_,String>(4)?, "message":r.get::<_,String>(5)?,"batch":r.get::<_,Option<i64>>(6)?,"group":r.get::<_,Option<i64>>(7)?,"group_size":r.get::<_,i64>(8)?}))
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
                let scope: Option<PathBuf> = serde_json::from_value(request["source"].clone())?;
                let multiple = !self.config.sources.is_empty();
                let mut unavailable = Vec::new();
                for source in self
                    .config
                    .watched_folders()
                    .filter(|source| scope.as_ref().is_none_or(|path| path == &source.path))
                {
                    let paths = match self.source_entries(&source.path) {
                        Ok(paths) => paths,
                        Err(_) => {
                            unavailable.push(source.path.display().to_string());
                            continue;
                        }
                    };
                    for path in paths {
                        if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file())
                            && !temporary(&path)
                            && condition.matches(&path)
                        {
                            let display = if multiple {
                                path.as_path().as_os_str()
                            } else {
                                path.file_name().unwrap()
                            };
                            self.rule_test.push(display.to_string_lossy().into_owned());
                        }
                    }
                }
                ensure!(
                    unavailable.is_empty(),
                    "Предпросмотр недоступен для папок: {}",
                    unavailable.join(", ")
                );
                self.message = format!("Условию соответствуют файлов: {}. Исключения и правила выше могут иметь приоритет.", self.rule_test.len());
            }
            "plan_move" => {
                let items = request["items"].as_array().context("Не выбраны файлы")?;
                ensure!(!items.is_empty(), "Не выбраны файлы");
                self.preview = None;
                let bundle = request["bundle"] == true;
                let mut reserved = HashSet::new();
                let mut groups = Vec::new();
                for item in items {
                    let token = item["id"].as_str().context("Нет ID")?;
                    let folder = item["folder"].as_str().context("Нет назначения")?;
                    match self.plan_group(token, folder, &mut reserved, bundle) {
                        Ok(group) => groups.push(group),
                        Err(error) => {
                            if let Some(pending) = self.pending.get(token) {
                                if let Some(snapshot) = &pending.snapshot {
                                    let destination = pending
                                        .path
                                        .parent()
                                        .unwrap()
                                        .join(folder)
                                        .join(pending.path.file_name().unwrap());
                                    self.record_move_error(
                                        &pending.path,
                                        &destination,
                                        snapshot,
                                        &error,
                                    )?;
                                }
                            }
                            return Err(error);
                        }
                    }
                }
                let entries = groups
                    .iter()
                    .flat_map(|group| &group.entries)
                    .collect::<Vec<_>>();
                ensure!(
                    entries
                        .iter()
                        .filter(|item| item.snapshot.directory)
                        .all(|folder| entries
                            .iter()
                            .all(|item| !item.destination.starts_with(&folder.source))),
                    "Назначение находится внутри папки, выбранной для переноса"
                );
                if bundle {
                    let first = &groups[0].entries[0];
                    ensure!(
                        groups
                            .iter()
                            .flat_map(|group| &group.entries)
                            .all(|item| item.source.parent() == first.source.parent()
                                && item.destination.parent() == first.destination.parent()),
                        "Для комплекта выберите файлы из одной папки и общее назначение"
                    );
                }
                self.sequence += 1;
                self.preview = Some(MovePlan {
                    id: self.sequence,
                    groups,
                    bundle,
                });
            }
            "apply_move" => {
                let plan = self
                    .preview
                    .take()
                    .context("Предпросмотр устарел. Проверьте пути заново")?;
                ensure!(request["id"] == plan.id, "Предпросмотр устарел");
                let batch = self.new_batch()?;
                let mut failures = Vec::new();
                let mut moved = 0;
                let groups = if plan.bundle {
                    vec![plan.groups.as_slice()]
                } else {
                    plan.groups.chunks(1).collect()
                };
                for group in groups {
                    let last_id = self.last_operation_id()?;
                    match self.execute_groups(group, batch) {
                        Ok(()) => {
                            moved += group.iter().map(|item| item.entries.len()).sum::<usize>()
                        }
                        Err(error) => {
                            if self.last_operation_id()? == last_id {
                                let item = &group[0].entries[0];
                                self.record_move_error(
                                    &item.source,
                                    &item.destination,
                                    &item.snapshot,
                                    &error,
                                )?;
                            }
                            failures.push(format!("{error:#}"));
                        }
                    }
                }
                self.scan_at(Instant::now())?;
                if !failures.is_empty() {
                    bail!("Перемещено: {moved}. {}", failures.join("; "));
                }
                self.message = format!("Перемещено: {moved}");
            }
            "undo" | "undo_batch" => {
                let id = request["id"].as_i64().context("Нет операции")?;
                let is_batch = request["op"] == "undo_batch";
                let (restored, errors) = self.undo_selection(id, is_batch)?;
                self.scan_at(Instant::now())?;
                if !errors.is_empty() {
                    bail!("Восстановлено: {restored}. {}", errors.join("; "));
                }
                self.message = format!("Восстановлено: {restored}. Файлы ждут ручного решения");
            }
            "add_directory" => {
                let path = PathBuf::from(request["path"].as_str().context("Нет папки комплекта")?);
                ensure!(
                    path.is_absolute() && fs::symlink_metadata(&path)?.is_dir(),
                    "Выберите обычную папку комплекта"
                );
                let root = path.parent().context("Нет исходной папки")?;
                ensure!(
                    self.config
                        .watched_folders()
                        .any(|source| source.path == root),
                    "Комплект должен находиться прямо во входящей папке"
                );
                ensure!(
                    !self
                        .config
                        .exclusions
                        .iter()
                        .any(|rule| rule.matches(&path)),
                    "Папка исключена правилами"
                );
                self.directories.insert(path);
                self.scan_at(Instant::now())?;
                self.message = "Папка комплекта добавлена в очередь".into();
            }
            "find_duplicates" => {
                self.duplicates.clear();
                let token = request["id"].as_str().context("Нет ID файла")?;
                let root = Path::new(request["path"].as_str().context("Выберите папку")?);
                self.duplicates = self.find_duplicates(token, root)?;
            }
            "defer" => {
                let token = request["id"].as_str().context("Нет ID")?;
                let item = self.pending.get(token).context("Предпросмотр устарел")?;
                for path in std::iter::once(&item.path).chain(item.companions.iter()) {
                    self.db.execute(
                        "INSERT OR IGNORE INTO deferred(path) VALUES(?1)",
                        [bytes(path)],
                    )?;
                }
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
                config.only_new = false;
                for source in &mut config.sources {
                    source.only_new = false;
                }
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
        self.duplicates.clear();
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
        json!({"config":self.config,"first_run":self.first_run,"queue":self.queue,"history":self.history().unwrap_or_default(),"source_errors": self.source_errors,"plan":self.plan_view(),"duplicates":self.duplicates,"message":self.message,"error":error,"matches":self.rule_test,"request_id":request_id,"operation":operation}).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn command(engine: &mut Engine, request: Value) -> Value {
        serde_json::from_str(&engine.command(&request.to_string())).unwrap()
    }

    #[test]
    fn series_waits_for_subtitles_and_keeps_their_name_after_collision() {
        let (_temp, mut engine) = fixture();
        let root = engine.config.root.clone();
        engine.config.mode = "automatic".into();
        engine.config.rules[0].condition.extensions = vec!["mkv".into()];
        engine.config.rules[0].condition.pattern = "Show*".into();
        engine.config.rules[0].seasons = true;
        let video = "Show.S02E03.mkv";
        let subtitle = "Show.S02E03.ru.ass";
        let partial = root.join(format!("{subtitle}.part"));
        fs::write(root.join(video), b"video").unwrap();
        fs::write(&partial, b"subtitle").unwrap();
        let target = root.join("Картинки/Сезон 02");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join(video), b"previous video").unwrap();
        let now = Instant::now();
        engine.tick_at(now).unwrap();
        engine.tick_at(now + Duration::from_secs(61)).unwrap();
        assert!(root.join(video).is_file());
        fs::rename(partial, root.join(subtitle)).unwrap();
        engine.tick_at(now + Duration::from_secs(62)).unwrap();
        assert!(root.join(video).is_file());
        engine.tick_at(now + Duration::from_secs(122)).unwrap();
        assert_eq!(
            fs::read(target.join("Show.S02E03 (2).mkv")).unwrap(),
            b"video"
        );
        assert_eq!(
            fs::read(target.join("Show.S02E03 (2).ru.ass")).unwrap(),
            b"subtitle"
        );
        assert_eq!(fs::read(target.join(video)).unwrap(), b"previous video");
        let history = engine.history().unwrap();
        assert_eq!(history[0]["group"], history[1]["group"]);
        assert_eq!(
            engine
                .undo_selection(history[0]["id"].as_i64().unwrap(), false)
                .unwrap()
                .0,
            2
        );
        assert!(root.join(video).is_file() && root.join(subtitle).is_file());
    }

    #[test]
    fn exact_preview_refuses_a_later_collision_and_batch_undo_restores_files() {
        let (_temp, mut engine) = fixture();
        let root = engine.config.root.clone();
        let folder = root.join("Картинки");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("a.png"), b"existing").unwrap();
        fs::write(root.join("a.png"), b"move a").unwrap();
        fs::write(root.join("b.png"), b"move b").unwrap();
        let _ = ready(&mut engine);
        let selections = engine
            .queue
            .iter()
            .map(|row| json!({"id":row["id"],"folder":"Картинки"}))
            .collect::<Vec<_>>();
        let response = command(&mut engine, json!({"op":"plan_move", "items":&selections}));
        let plan = &response["plan"];
        assert_eq!(
            plan["entries"][0]["destination"],
            json!(folder.join("a (2).png"))
        );
        fs::write(folder.join("a (2).png"), b"late collision").unwrap();
        let response = command(&mut engine, json!({"op":"apply_move", "id":plan["id"]}));
        assert!(!response["error"].as_str().unwrap().is_empty());
        assert!(root.join("a.png").exists());
        assert_eq!(
            fs::read(folder.join("a (2).png")).unwrap(),
            b"late collision"
        );
        assert!(folder.join("b.png").exists());
        let response = command(
            &mut engine,
            json!({"op":"undo_batch","id":response["history"][0]["batch"]}),
        );
        assert_eq!(response["error"], "");
        assert!(root.join("b.png").exists());
    }

    #[test]
    fn external_destination_preserves_source_when_missing_and_moves_between_devices() {
        let (_temp, mut engine) = fixture();
        let other = TempDir::new_in("/dev/shm").unwrap();
        let folder = other.path().join("archive");
        fs::create_dir(&folder).unwrap();
        let root = engine.config.root.clone();
        assert_ne!(
            fs::metadata(&root).unwrap().dev(),
            fs::metadata(&folder).unwrap().dev()
        );
        let mut config = engine.config.clone();
        config.categories[0].folder = folder.to_string_lossy().into_owned();
        engine.persist(config).unwrap();
        let source = root.join("image.png");
        fs::write(&source, b"verified payload").unwrap();
        let token = ready(&mut engine);
        let selected = json!([{"id":token,"folder":folder}]);
        let plan = command(&mut engine, json!({"op":"plan_move","items":selected}))["plan"].clone();
        assert_eq!(plan["entries"][0]["copy"], true);
        let disconnected = other.path().join("disconnected");
        fs::rename(&folder, &disconnected).unwrap();
        let stopped = command(&mut engine, json!({"op":"apply_move","id":plan["id"]}));
        assert!(!stopped["error"].as_str().unwrap().is_empty());
        assert_eq!(fs::read(&source).unwrap(), b"verified payload");
        assert!(!folder.exists());
        fs::rename(disconnected, &folder).unwrap();
        let token = ready(&mut engine);
        engine.move_one(&token, folder.to_str().unwrap()).unwrap();
        assert!(!source.exists());
        assert_eq!(
            fs::read(folder.join("image.png")).unwrap(),
            b"verified payload"
        );
        let id = engine.history().unwrap()[0]["id"].as_i64().unwrap();
        engine.undo(id).unwrap();
        assert_eq!(fs::read(source).unwrap(), b"verified payload");
        assert!(!folder.join("image.png").exists());

        let game = root.join("Game");
        fs::create_dir_all(game.join("resources")).unwrap();
        fs::write(game.join("resources/assets.bin"), b"game payload").unwrap();
        assert_eq!(
            command(&mut engine, json!({"op":"add_directory","path":game}))["error"],
            ""
        );
        let _ = ready(&mut engine);
        let token = engine
            .queue
            .iter()
            .find(|row| row["directory"] == true)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        engine.move_one(&token, folder.to_str().unwrap()).unwrap();
        let id = engine.history().unwrap()[0]["id"].as_i64().unwrap();
        assert_eq!(
            fs::read(folder.join("Game/resources/assets.bin")).unwrap(),
            b"game payload"
        );
        assert!(!game.exists());
        engine.undo(id).unwrap();
        assert_eq!(
            fs::read(game.join("resources/assets.bin")).unwrap(),
            b"game payload"
        );
        assert!(!folder.join("Game").exists());
    }

    #[test]
    fn interrupted_copy_keeps_source_and_records_the_partial_path() {
        let (_temp, mut engine) = fixture();
        let root = engine.config.root.clone();
        let source = root.join("a.png");
        fs::write(&source, b"original").unwrap();
        let target = root.join("Картинки/a.png");
        let temporary = root.join(".stage.part");
        fs::write(&temporary, b"incomplete").unwrap();
        let snapshot = serde_json::to_string(&Snapshot::read(&source).unwrap()).unwrap();
        engine.db.execute(
            "INSERT INTO operations(source,destination,snapshot,state) VALUES(?1,?2,?3,'planned')",
            params![bytes(&source), bytes(&target), snapshot],
        ).unwrap();
        engine
            .db
            .execute(
                "INSERT INTO copies(operation,direction,temporary,holding) VALUES(1,'move',?1,?2)",
                params![bytes(&temporary), bytes(&root.join(".source.part"))],
            )
            .unwrap();
        engine.recover().unwrap();
        let history = engine.history().unwrap();
        assert_eq!(history[0]["state"], "needs_review");
        assert!(history[0]["message"]
            .as_str()
            .unwrap()
            .contains(".stage.part"));
        assert_eq!(fs::read(source).unwrap(), b"original");
        assert!(temporary.exists() && !target.exists());
    }
    #[test]
    fn interrupted_write_preserves_source() {
        use std::os::unix::process::CommandExt;
        if let Some(source) = std::env::var_os("TRIAGE_TEST_LIMITED_SOURCE") {
            let source = PathBuf::from(source);
            let temporary = PathBuf::from(std::env::var_os("TRIAGE_TEST_LIMITED_TARGET").unwrap());
            let expected = Snapshot::read(&source).unwrap();
            let error = payload::stage(&source, &temporary, &expected).unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(libc::EFBIG)
            );
            expected.verify(&source).unwrap();
            assert!(!temporary.exists());
            return;
        }
        let source_dir = TempDir::new().unwrap();
        let target_dir = TempDir::new_in("/dev/shm").unwrap();
        let source = source_dir.path().join("installer.bin");
        fs::write(&source, vec![7u8; 256 * 1024]).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", "core::tests::interrupted_write_preserves_source"])
            .env("TRIAGE_TEST_LIMITED_SOURCE", &source)
            .env(
                "TRIAGE_TEST_LIMITED_TARGET",
                target_dir.path().join(".partial"),
            );
        unsafe {
            child.pre_exec(|| {
                libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
                let limit = libc::rlimit {
                    rlim_cur: 64 * 1024,
                    rlim_max: 64 * 1024,
                };
                if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
        let output = child.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}"
        );
        assert_eq!(fs::read(&source).unwrap(), vec![7u8; 256 * 1024]);
    }

    #[test]
    fn game_folder_and_duplicate_search_preserve_content() {
        let (temp, mut engine) = fixture();
        let root = engine.config.root.clone();
        let game = root.join("Game.v2");
        fs::create_dir_all(game.join("data")).unwrap();
        fs::write(game.join("data/level.bin"), b"assets").unwrap();
        assert_eq!(
            command(&mut engine, json!({"op":"add_directory","path":game}))["error"],
            ""
        );
        let id = ready(&mut engine);
        engine.move_one(&id, "Games").unwrap();
        assert_eq!(
            fs::read(root.join("Games/Game.v2/data/level.bin")).unwrap(),
            b"assets"
        );
        assert!(!game.exists());
        engine.undo(1).unwrap();
        assert!(game.join("data/level.bin").exists());
        fs::write(root.join("installer.bin"), b"same data").unwrap();
        fs::write(temp.path().join("renamed.bin"), b"same data").unwrap();
        fs::write(temp.path().join("different.bin"), b"some data").unwrap();
        let folder = temp.path().join("archive");
        fs::create_dir(&folder).unwrap();
        fs::rename(temp.path().join("renamed.bin"), folder.join("renamed.bin")).unwrap();
        fs::rename(
            temp.path().join("different.bin"),
            folder.join("different.bin"),
        )
        .unwrap();
        let _ = ready(&mut engine);
        let id = engine
            .queue
            .iter()
            .find(|row| row["name"] == "installer.bin")
            .unwrap()["id"]
            .clone();
        let response = command(
            &mut engine,
            json!({"op":"find_duplicates","id":id,"path":folder}),
        );
        assert_eq!(response["error"], "");
        assert_eq!(response["duplicates"], json!([folder.join("renamed.bin")]));
        assert!(root.join("installer.bin").exists());

        fs::write(root.join("installer.exe"), b"installer file").unwrap();
        let _ = ready(&mut engine);
        let selections = engine
            .queue
            .iter()
            .filter(|row| row["name"].as_str().unwrap().starts_with("installer."))
            .map(|row| json!({"id": row["id"], "folder": "Installers"}))
            .collect::<Vec<_>>();
        let preview = command(
            &mut engine,
            json!({"op":"plan_move","items":selections,"bundle":true}),
        );
        assert_eq!(preview["error"], "");
        assert_eq!(preview["plan"]["entries"].as_array().unwrap().len(), 2);
        let applied = command(
            &mut engine,
            json!({"op":"apply_move","id":preview["plan"]["id"]}),
        );
        assert_eq!(applied["error"], "");
        assert_eq!(
            applied["history"][0]["group"],
            applied["history"][1]["group"]
        );
        assert!(
            root.join("Installers/installer.exe").exists()
                && root.join("Installers/installer.bin").exists()
        );
    }
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
    fn source_scoped_rules_and_manual_folders_keep_destinations_separate() {
        let (temp, mut engine) = fixture();
        let root = engine.config.root.clone();
        let extra = temp.path().join("Screenshots");
        fs::create_dir(&extra).unwrap();
        fs::write(root.join("test.png"), b"from Downloads").unwrap();
        fs::write(extra.join("test.png"), b"from Screenshots").unwrap();
        let mut config = engine.config.clone();
        config.mode = "automatic".into();
        config.sources.push(Source {
            path: extra.clone(),
            automatic: false,
            only_new: false,
        });
        config.categories.push(Category {
            id: "screenshots".into(),
            name: "Скриншоты".into(),
            folder: "Скриншоты".into(),
            device: None,
        });
        let mut scoped = config.rules[0].clone();
        scoped.id = "scoped".into();
        scoped.source = Some(extra.clone());
        scoped.category_id = "screenshots".into();
        config.rules.insert(0, scoped);
        engine.persist(config).unwrap();
        let response: Value = serde_json::from_str(&engine.command(&json!({
            "op": "test", "source": extra, "condition": {"extensions": ["png"], "pattern": ""}
        }).to_string())).unwrap();
        assert_eq!(response["matches"], json!([extra.join("test.png")]));
        let now = Instant::now();
        engine.tick_at(now).unwrap();
        engine.tick_at(now + Duration::from_secs(60)).unwrap();
        assert!(root.join("Картинки/test.png").exists());
        assert!(extra.join("test.png").exists());
        assert_eq!(engine.queue[0]["destination"], "Скриншоты");
        let mut config = engine.config.clone();
        config.sources[0].automatic = true;
        engine.persist(config).unwrap();
        assert_eq!(engine.config.mode, "manual");
        engine.config.mode = "automatic".into();
        engine.tick_at(now + Duration::from_secs(61)).unwrap();
        assert_eq!(
            fs::read(extra.join("Скриншоты/test.png")).unwrap(),
            b"from Screenshots"
        );
        assert!(!root.join("Скриншоты").exists());
    }
    #[test]
    fn backlog_protection_survives_restart_and_disabling_requests_preview() {
        let (temp, mut engine) = fixture();
        let root = engine.config.root.clone();
        fs::write(root.join("existing.png"), b"keep").unwrap();
        let mut config = engine.config.clone();
        config.only_new = true;
        config.mode = "automatic".into();
        engine.persist(config).unwrap();
        drop(engine);
        fs::write(root.join("new.png"), b"move").unwrap();
        let mut engine =
            Engine::open(&temp.path().join("config"), &temp.path().join("state")).unwrap();
        let now = Instant::now();
        engine.tick_at(now).unwrap();
        engine.tick_at(now + Duration::from_secs(60)).unwrap();
        assert!(root.join("Картинки/new.png").exists());
        assert!(root.join("existing.png").exists());
        assert_eq!(engine.queue[0]["manual_only"], true);
        let mut config = engine.config.clone();
        config.only_new = false;
        engine.persist(config).unwrap();
        assert_eq!(engine.config.mode, "manual");
        engine.tick_at(now + Duration::from_secs(61)).unwrap();
        assert!(root.join("existing.png").exists());
    }
    #[test]
    fn unavailable_source_does_not_interrupt_other_folders() {
        let (temp, mut engine) = fixture();
        let extra = temp.path().join("Offline");
        fs::create_dir(&extra).unwrap();
        let mut config = engine.config.clone();
        config.sources.push(Source {
            path: extra.clone(),
            automatic: false,
            only_new: false,
        });
        engine.persist(config).unwrap();
        fs::remove_dir(extra).unwrap();
        let root = engine.config.root.clone();
        fs::write(root.join("new.png"), b"move").unwrap();
        engine.config.mode = "automatic".into();
        let now = Instant::now();
        engine.tick_at(now).unwrap();
        engine.tick_at(now + Duration::from_secs(60)).unwrap();
        assert!(root.join("Картинки/new.png").exists());
        assert_eq!(engine.source_errors.len(), 1);
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
