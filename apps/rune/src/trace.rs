//! A trace of what the terminal does between frames.
//!
//! `GOKO_RUNE_TRACE=/path/to/log` turns it on. The log names the pane's
//! rect, the grid's size and the content of the first rows, frame by
//! frame — enough to tell a layout that moves from content that changes.

use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

static TRACE: OnceLock<Option<PathBuf>> = OnceLock::new();

/// The trace file, when `GOKO_RUNE_TRACE` names one.
fn path() -> Option<&'static PathBuf> {
    TRACE
        .get_or_init(|| {
            std::env::var_os("GOKO_RUNE_TRACE")
                .map(PathBuf::from)
                .filter(|path| !path.as_os_str().is_empty())
        })
        .as_ref()
}

/// Whether `GOKO_RUNE_TRACE` turned the trace on: a caller that builds its
/// line every frame checks this first, so an untraced app formats nothing.
pub fn enabled() -> bool {
    path().is_some()
}

/// Append one line to the trace. A trace that cannot be written is not an
/// error: it is a debugging aid, never a reason to break the app.
pub fn trace(line: &str) {
    let Some(path) = path() else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}
