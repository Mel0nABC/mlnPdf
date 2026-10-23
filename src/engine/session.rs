//! What survives between runs: last file, reading position and recent files.

use std::path::{Path, PathBuf};

use crate::engine::zoom::ZOOM_DEFAULT;

/// How many recently opened files are remembered.
pub const RECENT_LIMIT: usize = 8;

/// Last file, where the reader was, and the recent list.
#[derive(Default, Debug, PartialEq)]
pub struct Session {
    pub last: Option<PathBuf>,
    pub page: usize,
    pub zoom: f64,
    pub recent: Vec<PathBuf>,
}

/// Default location: `$XDG_CONFIG_HOME/mlnpdf/state.txt`.
pub fn state_file() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("mlnpdf").join("state.txt")
}

/// Reads a session from an explicit path, so tests never touch the real one.
pub fn load_session_from(path: &Path) -> Session {
    let mut session = Session {
        zoom: ZOOM_DEFAULT,
        ..Session::default()
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return session;
    };

    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "last" => session.last = Some(PathBuf::from(value)),
            "page" => session.page = value.parse().unwrap_or(0),
            "zoom" => session.zoom = value.parse().unwrap_or(ZOOM_DEFAULT),
            "recent" => session.recent.push(PathBuf::from(value)),
            _ => {}
        }
    }
    session
}

pub fn save_session_to(session: &Session, path: &Path) {
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }

    let mut text = String::new();
    if let Some(last) = &session.last {
        text.push_str(&format!("last={}\n", last.display()));
    }
    text.push_str(&format!("page={}\n", session.page));
    text.push_str(&format!("zoom={}\n", session.zoom));
    for file in &session.recent {
        text.push_str(&format!("recent={}\n", file.display()));
    }
    let _ = std::fs::write(path, text);
}

pub fn load_session() -> Session {
    load_session_from(&state_file())
}

pub fn save_session(session: &Session) {
    save_session_to(session, &state_file());
}

/// True when both paths name the same file.
///
/// The application is launched with whatever the caller typed, so the same
/// document can arrive as a relative path, an absolute one, or through a
/// symlink. Resolving first keeps the session matching it.
pub fn same_document(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Folder the file chooser should start in: the one holding the last document,
/// or the most recent one that still exists.
///
/// The remembered paths are used in order, so a deleted document does not drag
/// the reader to a folder that is gone.
pub fn last_directory(session: &Session) -> Option<PathBuf> {
    let candidates = session.last.iter().chain(session.recent.iter());
    for path in candidates {
        let Some(parent) = path.parent() else {
            continue;
        };
        if parent.is_dir() {
            return Some(parent.to_path_buf());
        }
    }
    None
}

/// Page and zoom saved for `path`, when the session remembers that document.
///
/// The single stored position belongs to the last file only, so any other
/// document has no position to restore.
pub fn saved_position_for(session: &Session, path: &Path) -> Option<(usize, f64)> {
    let last = session.last.as_deref()?;
    if !same_document(last, path) {
        return None;
    }
    Some((session.page, session.zoom))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-work");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir.join(name)
    }

    #[test]
    fn round_trips_through_the_state_file() {
        let path = scratch("session-round-trip.txt");
        let session = Session {
            last: Some(PathBuf::from("/documentos/un archivo.pdf")),
            page: 42,
            zoom: 1.5,
            recent: vec![PathBuf::from("/a.pdf"), PathBuf::from("/b.pdf")],
        };
        save_session_to(&session, &path);

        let loaded = load_session_from(&path);
        assert_eq!(loaded, session);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn missing_file_yields_defaults() {
        let loaded = load_session_from(&scratch("session-missing.txt"));
        assert_eq!(loaded.last, None);
        assert_eq!(loaded.page, 0);
        assert_eq!(loaded.zoom, ZOOM_DEFAULT);
        assert!(loaded.recent.is_empty());
    }

    #[test]
    fn ignores_garbage_and_keeps_valid_lines() {
        let path = scratch("session-garbage.txt");
        std::fs::write(&path, "page=no-es-numero\nzoom=3.0\nclave=valor\nlast=/x.pdf\n").expect("write");
        let loaded = load_session_from(&path);
        assert_eq!(loaded.page, 0, "un número inválido cae en 0");
        assert_eq!(loaded.zoom, 3.0);
        assert_eq!(loaded.last, Some(PathBuf::from("/x.pdf")));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn last_directory_follows_the_last_document_then_the_recent_ones() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-work");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let opened = dir.join("session-last-dir.pdf");
        std::fs::write(&opened, b"%PDF-1.4\n").expect("write scratch pdf");
        let gone = dir.join("session-gone.pdf");

        let remembered = Session {
            last: Some(opened.clone()),
            recent: vec![opened.clone()],
            ..Session::default()
        };
        assert_eq!(last_directory(&remembered), Some(dir.clone()));

        // A document that no longer exists falls through to the recent list.
        let deleted_last = Session {
            last: Some(gone.clone()),
            recent: vec![gone, opened.clone()],
            ..Session::default()
        };
        assert_eq!(last_directory(&deleted_last), Some(dir.clone()));

        // Nothing remembered, or nothing whose folder still exists.
        assert_eq!(last_directory(&Session::default()), None);
        let nowhere = Session {
            last: Some(PathBuf::from("sin-directorio.pdf")),
            recent: vec![PathBuf::from("otro.pdf")],
            ..Session::default()
        };
        assert_eq!(last_directory(&nowhere), None, "una ruta relativa no da carpeta");
        std::fs::remove_file(opened).ok();
    }

    fn existing(name: &str) -> PathBuf {
        let path = scratch(name);
        std::fs::write(&path, b"%PDF-1.4\n").expect("write scratch pdf");
        path
    }

    #[test]
    fn same_document_resolves_relative_and_absolute_paths() {
        let absolute = existing("session-same-document.pdf");
        let relative = PathBuf::from(".")
            .join(absolute.strip_prefix(env!("CARGO_MANIFEST_DIR")).expect("inside the project"));
        assert!(same_document(&absolute, &relative));
        assert!(same_document(&absolute, &absolute));
        std::fs::remove_file(absolute).ok();
    }

    #[test]
    fn same_document_rejects_other_files_and_missing_ones() {
        let first = existing("session-first.pdf");
        let second = existing("session-second.pdf");
        assert!(!same_document(&first, &second));

        let missing = scratch("session-does-not-exist.pdf");
        std::fs::remove_file(&missing).ok();
        assert!(!same_document(&first, &missing));
        assert!(same_document(&missing, &missing), "sin archivo se comparan tal cual");
        std::fs::remove_file(first).ok();
        std::fs::remove_file(second).ok();
    }

    #[test]
    fn saved_position_is_offered_only_for_the_remembered_document() {
        let remembered = existing("session-offered.pdf");
        let other = existing("session-not-offered.pdf");
        let session = Session {
            last: Some(remembered.clone()),
            page: 39,
            zoom: 1.5,
            recent: Vec::new(),
        };

        assert_eq!(saved_position_for(&session, &remembered), Some((39, 1.5)));
        assert_eq!(saved_position_for(&session, &other), None);
        assert_eq!(saved_position_for(&Session::default(), &remembered), None);
        std::fs::remove_file(remembered).ok();
        std::fs::remove_file(other).ok();
    }
}
