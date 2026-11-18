//! Background refresh of the Changes section: a quiet 3-second probe on the
//! same one-outstanding rule as `branch::poll`, backing off when a read is
//! slow. The read rides a dedicated `Req::ChangesPoll` — never `Req::Changes`
//! — so it applies without the loading spinner, the section error row or a
//! notice, and its generation lane never supersedes a manual refresh.
//!
//! The Files section's open document gets a second, independent 3-second
//! schedule: `Req::CheckFile` re-reads the file on disk so a clean buffer
//! can follow external writes and a dirty one raises `disk_notice`. It is
//! gated to a complete text doc with no save or reload in flight — those
//! own the next doc state.

use std::time::{Duration, Instant};

use rune::Tx;

use crate::editor::backend::Req;
use crate::editor::state::{EditorState, Section};

/// The base cadence, shared with the branch badge; also the backoff floor.
pub const INTERVAL: f64 = 3.0;
/// A slow read never pushes the next one out past this.
const MAX_DELAY: f64 = 30.0;

/// The poll's schedule, keyed to the checkout it was armed for — a switch
/// rebuilds it (see `poll`), so a stale `pending` can never block the new
/// checkout's reads.
#[derive(Default)]
pub struct AutoRefresh {
    /// Checkout key this schedule belongs to.
    pub checkout: Option<String>,
    /// A `Req::ChangesPoll` read is in flight.
    pub pending: bool,
    /// When the in-flight (or last) read was sent: the `Instant` measures
    /// how long the worker took, for the backoff.
    pub sent: Option<Instant>,
    /// The poll-clock value at `sent` — `next_at` is anchored to it, so the
    /// reply handler can schedule without seeing the poll clock itself.
    pub sent_clock: f64,
    /// Poll-clock time the next read is due.
    pub next_at: f64,
    /// A `Req::CheckFile` probe of the open doc is in flight.
    pub check_pending: bool,
    /// Poll-clock time the next disk check is due.
    pub check_next_at: f64,
    /// Last recorded poll's `view_is_editor`, for the became-active edge —
    /// not updated while no checkout is selected, so the edge stays armed.
    pub was_active: bool,
    /// Last recorded poll's `focused`, for the became-active edge.
    pub was_focused: bool,
}

/// The wait after a read that took `took`: the 3 s cadence, or five times
/// the read time when that is longer, capped at 30 s — a huge repository is
/// polled gently.
pub fn next_delay(took: Duration) -> Duration {
    Duration::from_secs_f64(INTERVAL.max(took.as_secs_f64() * 5.0).min(MAX_DELAY))
}

