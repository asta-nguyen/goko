//! Per-provider usage records: quota windows a collector wrote to disk, one
//! display-ready JSON file per provider (`<id>.json`).
//!
//! Goko collects its own on every OS: [`crate::usage`] reads Claude and Codex
//! from what their sessions leave behind, and [`crate::usage_collect`] asks
//! each signed-in agent's provider and writes a record here. A record can
//! also come from any other collector that writes this format:
//!
//! ```json
//! {
//!   "id": "devin",
//!   "name": "Devin",
//!   "tierLabel": "Pro",
//!   "limits": [
//!     {"label": "Weekly", "percent": 0.15, "resetsAt": "2026-11-22T08:00:00+00:00"}
//!   ],
//!   "resetCredits": {"available": 3, "nextExpiresAt": "2026-11-30T18:36:59+00:00"},
//!   "todaySessions": 1
//! }
//! ```
//!
//! Only `limits` is required; unknown fields are ignored. [`record_dirs`]
//! names where records are looked for: `GOKO_HOME/agents/usage` (anyone's to
//! write), `GOKO_HOME/usage/collected` (Goko's collectors), and the state
//! folder Omarchy's collectors write on the machines that run them. No OS
//! needs the last one.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Deserializer, Serialize};

/// A record is a few kilobytes; anything past this is not one and is skipped
/// rather than read.
const MAX_RECORD_BYTES: u64 = 1024 * 1024;

/// Where Goko's own collectors keep their records, under `GOKO_HOME`.
pub const COLLECTED_DIR: &str = "usage/collected";

/// How long a record still describes the account. A fresher record of the
/// same provider, from any folder, replaces one older than this.
pub const FRESH_MS: i64 = 15 * 60 * 1000;

/// One quota window of a provider.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct UsageWindow {
    #[serde(default)]
    pub label: String,
    /// The used share as a fraction: 0.42 is 42%.
    #[serde(default = "no_percent", deserialize_with = "percent")]
    pub percent: f64,
    /// When the window resets, RFC 3339; empty or absent when unknown.
    #[serde(default, rename = "resetsAt")]
    pub resets_at: Option<String>,
    /// A shorter name for the window, shown instead of `label` when present.
    #[serde(default)]
    pub title: Option<String>,
}

/// Free quota resets a provider grants (Codex).
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct ResetCredits {
    #[serde(default)]
    pub available: u32,
    #[serde(default, rename = "nextExpiresAt")]
    pub next_expires_at: Option<String>,
}

/// One provider's usage, as read from its `<id>.json`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct UsageRecord {
    /// The provider id (`claude`, `codex`, `devin`, …); the file's name when
    /// the record does not say.
    #[serde(default)]
    pub id: String,
    /// The provider's display name; the id when the record does not say.
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "tierLabel")]
    pub tier_label: Option<String>,
    #[serde(default)]
    pub limits: Vec<UsageWindow>,
    #[serde(default, rename = "resetCredits")]
    pub reset_credits: Option<ResetCredits>,
    #[serde(default, rename = "todaySessions")]
    pub today_sessions: Option<u64>,
    /// When the record was written (its file's modification time), epoch
    /// milliseconds. Not part of the file.
    #[serde(skip)]
    pub collected_at_ms: Option<i64>,
}

impl UsageRecord {
    /// Whether the record was written within [`FRESH_MS`] of `now_ms`.
    pub fn is_fresh(&self, now_ms: i64) -> bool {
        self.collected_at_ms
            .is_some_and(|at| now_ms - at <= FRESH_MS)
    }
}

fn no_percent() -> f64 {
    f64::NAN
}

/// A missing or `null` percentage reads as "not a number", which
/// [`read_record`] drops: it must never draw as an empty meter.
fn percent<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    Ok(Option::<f64>::deserialize(deserializer)?.unwrap_or(f64::NAN))
}

