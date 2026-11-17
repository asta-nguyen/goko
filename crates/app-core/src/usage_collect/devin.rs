//! Devin: the quota in the user-status cache the Devin CLI writes every time
//! it starts (`user_status.*.bin`), the same data its `/usage` renders. No
//! request is made; the numbers are as fresh as Devin's last start.
//!
//! The cache is JSON holding the `GetUserStatus` answer as base64 protobuf;
//! only the handful of fields the quota lives in are decoded.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use super::{base64_decode, read_json, rfc3339_utc, Env, Outcome, Outside};
use crate::usage_records::{UsageRecord, UsageWindow};

/// The CLI rewrites its credentials on sign-in, so a status fetched before
/// that describes the account signed in before. The new account's first
/// fetch can land a beat before the file is flushed, hence the allowance.
const SIGN_IN_ALLOWANCE_SECS: i64 = 30;

pub(super) fn collect(env: &Env, _outside: &dyn Outside) -> Outcome {
    let cutoff = sign_in_time(env).map(|at| at - SIGN_IN_ALLOWANCE_SECS);
    let mut best: Option<(i64, Quota)> = None;
    for dir in status_dirs(env) {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            let name = path.file_name().and_then(|name| name.to_str());
            if !name.is_some_and(|n| n.starts_with("user_status.") && n.ends_with(".bin")) {
                continue;
            }
            let Some(cache) = read_json(&path) else {
                continue;
            };
            let Some(fetched_at) = cache.get("fetched_at_secs").and_then(|v| v.as_f64()) else {
                continue;
            };
            let fetched_at = fetched_at as i64;
            // One cache per sign-in and no marker of which is active: the
            // newest fetch under the current sign-in wins.
            if cutoff.is_some_and(|cutoff| fetched_at < cutoff)
                || best.as_ref().is_some_and(|(at, _)| *at >= fetched_at)
            {
                continue;
            }
            let quota = cache
                .get("payload")
                .and_then(|payload| payload.as_str())
                .and_then(base64_decode)
                .map(|payload| parse_user_status(&payload));
            if let Some(quota) = quota.filter(|quota| !quota.limits.is_empty()) {
                best = Some((fetched_at, quota));
            }
        }
    }
    match best {
        Some((_, quota)) => Outcome::Record(UsageRecord {
            id: "devin".to_string(),
            name: "Devin".to_string(),
            tier_label: (!quota.tier.is_empty()).then_some(quota.tier),
            limits: quota.limits,
            ..Default::default()
        }),
        None => Outcome::SignedOut,
    }
}

/// Where the CLI may keep its cache: where it is told to, the XDG cache
/// folder it uses on Linux, and each OS's own cache folder.
fn status_dirs(env: &Env) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = env.path_var("DEVIN_CACHE_DIR").into_iter().collect();
    let caches = [
        env.xdg("XDG_CACHE_HOME", ".cache"),
        env.home.as_ref().map(|home| home.join("Library/Caches")),
        env.path_var("LOCALAPPDATA"),
    ];
    for cache in caches.into_iter().flatten() {
        dirs.push(cache.join("devin/cli"));
    }
    dirs
}

/// When the current account signed in: the credentials file's last write.
fn sign_in_time(env: &Env) -> Option<i64> {
    let data_dirs = [
        env.path_var("DEVIN_DATA_DIR")
            .and_then(|dir| dir.parent().map(PathBuf::from)),
        env.xdg("XDG_DATA_HOME", ".local/share")
            .map(|data| data.join("devin")),
        env.home
            .as_ref()
            .map(|home| home.join("Library/Application Support/devin")),
        env.path_var("APPDATA").map(|data| data.join("devin")),
    ];
    data_dirs.into_iter().flatten().find_map(|dir| {
        let modified = fs::metadata(dir.join("credentials.toml"))
            .ok()?
            .modified()
            .ok()?;
        Some(modified.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64)
    })
}

#[derive(Debug, PartialEq)]
struct Quota {
    tier: String,
    limits: Vec<UsageWindow>,
}

/// One protobuf field's first value.
#[derive(Clone)]
enum Field {
    Varint(u64),
    Bytes(Vec<u8>),
}