/// The Changes poll, driven from `main::poll` beside `branch::poll`. Reads
/// every 3 s only while the Editor view is focused and the section is
/// Changes; at most one read outstanding; skipped silently while a write is
/// in flight (`changes.busy`) or the queue refuses. View entry and window
/// focus gains read at once — and refresh the file list once, so files an
/// agent created appear in the tree. That edge waits for a selected
/// checkout: entering the Editor before one is chosen leaves it armed, so
/// the first poll with a checkout still reads immediately.
pub fn poll(
    ed: &mut EditorState,
    view_is_editor: bool,
    focused: bool,
    now: f64,
    tx: &mut Tx,
) -> bool {
    let active = view_is_editor && focused;
    if !active {
        // Idle: record the flags so re-entry still edges, and release the
        // in-flight flags so a late reply is not awaited — the
        // became-active edge re-reads at once anyway.
        ed.autorefresh.was_active = view_is_editor;
        ed.autorefresh.was_focused = focused;
        ed.autorefresh.pending = false;
        ed.autorefresh.check_pending = false;
        return false;
    }
    let (Some(checkout), Some(key)) = (ed.checkout.clone(), ed.checkout_key.clone()) else {
        // No checkout yet: leave `was_active`/`was_focused` untouched so
        // the entry edge stays armed and fires on the first poll that
        // has one.
        return false;
    };
    // The entry/focus edge compares this call's inputs to the last
    // recorded poll's — recorded only once a checkout exists, so the gate
    // above can never consume it.
    let entered = !(ed.autorefresh.was_active && ed.autorefresh.was_focused);
    ed.autorefresh.was_active = view_is_editor;
    ed.autorefresh.was_focused = focused;
    if ed.autorefresh.checkout.as_ref() != Some(&key) {
        ed.autorefresh = AutoRefresh {
            checkout: Some(key.clone()),
            was_active: view_is_editor,
            was_focused: focused,
            ..Default::default()
        };
    }
    // The file list belongs to the Files tree, so the entry/focus refresh
    // runs in whatever section is showing; only the Changes read is gated.
    let mut sent = false;
    if entered {
        sent = ed.enqueue_read(checkout.clone(), key.clone(), Req::FileList);
        ed.autorefresh.next_at = 0.0;
        ed.autorefresh.check_next_at = 0.0;
    }
    // The open doc gets its own 3 s cadence while the Files section shows
    // a complete text file and no save or reload is in flight — those
    // own the doc's next state, so the probe stays out of their way.
    if ed.section == Section::Files {
        let checkable = ed.doc.as_ref().is_some_and(|doc| {
            doc.kind == crate::editor::draft::DocKind::Text && !doc.truncated && !doc.saving
        }) && ed.doc_reload.is_none();
        if !checkable {
            return sent;
        }
        tx.wake_after(Duration::from_secs_f64(INTERVAL));
        if ed.autorefresh.check_pending || now < ed.autorefresh.check_next_at {
            return sent;
        }
        let doc = ed.doc.as_ref().unwrap();
        let req = Req::CheckFile {
            path: doc.path.clone(),
            revision: doc.code.revision,
        };
        ed.autorefresh.check_pending = ed.enqueue_read(checkout, key, req);
        if ed.autorefresh.check_pending {
            ed.autorefresh.check_next_at = now + INTERVAL;
            sent = true;
        }
        return sent;
    }
    if ed.section != Section::Changes {
        return sent;
    }
    // Keep the cadence alive while the UI idles, like the branch probe.
    tx.wake_after(Duration::from_secs_f64(INTERVAL));
    if ed.changes.busy.is_some() {
        return sent;
    }
    if ed.autorefresh.pending || now < ed.autorefresh.next_at {
        return sent;
    }
    ed.autorefresh.pending = ed.enqueue_read(checkout, key, Req::ChangesPoll);
    if ed.autorefresh.pending {
        ed.autorefresh.sent = Some(Instant::now());
        ed.autorefresh.sent_clock = now;
        sent = true;
    }
    sent
}

/// Make the next `poll` read at once — for work that knows fresh data is
/// coming (the disk-check probe and its siblings). Resets both schedules:
/// whatever changed under the editor may have moved the open file too.
#[allow(dead_code)] // armed by later triggers; tests call it directly
pub fn kick(ed: &mut EditorState) {
    ed.autorefresh.next_at = 0.0;
    ed.autorefresh.check_next_at = 0.0;
}

