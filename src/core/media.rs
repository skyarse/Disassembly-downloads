use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

pub fn video(path: &Path) -> bool {
    let extension = path.extension().and_then(|value| value.to_str());
    extension.is_some_and(|extension| {
        ["mkv", "mp4", "avi", "mov", "webm", "m4v", "ts", "mpg"]
            .iter()
            .any(|value| extension.eq_ignore_ascii_case(value))
    })
}

pub fn season(path: &Path) -> Option<&str> {
    let name = path.file_stem()?.to_str()?;
    let bytes = name.as_bytes();
    for (index, item) in bytes.windows(6).enumerate() {
        if (index == 0 || !bytes[index - 1].is_ascii_alphanumeric())
            && item[0].eq_ignore_ascii_case(&b's')
            && item[1..3].iter().all(u8::is_ascii_digit)
            && item[3].eq_ignore_ascii_case(&b'e')
            && item[4..].iter().all(u8::is_ascii_digit)
        {
            return name.get(index + 1..index + 3);
        }
    }
    None
}

fn subtitle_stem(path: &Path) -> Option<String> {
    let extensions = ["srt", "ass", "ssa", "vtt", "sub", "idx", "sup"];
    let extension = path.extension()?.to_str()?;
    let path = if ["part", "crdownload", "partial", "tmp", "download", "aria2"]
        .iter()
        .any(|value| extension.eq_ignore_ascii_case(value))
    {
        Path::new(path.file_stem()?)
    } else {
        path
    };
    let extension = path.extension()?.to_str()?;
    if !extensions
        .iter()
        .any(|value| extension.eq_ignore_ascii_case(value))
    {
        return None;
    }
    Some(path.file_stem()?.to_str()?.to_lowercase())
}

fn subtitle_suffix(value: &str) -> bool {
    [
        "ru",
        "rus",
        "en",
        "eng",
        "ja",
        "jpn",
        "uk",
        "ukr",
        "zh",
        "zho",
        "chi",
        "ko",
        "kor",
        "es",
        "spa",
        "de",
        "deu",
        "fr",
        "fra",
        "it",
        "ita",
        "pt",
        "por",
        "pt-br",
        "forced",
        "default",
        "sdh",
        "signs",
        "songs",
        "commentary",
    ]
    .contains(&value)
}

#[derive(Default)]
pub struct Subtitles {
    pub files: HashMap<PathBuf, Vec<PathBuf>>,
    pub ambiguous: HashSet<PathBuf>,
}

pub fn subtitles(paths: &[PathBuf]) -> Subtitles {
    let mut by_name = HashMap::<String, Vec<&PathBuf>>::new();
    for path in paths.iter().filter(|path| video(path)) {
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
            by_name.entry(stem.to_lowercase()).or_default().push(path);
        }
    }
    let mut groups = Subtitles::default();
    for path in paths {
        let Some(stem) = subtitle_stem(path) else {
            continue;
        };
        let mut base = stem.as_str();
        loop {
            if let Some(videos) = by_name.get(base) {
                if videos.len() == 1 {
                    groups
                        .files
                        .entry(videos[0].clone())
                        .or_default()
                        .push(path.clone());
                } else {
                    groups
                        .ambiguous
                        .extend(videos.iter().map(|video| (*video).clone()));
                    groups.ambiguous.insert(path.clone());
                }
                break;
            }
            match base.rsplit_once('.') {
                Some((prefix, suffix)) if subtitle_suffix(suffix) => base = prefix,
                _ => break,
            }
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_episode_tokens_and_unambiguous_subtitle_names() {
        assert_eq!(
            season(Path::new("[Release] Название.S02E03.1080p.mkv")),
            Some("02")
        );
        assert_eq!(season(Path::new("DS02E03.png")), None);
        let paths = [
            "Title.mkv",
            "Title.ru.ass",
            "Title.en.forced.srt.part",
            "Title.ru.mp4",
            "Titlex.srt",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
        let groups = subtitles(&paths);
        assert_eq!(
            groups.files[Path::new("Title.mkv")],
            [PathBuf::from("Title.en.forced.srt.part")]
        );
        assert_eq!(
            groups.files[Path::new("Title.ru.mp4")],
            [PathBuf::from("Title.ru.ass")]
        );
    }
}
