//! OpenCode: the meters of an OpenCode Go subscription, asked with the key
//! OpenCode keeps in its `auth.json` (or one named in the environment).
//!
//! Zen pay-as-you-go has no meters: a key without a Go plan behind it is
//! answered 403, which is no trouble and no record.

use serde_json::Value;

use super::{bearer, read_json, rfc3339_utc, Env, HttpError, Outcome, Outside, Request};
use crate::usage_records::{UsageRecord, UsageWindow};

const USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";
/// Only the subscription's own provider ids: a custom gateway named like
/// "opencode-local" holds a key for somewhere else, never to be sent here.
const PROVIDERS: [&str; 2] = ["opencode", "opencode-go"];

pub(super) fn collect(env: &Env, outside: &dyn Outside) -> Outcome {
    let keys = keys(env);
    if keys.is_empty() {
        return Outcome::SignedOut;
    }
    let mut outcome = Outcome::SignedOut;
    for key in keys {
        let mut headers = bearer(&key);
        // The service turns away requests that do not say who is asking.
        headers.push(("User-Agent", "goko-usage/1.0".to_string()));
        let request = Request {
            method: "GET",
            url: USAGE_URL.to_string(),
            headers,
            body: None,
        };
        match outside.http(&request) {
            Ok(body) => {
                let limits = serde_json::from_str::<Value>(&body)
                    .map(|payload| limits(&payload, env.now_secs))
                    .unwrap_or_default();
                if limits.is_empty() {
                    return Outcome::Unavailable;
                }
                return Outcome::Record(UsageRecord {
                    id: "opencode".to_string(),
                    name: "OpenCode".to_string(),
                    tier_label: Some("Go".to_string()),
                    limits,
                    ..Default::default()
                });
            }
            // A refused key may just be the wrong one for this account, and
            // one without a plan has no meters: the next key gets its turn.
            Err(HttpError::Status(401 | 403)) => {}
            Err(_) => outcome = Outcome::Unavailable,
        }
    }
    outcome
}

/// The keys to try, an explicit one first, each once.
fn keys(env: &Env) -> Vec<String> {
    let mut keys: Vec<String> = ["OPENCODE_API_KEY", "OPENCODE_GO_API_KEY"]
        .into_iter()
        .filter_map(|name| (env.var)(name)?.into_string().ok())
        .collect();
    let stores = [
        env.xdg("XDG_DATA_HOME", ".local/share"),
        env.path_var("LOCALAPPDATA"),
    ];
    for store in stores.into_iter().flatten() {
        let Some(auth) = read_json(&store.join("opencode/auth.json")) else {
            continue;
        };
        for provider in PROVIDERS {
            // An entry is a `{type, key}` object, or the key as plain text.
            let key = match auth.get(provider) {
                Some(Value::String(key)) => key.as_str(),
                Some(entry) => entry.get("key").and_then(Value::as_str).unwrap_or(""),
                None => "",
            };
            keys.push(key.to_string());
        }
    }
    let mut unique: Vec<String> = Vec::new();
    for key in keys {
        let key = key.trim();
        // A `$VAR` or `!command` entry is resolved by its owner, never sent.
        if !key.is_empty() && !key.starts_with(['!', '$']) && !unique.iter().any(|k| k == key) {
            unique.push(key.to_string());
        }
    }
    unique
}