/// Make the next `poll`'s disk check due at once — the tab activation path
/// calls it so a newly focused doc is re-read before its next 3 s tick.
#[allow(dead_code)] // the tab-activation caller lands in a later task
pub fn check_now(ed: &mut EditorState) {
    ed.autorefresh.check_next_at = 0.0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::backend::{Job, Reply, ReplyData, channel_backend};
    use app_core::git_view::{GitViewChanges, StatusEntry};
    use rune::{Env, Runtime};
    use std::path::PathBuf;

    fn editor() -> (EditorState, std::sync::mpsc::Receiver<Job>) {
        let mut ed = EditorState::new(rune_kit::Kit::neutral(true), crate::test_handle());
        let (backend, jobs) = channel_backend();
        ed.backend = backend;
        ed.project_id = Some("p".into());
        ed.checkout = Some(PathBuf::from("/tmp/autorefresh-test"));
        ed.checkout_key = Some("key".into());
        (ed, jobs)
    }

    fn changes() -> GitViewChanges {
        GitViewChanges {
            branch: Some("main".into()),
            head: "12345678".into(),
            conflicts: Vec::new(),
            staged: Vec::new(),
            unstaged: Vec::new(),
            untracked: Vec::new(),
            ahead: None,
            operation: None,
            merge_message: None,
        }
    }

    fn poll_reply(job: &Job, result: Result<ReplyData, String>) -> Reply {
        Reply {
            req: job.req.clone(),
            generation: job.generation,
            checkout_key: job.checkout_key.clone(),
            result,
        }
    }

    #[test]
    fn next_delay_floors_at_three_seconds_and_caps_at_thirty() {
        assert_eq!(
            next_delay(Duration::from_millis(100)),
            Duration::from_secs(3)
        );
        assert_eq!(next_delay(Duration::from_secs(2)), Duration::from_secs(10));
        assert_eq!(next_delay(Duration::from_secs(10)), Duration::from_secs(30));
    }

    #[test]
    fn poll_reads_after_three_seconds_and_never_twice_at_once() {
        let (ed, jobs) = editor();
        let mut rt = Runtime::new(ed, |_, _| rune::row(), (400.0, 100.0), Env::new(true, 2.0));
        // The entry edge reads at once; the reply then schedules 3 s out.
        rt.update(|ed, tx| assert!(poll(ed, true, true, 0.0, tx)));
        jobs.try_recv().unwrap();
        let read = jobs.try_recv().unwrap();
        rt.state
            .apply_reply(poll_reply(&read, Ok(ReplyData::Changes(changes()))));
        rt.update(|ed, tx| assert!(!poll(ed, true, true, 2.9, tx)));
        assert!(jobs.try_recv().is_err(), "no read before the cadence");
        rt.update(|ed, tx| assert!(poll(ed, true, true, 3.0, tx)));
        assert!(matches!(jobs.try_recv().unwrap().req, Req::ChangesPoll));
        rt.update(|ed, tx| assert!(!poll(ed, true, true, 6.0, tx)));
        assert!(jobs.try_recv().is_err(), "one outstanding read");
    }

    #[test]
    fn poll_is_silent_when_inactive() {
        let (ed, jobs) = editor();
        let mut rt = Runtime::new(ed, |_, _| rune::row(), (400.0, 100.0), Env::new(true, 2.0));
        rt.update(|ed, tx| assert!(!poll(ed, false, true, 10.0, tx)));
        rt.update(|ed, tx| assert!(!poll(ed, true, false, 10.0, tx)));
        rt.update(|ed, tx| {
            // Already active: no entry edge, so only the section gate speaks.
            ed.autorefresh.was_active = true;
            ed.autorefresh.was_focused = true;
            ed.section = Section::Files;
            assert!(!poll(ed, true, true, 10.0, tx));
            ed.section = Section::Changes;
        });
        rt.update(|ed, tx| {
            ed.checkout = None;
            assert!(!poll(ed, true, true, 10.0, tx));
            ed.checkout = Some(PathBuf::from("/tmp/autorefresh-test"));
        });
        rt.update(|ed, tx| {
            ed.changes.busy = Some("stage");
            assert!(!poll(ed, true, true, 10.0, tx));
            ed.changes.busy = None;
        });
        assert!(jobs.try_recv().is_err(), "no read of any kind fired");
    }

    #[test]
    fn view_entry_and_focus_gain_read_at_once_and_refresh_the_file_list() {
        let (ed, jobs) = editor();
        let mut rt = Runtime::new(ed, |_, _| rune::row(), (400.0, 100.0), Env::new(true, 2.0));
        // View entry: read at once — no 3 s wait — plus one file list.
        rt.update(|ed, tx| assert!(poll(ed, true, true, 0.5, tx)));
        assert!(matches!(jobs.try_recv().unwrap().req, Req::FileList));
        assert!(matches!(jobs.try_recv().unwrap().req, Req::ChangesPoll));
        rt.update(|ed, tx| assert!(!poll(ed, true, true, 0.6, tx)));
        assert!(
            jobs.try_recv().is_err(),
            "staying active: no FileList again"
        );
        // Focus gain after being away: the same immediate read + FileList.
        rt.update(|ed, tx| assert!(!poll(ed, true, false, 1.0, tx)));
        rt.update(|ed, tx| assert!(poll(ed, true, true, 1.5, tx)));
        assert!(matches!(jobs.try_recv().unwrap().req, Req::FileList));
        assert!(matches!(jobs.try_recv().unwrap().req, Req::ChangesPoll));
    }

    /// Entering the Editor before a checkout is chosen must not consume
    /// the entry edge: polls with nothing selected enqueue nothing and
    /// leave the edge armed, so the first poll with a checkout still
    /// refreshes the file list and reads Changes at once — and only once.
    #[test]
    fn entry_edge_waits_for_a_checkout() {
        let (ed, jobs) = editor();
        let mut rt = Runtime::new(ed, |_, _| rune::row(), (400.0, 100.0), Env::new(true, 2.0));
        rt.update(|ed, tx| {
            ed.checkout = None;
            ed.checkout_key = None;
            assert!(!poll(ed, true, true, 0.0, tx));
            assert!(!poll(ed, true, true, 0.5, tx));
        });
        assert!(jobs.try_recv().is_err(), "no checkout: nothing enqueued");
        rt.update(|ed, tx| {
            ed.checkout = Some(PathBuf::from("/tmp/autorefresh-test"));
            ed.checkout_key = Some("key".into());
            assert!(poll(ed, true, true, 1.0, tx));
        });
        assert!(matches!(jobs.try_recv().unwrap().req, Req::FileList));
        assert!(matches!(jobs.try_recv().unwrap().req, Req::ChangesPoll));
        rt.update(|ed, tx| assert!(!poll(ed, true, true, 1.5, tx)));
        assert!(
            jobs.try_recv().is_err(),
            "staying active: no FileList again"
        );
    }

    #[test]
    fn view_entry_in_the_files_section_refreshes_the_file_list_only() {
        let (ed, jobs) = editor();
        let mut rt = Runtime::new(ed, |_, _| rune::row(), (400.0, 100.0), Env::new(true, 2.0));
        rt.update(|ed, _| ed.section = Section::Files);
        // The tree is what shows in Files, so entry re-reads the file list...
        rt.update(|ed, tx| assert!(poll(ed, true, true, 0.0, tx)));
        assert!(matches!(jobs.try_recv().unwrap().req, Req::FileList));
        // ...but Changes is not read while another section shows, and a
        // later poll in Files adds nothing.
        assert!(jobs.try_recv().is_err());
        rt.update(|ed, tx| assert!(!poll(ed, true, true, 5.0, tx)));
        assert!(jobs.try_recv().is_err());
        // Switching to Changes reads at once (no 3 s wait after entry).
        rt.update(|ed, tx| {
            ed.section = Section::Changes;
            assert!(poll(ed, true, true, 5.1, tx));
        });
        assert!(matches!(jobs.try_recv().unwrap().req, Req::ChangesPoll));
    }

    #[test]
    fn kick_makes_the_next_poll_read() {
        let (ed, jobs) = editor();
        let mut rt = Runtime::new(ed, |_, _| rune::row(), (400.0, 100.0), Env::new(true, 2.0));
        rt.update(|ed, tx| assert!(poll(ed, true, true, 0.0, tx)));
        jobs.try_recv().unwrap();
        let read = jobs.try_recv().unwrap();
        rt.state
            .apply_reply(poll_reply(&read, Ok(ReplyData::Changes(changes()))));
        rt.update(|ed, tx| assert!(!poll(ed, true, true, 0.5, tx)));
        rt.update(|ed, _| kick(ed));
        rt.update(|ed, tx| assert!(poll(ed, true, true, 0.5, tx)));
        assert!(matches!(jobs.try_recv().unwrap().req, Req::ChangesPoll));
    }

    /// A reply built the way `poll` leaves the schedule: sent at clock 0.
    fn arm(ed: &mut EditorState) {
        ed.autorefresh.pending = true;
        ed.autorefresh.sent = Some(Instant::now());
        ed.autorefresh.sent_clock = 0.0;
        ed.autorefresh.checkout = Some("key".into());
    }

    fn quiet_reply(result: Result<ReplyData, String>) -> Reply {
        Reply {
            req: Req::ChangesPoll,
            generation: 0,
            checkout_key: Some("key".into()),
            result,
        }
    }

    #[test]
    fn quiet_reply_with_identical_data_enqueues_no_diff_and_no_spinner() {
        let (mut ed, jobs) = editor();
        ed.changes.data = Some(changes());
        ed.changes.layer = Some("unstaged".into());
        ed.changes.patch_path = Some("a.txt".into());
        arm(&mut ed);
        ed.apply_reply(quiet_reply(Ok(ReplyData::Changes(changes()))));
        assert!(!ed.autorefresh.pending);
        assert!(jobs.try_recv().is_err(), "identical data: no diff re-read");
        assert!(
            !ed.changes.loading && !ed.changes.patch_loading,
            "no spinner"
        );
        assert!(ed.changes.error.is_none() && ed.notice.is_none());
    }

    #[test]
    fn quiet_reply_with_changed_data_updates_changes_and_refreshes_the_selected_diff_once() {
        let (mut ed, jobs) = editor();
        ed.changes.data = Some(changes());
        ed.changes.layer = Some("unstaged".into());
        ed.changes.patch_path = Some("a.txt".into());
        ed.changes.patch = Some(crate::editor::state::Patch {
            label: "unstaged".into(),
            raw: "old patch".into(),
            truncated: false,
            binary: false,
            files: Vec::new(),
            detail: None,
            checkout: "key".into(),
        });
        arm(&mut ed);
        let mut data = changes();
        data.untracked = vec![StatusEntry {
            path: "b.txt".into(),
            orig_path: None,
            status: "untracked",
            added: None,
            removed: None,
        }];
        ed.apply_reply(quiet_reply(Ok(ReplyData::Changes(data))));
        assert_eq!(ed.changes.data.as_ref().unwrap().untracked.len(), 1);
        assert_eq!(ed.changes.checkout.as_deref(), Some("key"));
        let job = jobs.try_recv().unwrap();
        assert!(
            matches!(
                job.req,
                Req::Diff {
                    ref layer,
                    path: Some(ref path),
                    commit: None,
                } if layer == "unstaged" && path == "a.txt"
            ),
            "the selected diff re-read once"
        );
        assert!(jobs.try_recv().is_err(), "exactly once");
        assert!(
            !ed.changes.loading && !ed.changes.patch_loading,
            "no spinner"
        );
        assert!(
            ed.changes.patch.is_some(),
            "the old patch is not cleared while the new diff flies"
        );
    }

    #[test]
    fn quiet_failure_keeps_data_and_sets_no_notice() {
        let (mut ed, jobs) = editor();
        ed.changes.data = Some(changes());
        arm(&mut ed);
        ed.apply_reply(quiet_reply(Err("git exploded".into())));
        assert!(!ed.autorefresh.pending);
        assert!(ed.changes.data.is_some(), "the old data stays");
        assert!(
            ed.notice.is_none() && ed.changes.error.is_none(),
            "no notice, no error"
        );
        assert_eq!(ed.autorefresh.next_at, 3.0, "retry at the base cadence");
        assert!(jobs.try_recv().is_err());
    }

    #[test]
    fn slow_read_pushes_the_next_poll_out() {
        let (mut ed, _jobs) = editor();
        arm(&mut ed);
        ed.autorefresh.sent = Some(Instant::now() - Duration::from_secs(2));
        ed.apply_reply(quiet_reply(Ok(ReplyData::Changes(changes()))));
        assert!(
            ed.autorefresh.next_at >= 10.0 && ed.autorefresh.next_at < 10.5,
            "a 2 s read waits 10 s, got {}",
            ed.autorefresh.next_at
        );
        arm(&mut ed);
        ed.autorefresh.sent = Some(Instant::now() - Duration::from_secs(10));
        ed.apply_reply(quiet_reply(Ok(ReplyData::Changes(changes()))));
        assert_eq!(ed.autorefresh.next_at, 30.0, "capped at 30 s, not 50");
    }

    #[test]
    fn a_poll_reply_for_another_checkout_is_dropped() {
        let (mut ed, jobs) = editor();
        ed.changes.data = Some(changes());
        arm(&mut ed);
        ed.autorefresh.checkout = Some("other".into());
        ed.apply_reply(Reply {
            req: Req::ChangesPoll,
            generation: 0,
            checkout_key: Some("other".into()),
            result: Ok(ReplyData::Changes(GitViewChanges {
                branch: Some("foreign".into()),
                ..changes()
            })),
        });
        assert_eq!(
            ed.changes.data,
            Some(changes()),
            "a foreign reply lands nowhere"
        );
        // The next poll sees the key change and rebuilds: the stale
        // in-flight flag goes with it — asserted through the Files gate so
        // the reset is observed before the next read re-arms it.
        let mut rt = Runtime::new(ed, |_, _| rune::row(), (400.0, 100.0), Env::new(true, 2.0));
        rt.update(|ed, tx| {
            // Already active (no entry edge), so only the Files gate speaks.
            ed.autorefresh.was_active = true;
            ed.autorefresh.was_focused = true;
            ed.section = Section::Files;
            assert!(!poll(ed, true, true, 0.0, tx));
        });
        assert!(!rt.state.autorefresh.pending, "a switch resets pending");
        assert_eq!(rt.state.autorefresh.checkout.as_deref(), Some("key"));
        let _ = jobs;
    }

    fn text_file(text: &str) -> app_core::git_view::GitViewFile {
        app_core::git_view::GitViewFile {
            path: "a.txt".into(),
            kind: "text",
            text: Some(text.into()),
            truncated: false,
            size: text.len() as u64,
        }
    }

    /// The open file's own 3 s schedule: an immediate check on view entry,
    /// one per cadence tick, never two outstanding, and none while a save
    /// or reload owns the doc, while it is not complete text, or while the
    /// Files section is not showing.
    #[test]
    fn poll_checks_the_open_file_every_three_seconds_in_the_files_section() {
        let (ed, jobs) = editor();
        let mut rt = Runtime::new(ed, |_, _| rune::row(), (400.0, 100.0), Env::new(true, 2.0));
        rt.update(|ed, _| {
            ed.section = Section::Files;
            ed.doc = Some(crate::editor::draft::OpenDoc::new(
                "a.txt",
                &text_file("orig"),
            ));
        });
        // Entry: the file list refresh plus the disk check, due at once.
        rt.update(|ed, tx| assert!(poll(ed, true, true, 0.0, tx)));
        assert!(matches!(jobs.try_recv().unwrap().req, Req::FileList));
        let read = jobs.try_recv().unwrap();
        assert!(matches!(read.req, Req::CheckFile { .. }));
        rt.state
            .apply_reply(poll_reply(&read, Ok(ReplyData::File(text_file("orig")))));
        // The cadence holds: nothing before 3 s, exactly one probe at 3 s.
        rt.update(|ed, tx| assert!(!poll(ed, true, true, 2.9, tx)));
        assert!(jobs.try_recv().is_err(), "no check before the cadence");
        rt.update(|ed, tx| assert!(poll(ed, true, true, 3.0, tx)));
        let read = jobs.try_recv().unwrap();
        assert!(matches!(read.req, Req::CheckFile { .. }));
        rt.update(|ed, tx| assert!(!poll(ed, true, true, 6.0, tx)));
        assert!(jobs.try_recv().is_err(), "never two outstanding");
        rt.state
            .apply_reply(poll_reply(&read, Ok(ReplyData::File(text_file("orig")))));
        // The gates: a save or reload owns the doc's next state, and a
        // non-text or truncated doc has nothing the probe can apply.
        rt.update(|ed, tx| {
            ed.doc.as_mut().unwrap().saving = true;
            assert!(!poll(ed, true, true, 7.0, tx));
            ed.doc.as_mut().unwrap().saving = false;
            ed.doc_reload = Some(9);
            assert!(!poll(ed, true, true, 7.1, tx));
            ed.doc_reload = None;
            ed.doc.as_mut().unwrap().kind = crate::editor::draft::DocKind::Binary;
            assert!(!poll(ed, true, true, 7.2, tx));
            ed.doc.as_mut().unwrap().kind = crate::editor::draft::DocKind::Text;
            ed.doc.as_mut().unwrap().truncated = true;
            assert!(!poll(ed, true, true, 7.3, tx));
            ed.doc.as_mut().unwrap().truncated = false;
        });
        assert!(
            jobs.try_recv().is_err(),
            "saving, reload, binary and truncated stay quiet"
        );
        // The Changes section still reads Changes — never the open file.
        rt.update(|ed, tx| {
            ed.section = Section::Changes;
            assert!(poll(ed, true, true, 8.0, tx));
        });
        assert!(matches!(jobs.try_recv().unwrap().req, Req::ChangesPoll));
        assert!(jobs.try_recv().is_err(), "no file probe in Changes");
    }
}
