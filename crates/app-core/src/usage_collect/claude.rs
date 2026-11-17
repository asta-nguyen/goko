//! Claude Code: the limits Anthropic reports for the sign-in Claude Code
//! keeps, the same numbers its own `/usage` shows.
//!
//! The sign-in is `<config>/.credentials.json` on Linux and Windows and the
//! login Keychain on macOS. The plan label comes from the profile Claude Code
//! keeps beside it.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{bearer, read_json, rfc3339_utc, text_of, Env, Outcome, Outside, Request};
use crate::usage_records::{UsageRecord, UsageWindow};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// The Keychain item Claude Code keeps its default sign-in under on macOS.
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

pub(super) fn collect(env: &Env, outside: &dyn Outside) -> Outcome {
    let Some(dir) = config_dir(env) else {
        return Outcome::SignedOut;
    };
    let Some(login) = login(env, &dir, outside) else {
        return Outcome::SignedOut;
    };
    let token = text_of(&login, "accessToken");
    if token.is_empty() {
        return Outcome::SignedOut;
    }
    let mut headers = bearer(&token);
    headers.push(("anthropic-beta", "oauth-2025-04-20".to_string()));
    let request = Request {
        method: "GET",
        url: USAGE_URL.to_string(),
        headers,
        body: None,
    };
    // A refused token has usually just lapsed: Claude Code renews it the next
    // time it runs, so the last numbers stay rather than vanish.
    let Ok(body) = outside.http(&request) else {
        return Outcome::Unavailable;
    };
    let Ok(payload) = serde_json::from_str::<Value>(&body) else {
        return Outcome::Unavailable;
    };
    let limits = limits(&payload);
    if limits.is_empty() {
        return Outcome::Unavailable;
    }
    let plan = plan_label(
        &tier(env, &dir, &login),
        &text_of(&login, "subscriptionType"),
    );
    Outcome::Record(UsageRecord {
        id: "claude".to_string(),
        name: "Claude Code".to_string(),
        tier_label: (!plan.is_empty()).then_some(plan),
        limits,
        ..Default::default()
    })
}

/// The config folder of the account in use.
fn config_dir(env: &Env) -> Option<PathBuf> {
    env.claude_dir
        .clone()
        .or_else(|| env.path_var("CLAUDE_CONFIG_DIR"))
        .or_else(|| default_dir(env))
}

fn default_dir(env: &Env) -> Option<PathBuf> {
    env.home.as_ref().map(|home| home.join(".claude"))
}

/// The `claudeAiOauth` object of the sign-in: the credentials file where
/// there is one, else macOS's Keychain for the default account.
fn login(env: &Env, dir: &Path, outside: &dyn Outside) -> Option<Value> {
    let stored = read_json(&dir.join(".credentials.json")).or_else(|| {
        if !cfg!(target_os = "macos") || Some(dir) != default_dir(env).as_deref() {
            return None;
        }
        let found = outside.run(
            "security",
            &["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"],
        )?;
        serde_json::from_str(&found).ok()
    })?;
    stored
        .get("claudeAiOauth")
        .filter(|v| v.is_object())
        .cloned()
}

/// The rate-limit tier. The credentials carry the tier their token was minted
/// with and an upgrade does not restamp it, so the profile Claude Code
/// refreshes is asked first, and outranks them only when it names a
/// multiplier.
fn tier(env: &Env, dir: &Path, login: &Value) -> String {
    let profile = read_json(&profile_path(env, dir)).unwrap_or_default();
    let account = profile.get("oauthAccount").cloned().unwrap_or_default();
    let stated = ["userRateLimitTier", "organizationRateLimitTier"]
        .into_iter()
        .map(|key| text_of(&account, key))
        .find(|tier| !tier.is_empty())
        .unwrap_or_default();
    if multiplier(&stated).is_some() {
        stated
    } else {
        text_of(login, "rateLimitTier")
    }
}

/// Claude Code's profile for a config folder: a legacy `.config.json` inside
/// it, else `~/.claude.json` for the default folder and `<dir>/.claude.json`
/// for one it was pointed at.
fn profile_path(env: &Env, dir: &Path) -> PathBuf {
    let legacy = dir.join(".config.json");
    if legacy.exists() {
        return legacy;
    }
    let pointed_here = env.path_var("CLAUDE_CONFIG_DIR").as_deref() == Some(dir);
    match &env.home {
        Some(home) if dir == home.join(".claude") && !pointed_here => home.join(".claude.json"),
        _ => dir.join(".claude.json"),
    }
}

