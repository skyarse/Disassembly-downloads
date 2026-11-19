use super::{bytes, sync_dir, Snapshot, Stamp};
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, FileTimes, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

fn record(path: &Path, relative: &Path, device: u64, digest: &mut Sha256) -> Result<()> {
    ensure!(
        relative.components().count() <= 128,
        "Слишком глубокая вложенность папок"
    );
    let metadata = fs::symlink_metadata(path)?;
    let stamp = Stamp::of(&metadata);
    ensure!(
        metadata.dev() == device,
        "Папка содержит точку монтирования"
    );
    ensure!(
        metadata.is_dir() || metadata.is_file(),
        "Комплект содержит ссылку или особый файл"
    );
    let name = bytes(relative);
    digest.update((name.len() as u64).to_le_bytes());
    digest.update(name);
    digest.update(metadata.mode().to_le_bytes());
    if metadata.is_dir() {
        let mut paths = fs::read_dir(path)?
            .map(|entry| entry.map(|e| e.path()))
            .collect::<io::Result<Vec<_>>>()?;
        paths.sort();
        for child in paths {
            record(
                &child,
                &relative.join(child.file_name().unwrap()),
                device,
                digest,
            )?;
        }
    } else {
        let value = Snapshot::read(&path)?;
        digest.update(value.stamp.size.to_le_bytes());
        digest.update(value.hash.as_bytes());
    }
    ensure!(
        Stamp::of(&fs::symlink_metadata(path)?) == stamp,
        "Комплект изменился во время проверки"
    );
    Ok(())
}

pub(super) fn directory_snapshot(path: &Path, metadata: &fs::Metadata) -> Result<Snapshot> {
    let mut digest = Sha256::new();
    record(path, Path::new(""), metadata.dev(), &mut digest)?;
    let stamp = Stamp::of(metadata);
    ensure!(
        Stamp::of(&fs::symlink_metadata(path)?) == stamp,
        "Папка изменилась во время проверки"
    );
    Ok(Snapshot {
        stamp,
        hash: format!("{:x}", digest.finalize()),
        directory: true,
    })
}

fn copy_regular(source: &Path, mut target: File) -> Result<()> {
    let mut source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(source)?;
    let before = source.metadata()?;
    ensure!(before.is_file(), "Разрешены только обычные файлы");
    io::copy(&mut source, &mut target)?;
    ensure!(
        Stamp::of(&source.metadata()?) == Stamp::of(&before),
        "Исходник изменился при копировании"
    );
    target.set_permissions(fs::Permissions::from_mode(before.mode() & 0o777))?;
    target.set_times(FileTimes::new().set_modified(before.modified()?))?;
    target.sync_all()?;
    Ok(())
}

fn create_regular(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?)
}

fn copy_directory(source: &Path, target: &Path, device: u64, depth: usize) -> Result<()> {
    ensure!(depth <= 128, "Слишком глубокая вложенность папок");
    let metadata = fs::symlink_metadata(source)?;
    ensure!(
        metadata.is_dir() && metadata.dev() == device,
        "Папка комплекта изменилась"
    );
    for item in fs::read_dir(source)? {
        let item = item?;
        let path = item.path();
        let dst = target.join(item.file_name());
        let child = fs::symlink_metadata(&path)?;
        ensure!(child.dev() == device, "Папка содержит точку монтирования");
        match child.file_type() {
            kind if kind.is_dir() => {
                fs::create_dir(&dst)?;
                copy_directory(&path, &dst, device, depth + 1)?;
            }
            kind if kind.is_file() => copy_regular(&path, create_regular(&dst)?)?,
            _ => anyhow::bail!("Комплект содержит ссылку или особый файл"),
        }
    }
    let directory = File::open(target)?;
    directory.set_permissions(fs::Permissions::from_mode(metadata.mode() & 0o777))?;
    directory.set_times(FileTimes::new().set_modified(metadata.modified()?))?;
    directory.sync_all()?;
    Ok(())
}

pub(super) fn stage(source: &Path, temporary: &Path, expected: &Snapshot) -> Result<Snapshot> {
    if expected.directory {
        fs::create_dir(temporary)?;
        copy_directory(source, temporary, expected.stamp.dev, 0)?;
    } else {
        let staged = create_regular(temporary)?;
        let identity = staged.metadata()?;
        if let Err(error) = copy_regular(source, staged) {
            if fs::symlink_metadata(temporary)
                .is_ok_and(|meta| meta.dev() == identity.dev() && meta.ino() == identity.ino())
            {
                let _ = fs::remove_file(temporary);
            }
            return Err(error);
        }
    }
    let snapshot = Snapshot::read(temporary)?;
    ensure!(
        expected.same_payload(&snapshot),
        "Содержимое копии отличается от исходника"
    );
    sync_dir(
        temporary
            .parent()
            .context("Нет каталога для временной копии")?,
    )?;
    Ok(snapshot)
}