/// Where usage records are looked for, in the order [`read`] prefers them:
/// the folder anyone may write, Goko's own collectors' folder, then Omarchy's
/// state folder (`$XDG_STATE_HOME`, else `$HOME/.local/state`) where there is
/// one. Takes the environment as values so tests never touch the real one.
pub fn record_dirs(
    goko_home: &Path,
    xdg_state_home: Option<OsString>,
    home: Option<OsString>,
) -> Vec<PathBuf> {
    let mut dirs = vec![
        goko_home.join("agents/usage"),
        goko_home.join(COLLECTED_DIR),
    ];
    let state = xdg_state_home
        .map(PathBuf::from)
        .filter(|state| state.is_absolute())
        .or_else(|| {
            home.filter(|home| !home.is_empty())
                .map(|home| PathBuf::from(home).join(".local/state"))
        });
    if let Some(state) = state {
        dirs.push(state.join("omarchy/agents/usage"));
    }
    dirs
}

/// Every provider's record from `dirs`, one per provider. Among the folders
/// holding a provider, the earliest one whose record is fresh wins; when none
/// is fresh, the newest record does, so a collector that stopped never hides
/// one that still runs. What the winner does not say (tier, free resets,
/// today's sessions) is filled in from the others. Records with no usable
/// window are left out. Never fails: the usage poll calls this every tick.
pub fn read(dirs: &[PathBuf], now_ms: i64) -> Vec<UsageRecord> {
    // Each provider's candidates, in folder order.
    let mut found: Vec<(String, Vec<UsageRecord>)> = Vec::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
            .collect();
        // `read_dir` has no order of its own.
        paths.sort();
        for path in paths {
            let Some(record) = read_record(&path) else {
                continue;
            };
            match found.iter_mut().find(|(id, _)| *id == record.id) {
                Some((_, candidates)) => candidates.push(record),
                None => found.push((record.id.clone(), vec![record])),
            }
        }
    }
    found
        .into_iter()
        .map(|(_, candidates)| pick(candidates, now_ms))
        .collect()
}

/// The record to show out of one provider's candidates (in folder order),
/// with its gaps filled from the rest.
fn pick(mut candidates: Vec<UsageRecord>, now_ms: i64) -> UsageRecord {
    let best = candidates
        .iter()
        .position(|record| record.is_fresh(now_ms))
        .unwrap_or_else(|| {
            let newest = candidates
                .iter()
                .map(|record| record.collected_at_ms.unwrap_or(i64::MIN))
                .max();
            candidates
                .iter()
                .position(|record| Some(record.collected_at_ms.unwrap_or(i64::MIN)) == newest)
                .unwrap_or(0)
        });
    let mut record = candidates.remove(best);
    for other in candidates {
        record.tier_label = record.tier_label.or(other.tier_label);
        record.reset_credits = record.reset_credits.or(other.reset_credits);
        record.today_sessions = record.today_sessions.or(other.today_sessions);
    }
    record
}

/// One record, or `None` when the file is not a record or has no window with
/// a real percentage.
fn read_record(path: &Path) -> Option<UsageRecord> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_RECORD_BYTES {
        return None;
    }
    let mut record: UsageRecord = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    record
        .limits
        .retain(|window| window.percent.is_finite() && window.percent >= 0.0);
    if record.limits.is_empty() {
        return None;
    }
    if record.id.is_empty() {
        record.id = path.file_stem()?.to_str()?.to_string();
    }
    if record.name.is_empty() {
        record.name = record.id.clone();
    }
    record.tier_label = record.tier_label.filter(|tier| !tier.is_empty());
    record.collected_at_ms = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|age| i64::try_from(age.as_millis()).ok());
    Some(record)
}