/// A message's fields by number, first value each. A truncated message
/// yields the fields read before the cut.
fn proto_fields(data: &[u8]) -> HashMap<u64, Field> {
    let mut found = HashMap::new();
    let mut pos = 0;
    while pos < data.len() {
        let Some(tag) = varint_at(data, &mut pos) else {
            break;
        };
        let value = match tag & 0x07 {
            0 => match varint_at(data, &mut pos) {
                Some(value) => Some(Field::Varint(value)),
                None => break,
            },
            1 => {
                pos += 8;
                None
            }
            2 => {
                let Some(len) = varint_at(data, &mut pos) else {
                    break;
                };
                let Some(end) = usize::try_from(len)
                    .ok()
                    .and_then(|len| pos.checked_add(len))
                    .filter(|end| *end <= data.len())
                else {
                    break;
                };
                let bytes = data[pos..end].to_vec();
                pos = end;
                Some(Field::Bytes(bytes))
            }
            5 => {
                pos += 4;
                None
            }
            _ => break,
        };
        if let Some(value) = value {
            found.entry(tag >> 3).or_insert(value);
        }
    }
    found
}

fn varint_at(data: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *data.get(*pos)?;
        *pos += 1;
        result |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(result);
        }
    }
    None
}

fn message(fields: &HashMap<u64, Field>, number: u64) -> HashMap<u64, Field> {
    match fields.get(&number) {
        Some(Field::Bytes(bytes)) => proto_fields(bytes),
        _ => HashMap::new(),
    }
}

/// A varint field as the signed number protobuf means by it.
fn varint(fields: &HashMap<u64, Field>, number: u64) -> Option<i64> {
    match fields.get(&number) {
        Some(Field::Varint(value)) => Some(*value as i64),
        _ => None,
    }
}

fn text(fields: &HashMap<u64, Field>, number: u64) -> String {
    match fields.get(&number) {
        Some(Field::Bytes(bytes)) => String::from_utf8_lossy(bytes).trim().to_string(),
        _ => String::new(),
    }
}

/// The plan tiers `GetUserStatus` numbers, named as Devin names them. Only
/// asked when the plan carries no name of its own.
fn tier_name(number: Option<i64>) -> &'static str {
    match number {
        Some(1) => "Teams",
        Some(2) => "Pro",
        Some(3) => "Enterprise Saas",
        Some(4) => "Hybrid",
        Some(5) => "Enterprise Self Hosted",
        Some(6) => "Waitlist Pro",
        Some(7) => "Teams Ultimate",
        Some(8) => "Pro Ultimate",
        Some(9) => "Trial",
        Some(10) => "Enterprise Self Serve",
        Some(11) => "Enterprise Saas Pooled",
        Some(12) => "Devin Enterprise",
        Some(14) => "Devin Teams",
        Some(15) => "Devin Teams V2",
        Some(16) => "Devin Pro",
        Some(17) => "Devin Max",
        Some(18) => "Max",
        Some(19) => "Devin Free",
        Some(20) => "Devin Trial",
        _ => "",
    }
}