/// The `5x` of `default_claude_max_5x`.
fn multiplier(tier: &str) -> Option<&str> {
    let lower = tier.to_ascii_lowercase();
    let start = lower.find("max_")? + 4;
    let digits = lower[start..]
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    let end = start + digits;
    (digits > 0 && lower[end..].starts_with('x')).then(|| &tier[start..=end])
}

/// The rate-limit tier names the ceiling, not the plan: a Team seat can run
/// on a Max 5x tier. The subscription leads and the multiplier qualifies it:
/// "Max 5x", "Team 5x", "Pro".
fn plan_label(tier: &str, subscription: &str) -> String {
    let mut plan = subscription.to_string();
    if let Some(first) = plan.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    match multiplier(tier) {
        Some(times) if plan.is_empty() => format!("Max {times}"),
        Some(times) => format!("{plan} {times}"),
        None => plan,
    }
}

/// Every window in the usage answer: the account's session and week, then
/// each model-scoped allowance (a weekly window only one model draws from).
fn limits(payload: &Value) -> Vec<UsageWindow> {
    let mut limits = Vec::new();
    // `seven_day` is the account's week; `seven_day_oauth_apps` is a side
    // bucket Claude Code never shows as the weekly limit.
    for (key, label, title) in [
        ("five_hour", "Session (5-hour)", "Session"),
        ("seven_day", "Weekly (7-day)", "Weekly"),
    ] {
        let Some(bucket) = payload.get(key).filter(|bucket| bucket.is_object()) else {
            continue;
        };
        if let Some(percent) = utilization(bucket.get("utilization")) {
            limits.push(UsageWindow {
                label: label.to_string(),
                percent,
                resets_at: reset_at(bucket.get("resets_at")),
                title: Some(title.to_string()),
            });
        }
    }
    let scoped = payload.get("limits").and_then(Value::as_array);
    let mut seen: Vec<(String, String)> = Vec::new();
    for entry in scoped.into_iter().flatten() {
        let Some(model) = entry.pointer("/scope/model").filter(|m| m.is_object()) else {
            continue;
        };
        let name = Some(text_of(model, "display_name"))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| text_of(model, "id"));
        let kind = text_of(entry, "kind");
        if name.is_empty() || seen.contains(&(name.clone(), kind.clone())) {
            continue;
        }
        let Some(percent) = utilization(entry.get("percent")) else {
            continue;
        };
        seen.push((name.clone(), kind.clone()));
        let window = scoped_window(&kind);
        let title = if window.is_empty() {
            name
        } else {
            format!("{name} {window}")
        };
        limits.push(UsageWindow {
            label: title.clone(),
            percent,
            resets_at: reset_at(entry.get("resets_at")),
            title: Some(title),
        });
    }
    limits
}

/// Every utilization this endpoint answers is a percentage (0.6 is 0.6%, not
/// 60%), as a number or as text. `None` when it is neither, or negative.
fn utilization(value: Option<&Value>) -> Option<f64> {
    let percent = match value? {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => text.trim().trim_end_matches('%').trim().parse().ok()?,
        _ => return None,
    };
    (percent >= 0.0).then(|| (percent / 100.0).min(1.0))
}