/// The windows of a usage answer. The live endpoint nests them under `usage`
/// with `percent` and `resetsAt`; another deployment spells them
/// `<window>Usage` with `usagePercent` and `resetInSec`. Both are read.
fn limits(payload: &Value, now_secs: i64) -> Vec<UsageWindow> {
    let usage = payload.get("usage").filter(|usage| usage.is_object());
    let mut limits = Vec::new();
    for (name, label, title) in [
        ("rolling", "Rolling (5h)", "Session"),
        ("weekly", "Weekly (7-day)", "Weekly"),
        ("monthly", "Monthly", "Monthly"),
    ] {
        let window = usage
            .unwrap_or(payload)
            .get(name)
            .filter(|window| window.is_object())
            .or_else(|| payload.get(format!("{name}Usage")))
            .filter(|window| window.is_object());
        let Some(window) = window else {
            continue;
        };
        let Some(percent) = fraction(window.get("percent").or(window.get("usagePercent"))) else {
            continue;
        };
        // A rate-limited window is spent until it resets, whatever it reads.
        let limited = window.get("status").and_then(Value::as_str) == Some("rate-limited");
        let iso_reset = window
            .get("resetsAt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|reset| reset.contains('T'))
            .map(str::to_string);
        let reset = iso_reset.or_else(|| {
            let seconds = window.get("resetInSec")?.as_f64()?;
            (seconds > 0.0).then(|| rfc3339_utc(now_secs + seconds as i64))?
        });
        limits.push(UsageWindow {
            label: label.to_string(),
            percent: if limited { 1.0 } else { percent },
            resets_at: reset,
            title: Some(title.to_string()),
        });
    }
    limits
}

/// A percentage (0–100) as a fraction; `None` when it is not a number.
fn fraction(value: Option<&Value>) -> Option<f64> {
    let percent = match value? {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => text.trim().trim_end_matches('%').trim().parse().ok()?,
        _ => return None,
    };
    (percent.is_finite() && percent >= 0.0).then(|| (percent / 100.0).min(1.0))
}

#[cfg(test)]
mod tests {
    use super::super::testing::{env, write, Scripted};
    use super::*;
    use crate::test_support::TempDir;

    fn signed_in(home: &std::path::Path) {
        write(
            &home.join(".local/share/opencode/auth.json"),
            r#"{"opencode":{"type":"api","key":"key-zen"},"opencode-go":"key-go",
                "opencode-local":{"key":"key-elsewhere"},"anthropic":{"key":"key-other"}}"#,
        );
    }

    #[test]
    fn a_go_plan_answers_with_its_windows() {
        let temp = TempDir::new("opencode");
        signed_in(temp.path());
        let answer = r#"{"usage":{
            "rolling":{"percent":12.5,"resetsAt":"2026-11-17T12:00:00Z"},
            "weekly":{"percent":40,"status":"rate-limited","resetsAt":"2026-11-20T00:00:00Z"},
            "monthly":{"percent":"7%"}}}"#;
        let outside = Scripted::answering(&[("/zen/go/v1/usage", Ok(answer))]);
        let Outcome::Record(record) = collect(&env(temp.path()), &outside) else {
            panic!("expected a record");
        };
        let windows: Vec<(&str, f64, Option<&str>)> = record
            .limits
            .iter()
            .map(|w| {
                (
                    w.title.as_deref().unwrap(),
                    w.percent,
                    w.resets_at.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            windows,
            [
                ("Session", 0.125, Some("2026-11-17T12:00:00Z")),
                ("Weekly", 1.0, Some("2026-11-20T00:00:00Z")),
                ("Monthly", 0.07, None),
            ]
        );
        // Only the subscription's own key was sent, and only to OpenCode.
        let requests = outside.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET https://opencode.ai/zen/go/v1/usage"));
        assert!(requests[0].contains("Bearer key-zen"));
    }

    #[test]
    fn the_other_spelling_of_the_answer_reads_too() {
        let payload = serde_json::from_str(
            r#"{"rollingUsage":{"usagePercent":50,"resetInSec":3600},"weeklyUsage":{"usagePercent":null}}"#,
        )
        .unwrap();
        let limits = limits(&payload, 1_700_000_000);
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].percent, 0.5);
        assert_eq!(limits[0].resets_at.as_deref(), Some("2023-11-14T23:13:20Z"));
    }

    /// No key asks nobody; keys with no Go plan behind them leave no record;
    /// an unreachable service keeps the last one.
    #[test]
    fn a_key_without_a_plan_leaves_no_record() {
        let temp = TempDir::new("opencode");
        let outside = Scripted::default();
        assert_eq!(collect(&env(temp.path()), &outside), Outcome::SignedOut);
        assert!(outside.requests().is_empty());

        signed_in(temp.path());
        let no_plan = Scripted::answering(&[("/zen/go/v1/usage", Err(HttpError::Status(403)))]);
        assert_eq!(collect(&env(temp.path()), &no_plan), Outcome::SignedOut);
        let sent = no_plan.requests();
        assert_eq!(sent.len(), 2, "each of the subscription's keys, once");
        assert!(!sent
            .iter()
            .any(|r| r.contains("key-elsewhere") || r.contains("key-other")));

        let offline = Scripted::default();
        assert_eq!(collect(&env(temp.path()), &offline), Outcome::Unavailable);
    }
}