fn parse_user_status(payload: &[u8]) -> Quota {
    let status = proto_fields(payload);
    let plan_status = message(&status, 13);
    let plan = message(&plan_status, 1);
    let mut tier = text(&plan, 2);
    if tier.is_empty() {
        tier = [varint(&plan, 1), varint(&status, 10)]
            .into_iter()
            .map(tier_name)
            .find(|name| !name.is_empty())
            .unwrap_or_default()
            .to_string();
    }

    let mut limits = Vec::new();
    if !plan_status.is_empty() {
        // A credit plan leaves the quota percents at their defaults, so a
        // window only shows when the server dates its reset or the plan
        // bills by quota, and never when the plan hides it.
        let quota_plan = varint(&plan, 35) == Some(2);
        for (label, hidden, remaining, reset) in [("Daily", 36, 14, 17), ("Weekly", 37, 15, 18)] {
            if varint(&plan, hidden).unwrap_or(0) != 0 {
                continue;
            }
            let reset = varint(&plan_status, reset).unwrap_or(0);
            if reset > 0 || quota_plan {
                // The percent is what is left; proto3 leaves a zero off the
                // wire, so a window with nothing left carries no percent.
                let left = varint(&plan_status, remaining).unwrap_or(0).clamp(0, 100);
                limits.push(UsageWindow {
                    label: label.to_string(),
                    percent: (100 - left) as f64 / 100.0,
                    resets_at: rfc3339_utc(reset),
                    title: None,
                });
            }
        }
        // Monthly credit buckets exist only on credit plans and last until
        // the plan's period ends.
        let plan_end = varint(&message(&plan_status, 3), 1).and_then(rfc3339_utc);
        for (label, limit_field, used_field) in [
            ("Prompt credits", 12, 6),
            ("Flow credits", 13, 5),
            ("Flex credits", 14, 7),
        ] {
            let limit = varint(&plan, limit_field).unwrap_or(0);
            if limit > 0 {
                let used = varint(&plan_status, used_field).unwrap_or(0).max(0);
                limits.push(UsageWindow {
                    label: label.to_string(),
                    percent: (used as f64 / limit as f64).min(1.0),
                    resets_at: plan_end.clone(),
                    title: None,
                });
            }
        }
    }
    Quota { tier, limits }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{env, write, Scripted};
    use super::*;
    use crate::test_support::TempDir;

    fn put_varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 {
            out.push(value as u8 | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }

    fn varint_field(out: &mut Vec<u8>, number: u64, value: u64) {
        put_varint(out, number << 3);
        put_varint(out, value);
    }

    fn bytes_field(out: &mut Vec<u8>, number: u64, bytes: &[u8]) {
        put_varint(out, number << 3 | 2);
        put_varint(out, bytes.len() as u64);
        out.extend_from_slice(bytes);
    }

    fn base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// A quota plan named "Pro": 100% of the day left, 85% of the week.
    fn status(weekly_left: u64, weekly_reset: u64) -> Vec<u8> {
        let mut plan = Vec::new();
        bytes_field(&mut plan, 2, b"Pro");
        varint_field(&mut plan, 35, 2);
        let mut plan_status = Vec::new();
        bytes_field(&mut plan_status, 1, &plan);
        varint_field(&mut plan_status, 14, 100);
        varint_field(&mut plan_status, 15, weekly_left);
        varint_field(&mut plan_status, 18, weekly_reset);
        let mut status = Vec::new();
        // A fixed64 and a fixed32 the parser must step over.
        put_varint(&mut status, 2 << 3 | 1);
        status.extend_from_slice(&[0; 8]);
        put_varint(&mut status, 3 << 3 | 5);
        status.extend_from_slice(&[0; 4]);
        bytes_field(&mut status, 13, &plan_status);
        status
    }

    fn cache(fetched_at: i64, payload: &[u8]) -> String {
        format!(
            r#"{{"fetched_at_secs":{fetched_at},"payload":"{}"}}"#,
            base64(payload)
        )
    }

    #[test]
    fn the_status_cache_becomes_daily_and_weekly_windows() {
        let quota = parse_user_status(&status(85, 1_700_600_000));
        assert_eq!(quota.tier, "Pro");
        let windows: Vec<(&str, f64, Option<&str>)> = quota
            .limits
            .iter()
            .map(|w| (w.label.as_str(), w.percent, w.resets_at.as_deref()))
            .collect();
        assert_eq!(
            windows,
            [
                ("Daily", 0.0, None),
                ("Weekly", 0.15, Some("2023-11-21T20:53:20Z")),
            ]
        );
        // Garbage and a cut-off message decode to nothing rather than panic.
        assert!(parse_user_status(&[0xff; 12]).limits.is_empty());
        let whole = status(85, 1_700_600_000);
        let _ = parse_user_status(&whole[..whole.len() - 3]);
    }

    /// The newest fetch wins; one from before the current sign-in describes
    /// another account and is never shown.
    #[test]
    fn the_newest_fetch_under_the_current_sign_in_wins() {
        let temp = TempDir::new("devin");
        let env = env(temp.path());
        assert_eq!(collect(&env, &Scripted::default()), Outcome::SignedOut);

        let dir = temp.path().join(".cache/devin/cli");
        write(
            &dir.join("user_status.aaaa.bin"),
            &cache(1_000, &status(40, 1_700_600_000)),
        );
        write(
            &dir.join("user_status.bbbb.bin"),
            &cache(2_000, &status(85, 1_700_600_000)),
        );
        write(&dir.join("user_status.cccc.bin"), "not json");
        write(
            &dir.join("model_configs.bbbb.bin"),
            &cache(9_000, &status(1, 1)),
        );
        let Outcome::Record(record) = collect(&env, &Scripted::default()) else {
            panic!("expected a record");
        };
        assert_eq!(
            (record.id.as_str(), record.name.as_str()),
            ("devin", "Devin")
        );
        assert_eq!(record.tier_label.as_deref(), Some("Pro"));
        assert_eq!(record.limits[1].percent, 0.15);

        // Signing in now makes both fetches the previous account's.
        write(&temp.path().join(".local/share/devin/credentials.toml"), "");
        assert_eq!(collect(&env, &Scripted::default()), Outcome::SignedOut);
    }
}