/// A reset time as RFC 3339: the answer's own text, or an epoch in seconds
/// or milliseconds.
fn reset_at(value: Option<&Value>) -> Option<String> {
    let raw = match value? {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    if raw.is_empty() {
        return None;
    }
    match raw.parse::<i64>() {
        Ok(epoch) if epoch >= 1_000_000_000_000 => rfc3339_utc(epoch / 1000),
        Ok(epoch) => rfc3339_utc(epoch),
        Err(_) => Some(raw),
    }
}

/// The window a scoped entry's `kind` names ("weekly_scoped",
/// "five_hour_scoped"), settled here so a model name like "Opus (1M
/// context)" is never read as a window.
fn scoped_window(kind: &str) -> &'static str {
    let kind = kind.to_ascii_lowercase();
    if kind.contains("month") {
        "Monthly"
    } else if kind.contains("week") || kind.contains("day") {
        "Weekly"
    } else if kind.contains("hour") || kind.contains("session") {
        "Session"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{env, write, Scripted};
    use super::super::HttpError;
    use super::*;
    use crate::test_support::TempDir;

    const ANSWER: &str = r#"{
        "five_hour": {"utilization": 29.0, "resets_at": "2026-11-16T14:20:00.073441+00:00"},
        "seven_day": {"utilization": "37%", "resets_at": 1795150800},
        "seven_day_oauth_apps": {"utilization": 99.0, "resets_at": null},
        "seven_day_opus": null,
        "limits": [
            {"kind": "weekly_scoped", "percent": 53, "resets_at": "2026-11-20T05:00:00+00:00",
             "scope": {"model": {"id": "claude-fable-5", "display_name": "Fable"}}},
            {"kind": "weekly_scoped", "percent": 53,
             "scope": {"model": {"id": "claude-fable-5", "display_name": "Fable"}}},
            {"kind": "weekly", "percent": 37, "scope": null},
            {"kind": "five_hour_scoped", "percent": -1,
             "scope": {"model": {"id": "claude-opus-5"}}}
        ]
    }"#;

    fn signed_in(home: &Path) {
        write(
            &home.join(".claude/.credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"tok-claude","subscriptionType":"max",
                "rateLimitTier":"default_claude_max_5x","expiresAt":1}}"#,
        );
    }

    #[test]
    fn the_usage_answer_becomes_session_week_and_scoped_windows() {
        let temp = TempDir::new("claude");
        signed_in(temp.path());
        // The profile names the upgraded tier the credentials still lack.
        write(
            &temp.path().join(".claude.json"),
            r#"{"oauthAccount":{"organizationRateLimitTier":"default_claude_max_20x"}}"#,
        );
        let outside = Scripted::answering(&[("/api/oauth/usage", Ok(ANSWER))]);
        let Outcome::Record(record) = collect(&env(temp.path()), &outside) else {
            panic!("expected a record");
        };
        assert_eq!(record.id, "claude");
        assert_eq!(record.tier_label.as_deref(), Some("Max 20x"));
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
                ("Session", 0.29, Some("2026-11-16T14:20:00.073441+00:00")),
                ("Weekly", 0.37, Some("2026-11-20T05:00:00Z")),
                ("Fable Weekly", 0.53, Some("2026-11-20T05:00:00+00:00")),
            ]
        );
        // The token went to Anthropic's usage endpoint and nowhere else.
        assert_eq!(
            outside.requests(),
            [
                "GET https://api.anthropic.com/api/oauth/usage | Authorization: Bearer tok-claude \
              | Accept: application/json | anthropic-beta: oauth-2025-04-20"
            ]
        );
    }

    #[test]
    fn no_sign_in_asks_nobody_and_a_refusal_keeps_the_last_record() {
        let temp = TempDir::new("claude");
        let outside = Scripted::default();
        assert_eq!(collect(&env(temp.path()), &outside), Outcome::SignedOut);
        assert!(outside.requests().is_empty());

        signed_in(temp.path());
        for answer in [
            Err(HttpError::Status(401)),
            Err(HttpError::Transport),
            Ok("{}"),
        ] {
            let outside = Scripted::answering(&[("/api/oauth/usage", answer)]);
            assert_eq!(collect(&env(temp.path()), &outside), Outcome::Unavailable);
        }
    }

    /// A named account reads its own folder's sign-in and profile.
    #[test]
    fn the_active_accounts_folder_is_the_one_read() {
        let temp = TempDir::new("claude");
        signed_in(temp.path());
        let work = temp.path().join("accounts/work");
        write(
            &work.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"tok-work","subscriptionType":"team"}}"#,
        );
        let mut env = env(temp.path());
        env.claude_dir = Some(work);
        let outside = Scripted::answering(&[("/api/oauth/usage", Ok(ANSWER))]);
        let Outcome::Record(record) = collect(&env, &outside) else {
            panic!("expected a record");
        };
        assert_eq!(record.tier_label.as_deref(), Some("Team"));
        assert!(outside.requests()[0].contains("Bearer tok-work"));
    }

    #[test]
    fn the_plan_label_leads_with_the_subscription() {
        assert_eq!(plan_label("default_claude_max_5x", "max"), "Max 5x");
        assert_eq!(plan_label("default_claude_max_20x", "team"), "Team 20x");
        assert_eq!(plan_label("default_claude_max_5x", ""), "Max 5x");
        assert_eq!(plan_label("default_claude_ai", "pro"), "Pro");
        assert_eq!(plan_label("", ""), "");
        assert_eq!(multiplier("max_x"), None);
    }
}
