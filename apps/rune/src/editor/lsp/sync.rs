//! Per-document server synchronization state.
//!
//! One [`DocSync`] describes the single open document mirrored to one
//! language server: the URI/language identity, the connection epoch the
//! sync belongs to, the wire version, the last text actually sent, and
//! the pending debounce/save flags the poll loop flushes in order
//! (open → change → save). A departing document's `didClose` is sent
//! synchronously at reconcile time, so no close queue is needed — the
//! command channel's FIFO order already guarantees close-before-reopen.

use std::path::PathBuf;
use std::time::Instant;

use app_core::lsp::LspKey;

/// State for the one document currently mirrored to a server.
pub struct DocSync {
    /// `(checkout_key, server_id)` this sync belongs to.
    pub key: LspKey,
    /// Checkout root the URI was built from (for `initialize` + URI checks).
    pub checkout: PathBuf,
    /// `file://` URI sent in document notifications.
    pub uri: String,
    /// LSP language id from the extension map.
    pub language_id: String,
    /// Repo-relative path, for identity checks against `OpenDoc.path`.
    pub path: String,
    /// Connection epoch currently mirrored. A status epoch change resets
    /// `open`/`wire_version`/`last_sent` so the document re-opens on the
    /// new connection.
    pub epoch: u64,
    /// `didOpen` sent on the current epoch.
    pub open: bool,
    /// `doc_opened` was counted for this logical doc (once per key, not
    /// per epoch — the hub's `open_docs` tracks logical opens).
    pub reported_open: bool,
    /// Per-document monotonic version (1 at `didOpen`, +1 per change).
    pub wire_version: u64,
    /// Text last sent to the server (diff base for incremental sync).
    pub last_sent: String,
    /// `CodeState.revision` last observed; a mismatch arms the debounce.
    pub revision_seen: u64,
    /// When the pending change may flush (None = nothing pending).
    pub change_at: Option<Instant>,
    /// A save completed since the last flush → `didSave` is due.
    pub save_pending: bool,
}

impl DocSync {
    pub fn new(
        key: LspKey,
        checkout: PathBuf,
        uri: String,
        path: String,
        language_id: String,
    ) -> Self {
        DocSync {
            key,
            checkout,
            uri,
            language_id,
            path,
            epoch: 0,
            open: false,
            reported_open: false,
            wire_version: 0,
            last_sent: String::new(),
            revision_seen: 0,
            change_at: None,
            save_pending: false,
        }
    }

    /// Connection epoch changed (crash/respawn): the new server knows
    /// nothing, so the document must re-open from scratch. Logical
    /// `reported_open` stays — the doc never closed editor-side.
    pub fn reset_for_epoch(&mut self, epoch: u64) {
        self.epoch = epoch;
        self.open = false;
        self.wire_version = 0;
        self.last_sent.clear();
        self.change_at = None;
        // `save_pending` survives: a save that raced the crash still
        // deserves its `didSave` once the doc re-opens.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_reset_reopens_but_keeps_logical_open_count() {
        let mut s = DocSync::new(
            LspKey::new("k", "srv"),
            PathBuf::from("/co"),
            "file:///co/a.rs".into(),
            "a.rs".into(),
            "rust".into(),
        );
        s.epoch = 3;
        s.open = true;
        s.reported_open = true;
        s.wire_version = 7;
        s.last_sent = "fn main() {}".into();
        s.reset_for_epoch(4);
        assert_eq!(s.epoch, 4);
        assert!(!s.open);
        assert_eq!(s.wire_version, 0);
        assert!(s.last_sent.is_empty());
        assert!(s.reported_open, "logical open survives a respawn");
    }
}
