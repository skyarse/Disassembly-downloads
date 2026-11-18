use super::*;

fn numbered_name(source: &Path, primary: Option<&Path>, number: usize) -> OsString {
    let filename = source.file_name().unwrap();
    if number == 1 {
        return filename.to_os_string();
    }
    if let Some(primary) = primary {
        let stem = primary.file_stem().unwrap().to_string_lossy();
        let name = filename.to_string_lossy();
        let index = name
            .char_indices()
            .nth(stem.chars().count())
            .map_or(name.len(), |(index, _)| index);
        return format!("{} ({number}){}", &name[..index], &name[index..]).into();
    }
    if source.is_dir() {
        let mut name = filename.to_os_string();
        name.push(format!(" ({number})"));
        return name;
    }
    let mut name = source.file_stem().unwrap_or(filename).to_os_string();
    name.push(format!(" ({number})"));
    if let Some(extension) = source.extension() {
        name.push(".");
        name.push(extension);
    }
    name
}

impl Engine {
    fn registered_root<'a>(&self, path: &'a Path) -> Result<&'a Path> {
        let root = path.parent().context("Нет исходной папки")?;
        ensure!(
            self.config
                .watched_folders()
                .any(|source| source.path == root)
                && root.canonicalize()? == root,
            "Исходная папка изменилась или больше не отслеживается"
        );
        Ok(root)
    }

    pub(super) fn plan_group(
        &self,
        token: &str,
        folder: &str,
        reserved: &mut HashSet<PathBuf>,
        bundle: bool,
    ) -> Result<MoveGroup> {
        let pending = self
            .pending
            .get(token)
            .context("Предпросмотр устарел. Обновите очередь")?;
        let snapshot = pending
            .snapshot
            .as_ref()
            .context("Файл ещё не готов к перемещению")?;
        let root = self.registered_root(&pending.path)?;
        let (directory, device) = self.resolve_destination(root, Path::new(folder), false)?;
        ensure!(
            !directory.starts_with(&pending.path),
            "Нельзя переместить комплект внутрь него самого"
        );
        let sources = std::iter::once((pending.path.as_path(), snapshot))
            .chain(
                pending
                    .companions
                    .iter()
                    .map(|path| (path.as_path(), &self.snapshots[path])),
            )
            .collect::<Vec<_>>();
        let primary = (!pending.companions.is_empty()).then_some(pending.path.as_path());
        let targets = (1..=10000)
            .find_map(|number| {
                let targets = sources
                    .iter()
                    .map(|(source, _)| directory.join(numbered_name(source, primary, number)))
                    .collect::<Vec<_>>();
                (targets.iter().collect::<HashSet<_>>().len() == targets.len()
                    && targets
                        .iter()
                        .all(|target| !present(target) && !reserved.contains(target)))
                .then_some(targets)
            })
            .context("Не удалось выбрать свободные имена назначения")?;
        if bundle {
            ensure!(
                sources
                    .iter()
                    .zip(&targets)
                    .all(|((source, _), target)| source.file_name() == target.file_name()),
                "В папке уже есть файл с таким именем. Выберите пустую папку для комплекта"
            );
        }
        reserved.extend(targets.iter().cloned());
        let entries = sources
            .into_iter()
            .zip(targets)
            .map(|((source, snapshot), destination)| PlannedEntry {
                source: source.to_path_buf(),
                destination,
                snapshot: snapshot.clone(),
                device,
            })
            .collect();
        Ok(MoveGroup {
            token: token.into(),
            folder: folder.into(),
            entries,
        })
    }

    pub(super) fn new_batch(&self) -> Result<i64> {
        self.db.execute("INSERT INTO batches DEFAULT VALUES", [])?;
        Ok(self.db.last_insert_rowid())
    }
    pub(super) fn record_move_error(
        &self,
        source: &Path,
        destination: &Path,
        snapshot: &Snapshot,
        error: &anyhow::Error,
    ) -> Result<()> {
        self.db.execute(
            "INSERT INTO operations(source,destination,snapshot,state,message) VALUES(?1,?2,?3,'error',?4)",
            params![bytes(source), bytes(destination), serde_json::to_string(snapshot)?, format!("{error:#}")],
        )?;
        Ok(())
    }
    pub(super) fn plan_view(&self) -> Value {
        self.preview.as_ref().map_or(Value::Null, |plan| {
            let entries = plan.groups.iter().flat_map(|group| &group.entries).map(|entry| {
                json!({"source": entry.source.to_string_lossy(), "destination": entry.destination.to_string_lossy(),
                    "copy": entry.device != entry.snapshot.stamp.dev})
            }).collect::<Vec<_>>();
            json!({"id": plan.id, "entries": entries, "bundle": plan.bundle})
        })
    }

    pub(super) fn undo_selection(&mut self, id: i64, batch: bool) -> Result<(usize, Vec<String>)> {
        let mut groups = Vec::new();
        if batch {
            let mut query = self.db.prepare(
                "SELECT DISTINCT group_head FROM batch_members WHERE batch=?1 ORDER BY group_head DESC",
            )?;
            groups.extend(
                query
                    .query_map([id], |row| row.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            );
        } else {
            let group = self.db.query_row(
                "SELECT group_head FROM batch_members WHERE operation=?1",
                [id],
                |row| row.get::<_, i64>(0),
            );
            match group {
                Ok(group) => groups.push(group),
                Err(rusqlite::Error::QueryReturnedNoRows) => {
                    self.undo(id)?;
                    return Ok((1, Vec::new()));
                }
                Err(error) => return Err(error.into()),
            }
        }
        let mut restored = 0;
        let mut errors = Vec::new();
        for group in groups {
            let rows = self.db.prepare(
                "SELECT o.id,o.source,o.destination,o.snapshot FROM operations o JOIN batch_members b ON b.operation=o.id
                 WHERE b.group_head=?1 AND o.state='moved' ORDER BY o.id DESC",
            )?.query_map([group], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,Vec<u8>>(1)?,row.get::<_,Vec<u8>>(2)?,row.get::<_,String>(3)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let preflight = (|| -> Result<()> {
                for (_, source, destination, snapshot) in &rows {
                    let source = from_bytes(source.clone());
                    ensure!(
                        !present(&source),
                        "Исходный путь уже занят: {}",
                        source.display()
                    );
                    let target = from_bytes(destination.clone());
                    serde_json::from_str::<Snapshot>(snapshot)?
                        .verify(&target)
                        .with_context(|| target.display().to_string())?;
                }
                Ok(())
            })();
            if let Err(error) = preflight {
                errors.push(format!("{error:#}"));
                continue;
            }
            for (operation, _, _, _) in rows {
                match self.undo(operation) {
                    Ok(()) => restored += 1,
                    Err(error) => {
                        errors.push(format!("{error:#}"));
                        break;
                    }
                }
            }
        }
        Ok((restored, errors))
    }

    pub(super) fn execute_groups(&mut self, groups: &[MoveGroup], batch: i64) -> Result<()> {
        let mut paths = HashSet::new();
        for group in groups {
            let primary = &group.entries[0];
            let root = self.registered_root(&primary.source)?;
            let (directory, device) =
                self.resolve_destination(root, Path::new(&group.folder), false)?;
            if !primary.snapshot.directory && media::video(&primary.source) {
                let current = self.subtitle_catalog(&self.source_entries(root)?);
                if let Some(subtitles) = current.files.get(&primary.source) {
                    for subtitle in subtitles {
                        if group.entries.iter().any(|entry| &entry.source == subtitle) {
                            continue;
                        }
                        let deferred = self.db.query_row(
                            "SELECT EXISTS(SELECT 1 FROM deferred WHERE path=?1)",
                            [bytes(subtitle)],
                            |row| row.get::<_, bool>(0),
                        )?;
                        ensure!(deferred, "Появились новые субтитры. Обновите предпросмотр");
                    }
                }
            }
            for item in &group.entries {
                ensure!(
                    item.destination.parent() == Some(directory.as_path()) && item.device == device,
                    "Диск назначения изменился после предпросмотра"
                );
                ensure!(paths.insert(&item.source), "Один файл выбран несколько раз");
                ensure!(
                    !present(&item.destination),
                    "Назначение уже занято: {}",
                    item.destination.display()
                );
                item.snapshot.verify(&item.source)?;
            }
        }

        let transaction = self.db.transaction()?;
        let mut operations = Vec::new();
        let mut group_head = None;
        for (group, item) in groups
            .iter()
            .flat_map(|group| group.entries.iter().map(move |item| (group, item)))
        {
            transaction.execute(
                "INSERT INTO operations(source,destination,snapshot,state) VALUES(?1,?2,?3,'planned')",
                params![bytes(&item.source), bytes(&item.destination), serde_json::to_string(&item.snapshot)?],
            )?;
            let id = transaction.last_insert_rowid();
            let head = *group_head.get_or_insert(id);
            transaction.execute(
                "INSERT INTO batch_members(operation,batch,group_head,original_device) VALUES(?1,?2,?3,?4)",
                params![id, batch, head, item.snapshot.stamp.dev.to_string()],
            )?;
            operations.push((id, group, item));
        }
        transaction.commit()?;

        let mut finished = Vec::new();
        for (index, (id, plan, item)) in operations.iter().enumerate() {
            let result = (|| -> Result<Snapshot> {
                let root = self.registered_root(&item.source)?;
                let (dir, dev) = self.resolve_destination(root, Path::new(&plan.folder), true)?;
                ensure!(
                    Some(dir.as_path()) == item.destination.parent() && dev == item.device,
                    "Диск назначения изменился"
                );
                self.transfer(
                    *id,
                    "move",
                    &item.source,
                    &item.destination,
                    &item.snapshot,
                    item.device,
                )
            })();
            match result {
                Ok(moved) => {
                    self.db.execute(
                        "UPDATE operations SET state='moved',snapshot=?2 WHERE id=?1",
                        params![id, serde_json::to_string(&moved)?],
                    )?;
                    self.db.execute(
                        "DELETE FROM manual_only WHERE path=?1",
                        [bytes(&item.source)],
                    )?;
                    finished.push(*id);
                }
                Err(error) => {
                    let state = if present(&item.source) && !self.copy_has_progress(*id)? {
                        "error"
                    } else {
                        "needs_review"
                    };
                    self.db.execute(
                        "UPDATE operations SET state=?2,message=?3 WHERE id=?1",
                        params![id, state, format!("{error:#}")],
                    )?;
                    for (skipped, _, _) in &operations[index + 1..] {
                        self.db.execute("UPDATE operations SET state='error',message='Перенос комплекта остановлен' WHERE id=?1", [skipped])?;
                    }
                    for moved in finished.into_iter().rev() {
                        if let Err(undo) = self.undo(moved) {
                            self.db.execute(
                                "UPDATE operations SET message=?2 WHERE id=?1",
                                params![
                                    moved,
                                    format!("Не удалось вернуть часть комплекта: {undo:#}")
                                ],
                            )?;
                        }
                    }
                    return Err(error);
                }
            }
        }
        for group in groups {
            self.pending.remove(&group.token);
        }
        Ok(())
    }

    pub(super) fn move_one_in_batch(
        &mut self,
        token: &str,
        folder: &str,
        batch: i64,
    ) -> Result<()> {
        let group = self.plan_group(token, folder, &mut HashSet::new(), false)?;
        self.execute_groups(&[group], batch)
    }

    fn copy_has_progress(&self, id: i64) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM copies WHERE operation=?1 AND fingerprint IS NOT NULL)",
            [id],
            |row| row.get(0),
        )?)
    }

    pub(super) fn transfer(
        &self,
        id: i64,
        direction: &str,
        source: &Path,
        destination: &Path,
        expected: &Snapshot,
        device: u64,
    ) -> Result<Snapshot> {
        expected.verify(source)?;
        let destination_parent = destination.parent().context("Нет целевой папки")?;
        let source_parent = source.parent().context("Нет исходной папки")?;
        ensure!(
            fs::metadata(destination_parent)?.dev() == device,
            "Диск назначения отключён или изменился"
        );
        if device == expected.stamp.dev {
            rename_noreplace(source, destination)?;
            let moved = Snapshot::read(destination)?;
            ensure!(
                expected.same_content(&moved),
                "Файл изменился во время переноса"
            );
            return Ok(moved);
        }

        let temporary =
            destination_parent.join(format!(".downloads-triage-{id}-{direction}-copy.part"));
        let holding =
            source_parent.join(format!(".downloads-triage-{id}-{direction}-original.part"));
        self.db.execute(
            "INSERT OR REPLACE INTO copies(operation,direction,temporary,holding,fingerprint) VALUES(?1,?2,?3,?4,NULL)",
            params![id, direction, bytes(&temporary), bytes(&holding)],
        )?;
        let staged = payload::stage(source, &temporary, expected)
            .with_context(|| format!("Не удалось подготовить копию: {}", temporary.display()))?;
        self.db.execute(
            "UPDATE copies SET fingerprint=?2 WHERE operation=?1",
            params![id, serde_json::to_string(&staged)?],
        )?;
        expected.verify(source)?;
        ensure!(
            staged.stamp.dev == device && fs::metadata(destination_parent)?.dev() == device,
            "Диск назначения отключён или изменился"
        );
        rename_noreplace(&temporary, destination)?;
        let published = Snapshot::read(destination)?;
        ensure!(
            staged.same_content(&published),
            "Итоговая копия не прошла проверку"
        );
        expected.verify(source)?;
        rename_noreplace(source, &holding)?;
        let ready = (|| -> Result<()> {
            published.verify(destination)?;
            let held = Snapshot::read(&holding)?;
            ensure!(
                expected.same_content(&held)
                    && Stamp::of(&fs::symlink_metadata(&holding)?) == held.stamp,
                "Исходник изменился"
            );
            ensure!(
                fs::metadata(destination_parent)?.dev() == device
                    && Stamp::of(&fs::symlink_metadata(destination)?) == published.stamp,
                "Копия недоступна или изменилась"
            );
            Ok(())
        })();
        if let Err(error) = ready {
            rename_noreplace(&holding, source)
                .with_context(|| format!("{error:#}. Исходник: {}", holding.display()))?;
            return Err(error);
        }
        let removed = if expected.directory {
            fs::remove_dir_all(&holding)
        } else {
            fs::remove_file(&holding)
        };
        removed.with_context(|| {
            format!(
                "Копия проверена, но не удалось убрать остаток исходника: {}",
                holding.display()
            )
        })?;
        sync_dir(source_parent)?;
        Ok(published)
    }

    pub(super) fn recover_copy(
        &self,
        id: i64,
        direction: &str,
        old: &Path,
        new: &Path,
        expected: &Snapshot,
    ) -> Result<Option<(bool, Option<Snapshot>, String)>> {
        let row = self.db.query_row(
            "SELECT temporary,holding,fingerprint FROM copies WHERE operation=?1 AND direction=?2",
            params![id, direction],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        );
        let (temporary, holding, fingerprint) = match row {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let temporary = from_bytes(temporary);
        let holding = from_bytes(holding);
        let staged = fingerprint
            .map(|data| serde_json::from_str::<Snapshot>(&data))
            .transpose()?;
        let final_file = Snapshot::read(new).ok().filter(|file| {
            staged
                .as_ref()
                .is_some_and(|staged| staged.same_content(file))
        });
        let source_parent_ok = old.parent().is_some_and(|directory| {
            directory
                .canonicalize()
                .is_ok_and(|canonical| canonical == directory)
                && fs::metadata(directory).is_ok_and(|meta| meta.dev() == expected.stamp.dev)
        });
        let missing = |path: &Path| {
            fs::symlink_metadata(path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        };
        if final_file.is_some() && source_parent_ok && missing(old) && missing(&holding) {
            return Ok(Some((
                true,
                final_file,
                "Перенос восстановлен по журналу".into(),
            )));
        }
        let mut message =
            "Копирование было прервано. Проверьте исходный и конечный пути".to_owned();
        if present(&holding) {
            if source_parent_ok
                && missing(old)
                && Snapshot::read(&holding).is_ok_and(|snapshot| expected.same_content(&snapshot))
                && rename_noreplace(&holding, old).is_ok()
            {
                message.push_str(". Исходник возвращён на место");
            } else {
                message.push_str(&format!(
                    ". Исходник сохранён под временным именем: {}",
                    holding.display()
                ));
            }
        }
        if present(&temporary) {
            message.push_str(&format!(". Временная копия: {}", temporary.display()));
        }
        Ok(Some((false, None, message)))
    }

    pub(super) fn find_duplicates(&self, token: &str, root: &Path) -> Result<Vec<String>> {
        let item = self.pending.get(token).context("Предпросмотр устарел")?;
        let source = item
            .snapshot
            .as_ref()
            .filter(|snapshot| !snapshot.directory)
            .context("Выберите готовый файл")?;
        source.verify(&item.path)?;
        let root = root.canonicalize().context("Папка для поиска недоступна")?;
        ensure!(root.is_dir(), "Выберите папку для поиска");
        let mut stack = vec![root];
        let mut visited = HashSet::new();
        let mut result = Vec::new();
        while let Some(directory) = stack.pop() {
            let metadata = fs::symlink_metadata(&directory)?;
            if !metadata.is_dir() || !visited.insert((metadata.dev(), metadata.ino())) {
                continue;
            }
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path)?;
                if metadata.is_dir() {
                    stack.push(path);
                } else if metadata.is_file()
                    && metadata.len() == source.stamp.size
                    && path != item.path
                    && Snapshot::read(&path).is_ok_and(|snapshot| source.same_payload(&snapshot))
                {
                    result.push(path.to_string_lossy().into_owned());
                }
            }
        }
        result.sort();
        Ok(result)
    }
}