/// Writes `record` as `<dir>/<id>.json`, whole or not at all: a reader never
/// sees half a record.
pub fn write(dir: &Path, record: &UsageRecord) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let json = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    let temp = dir.join(format!(".{}.tmp", record.id));
    fs::write(&temp, json)?;
    fs::rename(&temp, dir.join(format!("{}.json", record.id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::time::{Duration, SystemTime};

    const NOW_MS: i64 = 1_700_000_000_000;

    /// Writes `json` as `dir/file`, last modified `age_secs` before `NOW_MS`.
    fn write_aged(dir: &Path, file: &str, json: &str, age_secs: u64) {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(file);
        fs::write(&path, json).unwrap();
        let modified = SystemTime::UNIX_EPOCH + Duration::from_millis(NOW_MS as u64)
            - Duration::from_secs(age_secs);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    fn write_json(dir: &Path, file: &str, json: &str) {
        write_aged(dir, file, json, 0);
    }

    #[test]
    fn a_record_reads_its_windows_tier_credits_and_sessions() {
        let temp = TempDir::new("records");
        write_json(
            temp.path(),
            "codex.json",
            r#"{"schemaVersion":1,"id":"codex","name":"Codex","tierLabel":"Plus",
                "limits":[{"label":"5h window","percent":0.07,"resetsAt":"2026-11-16T12:32:45+00:00"},
                          {"label":"Weekly (7-day)","title":"Weekly","percent":0.47,"resetsAt":""}],
                "resetCredits":{"available":3,"nextExpiresAt":"2026-11-30T18:36:59+00:00"},
                "todaySessions":1,"recentDays":[{"date":"2026-11-10","messageCount":4}]}"#,
        );
        let records = read(&[temp.path().to_path_buf()], NOW_MS);
        assert_eq!(records.len(), 1);
        let codex = &records[0];
        assert_eq!((codex.id.as_str(), codex.name.as_str()), ("codex", "Codex"));
        assert_eq!(codex.tier_label.as_deref(), Some("Plus"));
        assert_eq!(codex.limits.len(), 2);
        assert_eq!(codex.limits[0].percent, 0.07);
        assert_eq!(codex.limits[1].title.as_deref(), Some("Weekly"));
        assert_eq!(codex.reset_credits.as_ref().unwrap().available, 3);
        assert_eq!(codex.today_sessions, Some(1));
        assert_eq!(codex.collected_at_ms, Some(NOW_MS));
    }

    /// A window without a real percentage is dropped, never drawn as an empty
    /// meter; a record left with no window is not a record.
    #[test]
    fn windows_without_a_real_percentage_are_dropped() {
        let temp = TempDir::new("records");
        let dir = temp.path();
        write_json(
            dir,
            "devin.json",
            r#"{"id":"devin","limits":[{"label":"Daily"},{"label":"Odd","percent":null},
                {"label":"Minus","percent":-0.2},{"label":"Weekly","percent":0.15}]}"#,
        );
        write_json(dir, "cursor.json", r#"{"id":"cursor","limits":[]}"#);
        write_json(
            dir,
            "grok.json",
            r#"{"id":"grok","limits":[{"label":"Daily"}]}"#,
        );
        write_json(dir, "broken.json", "{not json");
        write_json(dir, "notes.txt", r#"{"id":"x","limits":[{"percent":0.5}]}"#);
        let records = read(&[dir.to_path_buf()], NOW_MS);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "devin");
        let labels: Vec<&str> = records[0].limits.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["Weekly"]);
    }

    #[test]
    fn a_record_without_a_name_or_id_takes_them_from_its_file() {
        let temp = TempDir::new("records");
        write_json(
            temp.path(),
            "ollama.json",
            r#"{"limits":[{"label":"Monthly","percent":0.0}]}"#,
        );
        let records = read(&[temp.path().to_path_buf()], NOW_MS);
        assert_eq!(
            (records[0].id.as_str(), records[0].name.as_str()),
            ("ollama", "ollama")
        );
    }

    /// Fresh records: the earliest folder wins, and a record there with no
    /// usable window does not hide a later folder's.
    #[test]
    fn the_earliest_folder_with_a_fresh_record_wins() {
        let temp = TempDir::new("records");
        let own = temp.path().join("own");
        let other = temp.path().join("other");
        write_json(
            &own,
            "agy.json",
            r#"{"id":"agy","limits":[{"label":"Weekly","percent":0.5}]}"#,
        );
        write_json(&own, "claude.json", r#"{"id":"claude","limits":[]}"#);
        write_json(
            &other,
            "agy.json",
            r#"{"id":"agy","limits":[{"label":"Weekly","percent":0.9}]}"#,
        );
        write_json(
            &other,
            "claude.json",
            r#"{"id":"claude","limits":[{"label":"Weekly","percent":0.3}]}"#,
        );
        let missing = temp.path().join("missing");
        let records = read(&[missing, own, other], NOW_MS);
        let read: Vec<(&str, f64)> = records
            .iter()
            .map(|r| (r.id.as_str(), r.limits[0].percent))
            .collect();
        assert_eq!(read, [("agy", 0.5), ("claude", 0.3)]);
    }

    /// A collector that stopped must not hide one that still runs: a stale
    /// record loses to a fresh one in a later folder, and among stale ones
    /// the newest stands.
    #[test]
    fn a_stale_record_gives_way_to_a_fresher_one() {
        let temp = TempDir::new("records");
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let record = |percent: f64| {
            format!(r#"{{"id":"agy","limits":[{{"label":"Weekly","percent":{percent}}}]}}"#)
        };
        write_aged(&first, "agy.json", &record(0.1), 3_600);
        write_aged(&second, "agy.json", &record(0.2), 60);
        let dirs = [first.clone(), second.clone()];
        assert_eq!(read(&dirs, NOW_MS)[0].limits[0].percent, 0.2);

        // Both stale: the newer of the two.
        write_aged(&second, "agy.json", &record(0.3), 7_200);
        assert_eq!(read(&dirs, NOW_MS)[0].limits[0].percent, 0.1);
    }

    /// The winner keeps its own windows and takes only what it lacks.
    #[test]
    fn the_winner_fills_its_gaps_from_the_other_records() {
        let temp = TempDir::new("records");
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        write_json(
            &first,
            "codex.json",
            r#"{"id":"codex","name":"Codex","limits":[{"label":"Weekly","percent":0.4}]}"#,
        );
        write_json(
            &second,
            "codex.json",
            r#"{"id":"codex","tierLabel":"Plus","todaySessions":2,
                "resetCredits":{"available":3},"limits":[{"label":"Weekly","percent":0.9}]}"#,
        );
        let records = read(&[first, second], NOW_MS);
        assert_eq!(records.len(), 1);
        let codex = &records[0];
        assert_eq!(codex.limits[0].percent, 0.4);
        assert_eq!(codex.tier_label.as_deref(), Some("Plus"));
        assert_eq!(codex.today_sessions, Some(2));
        assert_eq!(codex.reset_credits.as_ref().unwrap().available, 3);
    }

    #[test]
    fn a_written_record_reads_back_the_same() {
        let temp = TempDir::new("records");
        let record = UsageRecord {
            id: "devin".to_string(),
            name: "Devin".to_string(),
            tier_label: Some("Pro".to_string()),
            limits: vec![UsageWindow {
                label: "Weekly".to_string(),
                percent: 0.15,
                resets_at: Some("2026-11-22T08:00:00Z".to_string()),
                title: None,
            }],
            ..Default::default()
        };
        write(temp.path(), &record).unwrap();
        let read_back = read(&[temp.path().to_path_buf()], NOW_MS);
        assert_eq!(read_back.len(), 1, "the temp file is not a record");
        let mut read_back = read_back[0].clone();
        assert!(read_back.collected_at_ms.is_some());
        read_back.collected_at_ms = None;
        assert_eq!(read_back, record);
    }

    /// Goko's own folders need no other variable; Omarchy's follows
    /// `XDG_STATE_HOME` when that is absolute.
    #[test]
    fn record_dirs_start_with_gokos_own_folders() {
        let goko = Path::new("/goko");
        let own = [
            PathBuf::from("/goko/agents/usage"),
            PathBuf::from("/goko/usage/collected"),
        ];
        assert_eq!(record_dirs(goko, None, None), own);
        let dirs = record_dirs(goko, None, Some("/home/me".into()));
        assert_eq!(dirs[..2], own);
        assert_eq!(
            dirs[2],
            PathBuf::from("/home/me/.local/state/omarchy/agents/usage")
        );
        assert_eq!(
            record_dirs(goko, Some("/state".into()), Some("/home/me".into()))[2],
            PathBuf::from("/state/omarchy/agents/usage")
        );
        assert_eq!(
            record_dirs(goko, Some("relative".into()), Some("/home/me".into()))[2],
            PathBuf::from("/home/me/.local/state/omarchy/agents/usage")
        );
    }
}
