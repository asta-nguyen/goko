//! Editor tabs: the open files, their order and the parked (inactive)
//! documents. `EditorState.doc` stays the ACTIVE document — this is the
//! pure state around that slot: no I/O, no rune/UI types beyond `OpenDoc`.
//! No limit on the number of tabs and no eviction.

use std::collections::HashMap;

use crate::editor::draft::OpenDoc;

/// The tab set around the active-document slot. Invariants: every parked
/// path is in `order` and `recent`; the active path is in both but never
/// in `parked`; `recent` leads with the active path.
#[derive(Default)]
pub struct Tabs {
    /// Every open path in open order, the active one included.
    pub order: Vec<String>,
    /// Paths most recently used first, the active one first.
    pub recent: Vec<String>,
    /// The inactive documents by path.
    pub parked: HashMap<String, OpenDoc>,
}

impl Tabs {
    /// Install `doc` as the active document: the current one (if any) is
    /// parked, and whatever was parked under `doc.path` is replaced. The
    /// path joins `order` if absent and leads `recent`.
    pub fn open(&mut self, slot: &mut Option<OpenDoc>, doc: OpenDoc) {
        let path = doc.path.clone();
        if let Some(previous) = slot.replace(doc) {
            self.parked.insert(previous.path.clone(), previous);
        }
        self.parked.remove(&path);
        if !self.order.contains(&path) {
            self.order.push(path.clone());
        }
        self.recent.retain(|p| p != &path);
        self.recent.insert(0, path);
    }

