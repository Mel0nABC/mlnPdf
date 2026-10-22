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

/// Default location: `$XDG_CONFIG_HOME/visor_pdf/state.txt`.
pub fn state_file() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("visor_pdf").join("state.txt")
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
}