    /// Make the parked tab `path` the active document, parking the current
    /// one. `false` when `path` is already active or not a tab.
    pub fn activate(&mut self, slot: &mut Option<OpenDoc>, path: &str) -> bool {
        let Some(target) = self.parked.remove(path) else {
            return false;
        };
        if let Some(previous) = slot.replace(target) {
            self.parked.insert(previous.path.clone(), previous);
        }
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_string());
        true
    }

    /// Remove the tab `path` from `order`, `recent` and `parked`. Closing
    /// the active tab activates its right neighbour in `order` (as it was
    /// before removal), else the left one, else leaves the slot empty.
    /// Returns the newly active path — `None` when the slot ends empty or
    /// an inactive tab was closed.
    pub fn close(&mut self, slot: &mut Option<OpenDoc>, path: &str) -> Option<String> {
        let index = self.order.iter().position(|p| p == path)?;
        let was_active = slot.as_ref().is_some_and(|d| d.path == path);
        self.order.remove(index);
        self.recent.retain(|p| p != path);
        self.parked.remove(path);
        if !was_active {
            return None;
        }
        *slot = None;
        let next = self
            .order
            .get(index)
            .or_else(|| index.checked_sub(1).and_then(|i| self.order.get(i)))
            .cloned()?;
        self.activate(slot, &next).then_some(next)
    }

    /// Activate the tab `delta` positions away from the active one in
    /// `order`, wrapping. Returns the new active path; `None` with fewer
    /// than two tabs.
    pub fn step(&mut self, slot: &mut Option<OpenDoc>, delta: i32) -> Option<String> {
        if self.order.len() < 2 {
            return None;
        }
        let active = slot.as_ref().map(|d| d.path.as_str())?;
        let index = self.order.iter().position(|p| p == active)?;
        let next_index = (index as i32 + delta).rem_euclid(self.order.len() as i32) as usize;
        let next = self.order[next_index].clone();
        self.activate(slot, &next).then_some(next)
    }

    /// The most recently used path other than `active` — the tab `⌃Tab`
    /// would jump to. `None` with one or no tabs.
    pub fn previous_recent(&self, active: &str) -> Option<String> {
        self.recent.iter().find(|p| p.as_str() != active).cloned()
    }

    /// Dirty tabs in `order` order, the active document included.
    pub fn dirty_paths(&self, slot: &Option<OpenDoc>) -> Vec<String> {
        self.order
            .iter()
            .filter(|path| {
                slot.as_ref().is_some_and(|d| d.path == **path && d.dirty())
                    || self.parked.get(path.as_str()).is_some_and(|d| d.dirty())
            })
            .cloned()
            .collect()
    }

    /// `true` while `path` is an open tab (active or parked).
    pub fn is_open(&self, path: &str) -> bool {
        self.order.iter().any(|p| p == path)
    }

    /// The parked document at `path`, when the tab is inactive.
    pub fn parked_doc(&self, path: &str) -> Option<&OpenDoc> {
        self.parked.get(path)
    }

    /// Forget every tab and drop the parked documents. The active slot is
    /// untouched — its owner decides what becomes of it.
    pub fn clear(&mut self) {
        self.order.clear();
        self.recent.clear();
        self.parked.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(text: &str) -> app_core::git_view::GitViewFile {
        app_core::git_view::GitViewFile {
            path: "x".into(),
            kind: "text",
            text: Some(text.into()),
            truncated: false,
            size: text.len() as u64,
        }
    }

    fn doc(path: &str, text: &str) -> OpenDoc {
        OpenDoc::new(path, &file(text))
    }

    /// Buffer text, caret and dirty state survive being parked, whichever
    /// way the document got parked (`open` or `activate`).
    #[test]
    fn open_parks_the_active_document_and_keeps_every_buffer() {
        let mut tabs = Tabs::default();
        let mut slot = None;

        tabs.open(&mut slot, doc("a", "aaa"));
        slot.as_mut().unwrap().code.edit.insert("X"); // Xaaa, dirty
        slot.as_mut().unwrap().code.edit.set_selection(1, 2);
        tabs.open(&mut slot, doc("b", "bbb")); // a parked
        slot.as_mut().unwrap().code.edit.insert("Y"); // Ybbb, dirty
        slot.as_mut().unwrap().code.edit.set_selection(0, 1);
        tabs.open(&mut slot, doc("c", "ccc")); // b parked

        for (path, text, dirty, range) in [
            ("a", "Xaaa", true, (1, 2)),
            ("b", "Ybbb", true, (0, 1)),
            ("c", "ccc", false, (0, 0)),
        ] {
            assert!(tabs.activate(&mut slot, path), "{path} activates");
            let d = slot.as_ref().unwrap();
            assert_eq!(d.code.edit.text, text, "{path} text");
            assert_eq!(d.code.edit.range(), range, "{path} caret");
            assert_eq!(d.dirty(), dirty, "{path} dirty");
        }
    }

    #[test]
    fn close_activates_the_right_neighbour_then_the_left() {
        let mut tabs = Tabs::default();
        let mut slot = None;
        for p in ["a", "b", "c"] {
            tabs.open(&mut slot, doc(p, p));
        }
        tabs.activate(&mut slot, "b");
        assert_eq!(tabs.close(&mut slot, "b").as_deref(), Some("c"));
        assert_eq!(slot.as_ref().unwrap().path, "c");
        // No right neighbour left: the left one activates.
        assert_eq!(tabs.close(&mut slot, "c").as_deref(), Some("a"));
        assert_eq!(slot.as_ref().unwrap().path, "a");
        // The only tab leaves the slot empty.
        assert_eq!(tabs.close(&mut slot, "a"), None);
        assert!(slot.is_none());

        // An inactive tab closes without touching the active one.
        tabs.open(&mut slot, doc("x", "x"));
        tabs.open(&mut slot, doc("y", "y"));
        tabs.activate(&mut slot, "x");
        assert_eq!(tabs.close(&mut slot, "y"), None);
        assert_eq!(slot.as_ref().unwrap().path, "x");
        assert!(!tabs.is_open("y"));
        assert_eq!(tabs.order, vec!["x".to_string()]);
    }

    #[test]
    fn step_wraps_in_both_directions_and_needs_two_tabs() {
        let mut tabs = Tabs::default();
        let mut slot = None;
        assert_eq!(tabs.step(&mut slot, 1), None);
        tabs.open(&mut slot, doc("a", "a"));
        assert_eq!(tabs.step(&mut slot, 1), None, "one tab cannot step");
        tabs.open(&mut slot, doc("b", "b"));
        tabs.open(&mut slot, doc("c", "c")); // order [a,b,c], active c
        assert_eq!(
            tabs.step(&mut slot, 1).as_deref(),
            Some("a"),
            "wraps the end"
        );
        assert_eq!(tabs.step(&mut slot, 1).as_deref(), Some("b"));
        assert_eq!(
            tabs.step(&mut slot, -1).as_deref(),
            Some("a"),
            "wraps the start"
        );
        assert_eq!(tabs.step(&mut slot, -1).as_deref(), Some("c"));
    }

    #[test]
    fn previous_recent_toggles_between_two_tabs() {
        let mut tabs = Tabs::default();
        let mut slot = None;
        tabs.open(&mut slot, doc("a", "a"));
        assert_eq!(tabs.previous_recent("a"), None, "one tab → none");
        tabs.open(&mut slot, doc("b", "b"));
        tabs.activate(&mut slot, "a");
        assert_eq!(tabs.previous_recent("a").as_deref(), Some("b"));
        tabs.activate(&mut slot, "b");
        assert_eq!(tabs.previous_recent("b").as_deref(), Some("a"));
    }

    #[test]
    fn dirty_paths_lists_dirty_tabs_in_tab_order() {
        let mut tabs = Tabs::default();
        let mut slot = None;
        tabs.open(&mut slot, doc("a", "a"));
        slot.as_mut().unwrap().code.edit.insert("1");
        tabs.open(&mut slot, doc("b", "b")); // a parked dirty
        tabs.open(&mut slot, doc("c", "c"));
        slot.as_mut().unwrap().code.edit.insert("3"); // active dirty
        assert_eq!(
            tabs.dirty_paths(&slot),
            vec!["a".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn twenty_tabs_are_all_kept() {
        let mut tabs = Tabs::default();
        let mut slot = None;
        for i in 0..20 {
            tabs.open(&mut slot, doc(&format!("f{i:02}"), "x"));
        }
        assert_eq!(tabs.order.len(), 20);
        assert_eq!(tabs.parked.len(), 19, "all but the active are parked");
        for i in 0..20 {
            assert!(tabs.is_open(&format!("f{i:02}")), "f{i:02} stays open");
        }
    }

    #[test]
    fn reopening_a_parked_path_replaces_it_and_keeps_one_tab() {
        let mut tabs = Tabs::default();
        let mut slot = None;
        tabs.open(&mut slot, doc("a", "old"));
        tabs.open(&mut slot, doc("b", "b"));
        tabs.open(&mut slot, doc("a", "new"));
        assert_eq!(
            tabs.order,
            vec!["a".to_string(), "b".to_string()],
            "no duplicate tab"
        );
        assert!(tabs.parked_doc("a").is_none());
        assert_eq!(tabs.parked_doc("b").unwrap().code.edit.text, "b");
        let active = slot.as_ref().unwrap();
        assert_eq!(active.path, "a");
        assert_eq!(active.code.edit.text, "new", "the reopen buffer wins");
        assert_eq!(tabs.recent[0], "a");
    }
}
