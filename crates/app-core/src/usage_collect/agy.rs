//! Antigravity: the quota Google's Cloud Code service reports for the sign-in
//! `agy` keeps.
//!
//! The sign-in lives in the OS keyring (Secret Service on Linux, the Keychain
//! on macOS) or, without one, in `antigravity-oauth-token` in agy's folder.
//! Three questions are asked: the plan (`loadCodeAssist`), the plan's quota
//! windows (`retrieveUserQuotaSummary`), and each model's own quota
//! (`retrieveUserQuota`), which is where the Gemini models' window is when the
//! summary lists only the third-party group.

use std::path::PathBuf;

use serde_json::{json, Value};

use super::{base64_decode, bearer, read_json, text_of, Env, HttpError, Outcome, Outside, Request};
use crate::usage_records::{UsageRecord, UsageWindow};

const CLOUD_CODE_URL: &str = "https://daily-cloudcode-pa.googleapis.com";

pub(super) fn collect(env: &Env, outside: &dyn Outside) -> Outcome {
    let Some(token) = access_token(env, outside) else {
        return Outcome::SignedOut;
    };
    // The bearer token goes wherever CLOUD_CODE_URL points, so a destination
    // named there is taken only over https; anything else is Google's own.
    let base = (env.var)("CLOUD_CODE_URL")
        .and_then(|url| url.into_string().ok())
        .map(|url| url.trim_end_matches('/').to_string())
        .filter(|url| url.starts_with("https://"))
        .unwrap_or_else(|| CLOUD_CODE_URL.to_string());
    let ask = |endpoint: &str, body: &Value| -> Result<Value, HttpError> {
        let mut headers = bearer(&token);
        headers.push(("Content-Type", "application/json".to_string()));
        headers.push(("User-Agent", "antigravity".to_string()));
        let answer = outside.http(&Request {
            method: "POST",
            url: format!("{base}/v1internal:{endpoint}"),
            headers,
            body: Some(body.to_string()),
        })?;
        serde_json::from_str(&answer).map_err(|_| HttpError::Transport)
    };
    // agy renews its access token only while it runs, so a refused one has
    // usually just lapsed: the last numbers stay until agy is started again.
    let Ok(assist) = ask("loadCodeAssist", &json!({})) else {
        return Outcome::Unavailable;
    };
    let project = project_body(&assist);
    let Ok(summary) = ask("retrieveUserQuotaSummary", &project) else {
        return Outcome::Unavailable;
    };
    let models = ask("retrieveUserQuota", &project).ok();
    let Some(limits) = limits(&summary, models.as_ref()).filter(|limits| !limits.is_empty()) else {
        return Outcome::Unavailable;
    };
    let tier = tier(&assist);
    Outcome::Record(UsageRecord {
        id: "agy".to_string(),
        name: "Antigravity".to_string(),
        tier_label: (!tier.is_empty()).then_some(tier),
        limits,
        ..Default::default()
    })
}

/// The access token of agy's sign-in: the keyring's, else its token file's.
fn access_token(env: &Env, outside: &dyn Outside) -> Option<String> {
    let lookup = if cfg!(target_os = "macos") {
        outside.run(
            "security",
            &[
                "find-generic-password",
                "-s",
                "gemini",
                "-a",
                "antigravity",
                "-w",
            ],
        )
    } else if cfg!(target_os = "linux") {
        outside.run(
            "secret-tool",
            &["lookup", "service", "gemini", "username", "antigravity"],
        )
    } else {
        None
    };
    let stored = lookup
        .and_then(|found| sign_in(&found))
        .or_else(|| read_json(&app_root(env)?.join("antigravity-oauth-token")))?;
    let nested = stored.get("token").filter(|token| token.is_object());
    let token = Some(text_of(nested.unwrap_or(&Value::Null), "access_token"))
        .filter(|token| !token.is_empty())
        .unwrap_or_else(|| text_of(&stored, "access_token"));
    (!token.is_empty()).then_some(token)
}

/// What a keyring holds: the sign-in as JSON, which some keyring libraries
/// store base64-encoded behind a prefix.
fn sign_in(found: &str) -> Option<Value> {
    let found = found.trim();
    let json = match found.strip_prefix("go-keyring-base64:") {
        Some(encoded) => String::from_utf8(base64_decode(encoded)?).ok()?,
        None => found.to_string(),
    };
    serde_json::from_str::<Value>(&json)
        .ok()
        .filter(Value::is_object)
}

fn app_root(env: &Env) -> Option<PathBuf> {
    env.path_var("AGY_DIR")
        .or_else(|| Some(env.home.as_ref()?.join(".gemini/antigravity-cli")))
}

/// The plan's name: a paid tier names the plan; without one, the current
/// tier does.
fn tier(assist: &Value) -> String {
    for key in ["paidTier", "currentTier"] {
        let Some(tier) = assist.get(key).filter(|tier| tier.is_object()) else {
            continue;
        };
        let (name, id) = (text_of(tier, "name"), text_of(tier, "id"));
        if name.is_empty() && id.is_empty() {
            continue;
        }
        let (lower_name, lower_id) = (name.to_lowercase(), id.to_lowercase());
        for word in ["Ultra", "Pro", "Free"] {
            let needle = word.to_lowercase();
            if lower_id.contains(&needle) || lower_name.contains(&needle) {
                return word.to_string();
            }
        }
        return match lower_name.strip_prefix("google ai ") {
            Some(_) => name["google ai ".len()..].trim().to_string(),
            None if !name.is_empty() => name,
            None => {
                let mut plan = id.replace("-tier", "");
                if let Some(first) = plan.get_mut(..1) {
                    first.make_ascii_uppercase();
                }
                plan
            }
        };
    }
    String::new()
}

/// The quota is asked for the project the plan's answer names, its id given
/// as a string or as `{id}`; an answer naming none leaves the body empty.
fn project_body(assist: &Value) -> Value {
    let project = match assist.get("cloudaicompanionProject") {
        Some(Value::String(id)) => id.trim(),
        Some(project) => project
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim(),
        None => "",
    };
    if project.is_empty() {
        json!({})
    } else {
        json!({ "project": project })
    }
}

/// How much of a bucket is spent, or `None` when it says nothing usable. A
/// bucket's remainder is a fraction or an amount; proto3 JSON leaves a zero
/// out, so a bucket with a reset time and neither has none left. A disabled
/// bucket counts against nothing.
fn used(bucket: &Value) -> Option<f64> {
    if bucket.get("disabled") == Some(&Value::Bool(true)) {
        return None;
    }
    let field = |camel: &str, snake: &str| bucket.get(camel).or_else(|| bucket.get(snake));
    let remaining = field("remainingFraction", "remaining_fraction").filter(|v| !v.is_null());
    let amount = field("remainingAmount", "remaining_amount").filter(|v| !v.is_null());
    let remaining = match (remaining, amount) {
        (Some(remaining), _) => match remaining {
            Value::Number(number) => number.as_f64()?,
            Value::String(text) => text.trim().parse().ok()?,
            _ => return None,
        },
        // An amount gives no share of the whole to show, unless it is zero.
        (None, Some(amount)) => {
            let spent = amount.as_f64() == Some(0.0) || amount.as_str() == Some("0");
            return spent.then_some(1.0);
        }
        (None, None) => return reset_of(bucket).map(|_| 1.0),
    };
    remaining
        .is_finite()
        .then(|| ((1.0 - remaining).clamp(0.0, 1.0) * 10_000.0).round() / 10_000.0)
}

fn reset_of(bucket: &Value) -> Option<String> {
    let reset = Some(text_of(bucket, "resetTime"))
        .filter(|reset| !reset.is_empty())
        .unwrap_or_else(|| text_of(bucket, "reset_time"));
    (!reset.is_empty()).then_some(reset)
}

/// Which window a summary bucket is.
#[derive(Clone, Copy, PartialEq)]
enum Window {
    Session,
    Weekly,
}

fn window_of(bucket: &Value) -> Option<Window> {
    let window = text_of(bucket, "window").to_lowercase();
    let id = text_of(bucket, "bucketId").to_lowercase();
    let name = text_of(bucket, "displayName").to_lowercase();
    if window == "5h" || id.contains("5h") || name.contains("five") {
        Some(Window::Session)
    } else if window == "weekly" || id.contains("weekly") || name.contains("week") {
        Some(Window::Weekly)
    } else {
        None
    }
}

fn window(title: &str, label: &str, bucket: &Value) -> Option<UsageWindow> {
    Some(UsageWindow {
        label: label.to_string(),
        percent: used(bucket)?,
        resets_at: reset_of(bucket),
        title: Some(title.to_string()),
    })
}

/// The plan's windows out of the quota summary and the per-model quota.
///
/// "Session" and "Weekly" are the plan's own: the summary's Gemini group
/// when it has one, else its first group. The Gemini models' own session
/// quota, when the per-model answer has it, is the session that matters:
/// the most-spent Gemini model stands for it. Other groups, and a first
/// group whose session is not the Gemini one, show once they are touched.
/// `None` when the summary is not an answer at all (an error, a wrong shape).
fn limits(summary: &Value, models: Option<&Value>) -> Option<Vec<UsageWindow>> {
    let listed = |value: &Value, key: &str| match value.get(key) {
        None | Some(Value::Null) | Some(Value::Array(_)) => true,
        Some(_) => false,
    };
    if !summary.is_object()
        || summary.get("error").is_some()
        || !listed(summary, "groups")
        || !listed(summary, "buckets")
    {
        return None;
    }
    let array = |value: &Value, key: &str| -> Vec<Value> {
        let items = value.get(key).and_then(Value::as_array);
        items
            .into_iter()
            .flatten()
            .filter(|item| item.is_object())
            .cloned()
            .collect()
    };
    let mut groups = array(summary, "groups");
    // An answer that lists its buckets without groups lists the Gemini ones.
    let flat = array(summary, "buckets");
    if groups.is_empty() && !flat.is_empty() {
        groups.push(json!({ "displayName": "Gemini", "buckets": flat }));
    }
    let is_gemini = |group: &Value| {
        text_of(group, "displayName")
            .to_lowercase()
            .contains("gemini")
    };
    let own = groups.iter().position(is_gemini).unwrap_or(0);

    // The most-spent Gemini model's own quota.
    let gemini_model = models
        .map(|models| array(models, "buckets"))
        .unwrap_or_default()
        .into_iter()
        .filter(|bucket| text_of(bucket, "modelId").to_lowercase().contains("gemini"))
        .filter(|bucket| reset_of(bucket).is_some())
        .filter_map(|bucket| Some((used(&bucket)?, bucket)))
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, bucket)| bucket);

    let mut limits = Vec::new();
    let mut extra = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        let name = text_of(group, "displayName");
        let short = if name.to_lowercase().contains("claude") {
            "Claude/GPT".to_string()
        } else if name.is_empty() {
            "3P".to_string()
        } else {
            name
        };
        let (mut session, mut weekly) = (None, None);
        for bucket in array(group, "buckets") {
            if used(&bucket).is_none() {
                continue;
            }
            match window_of(&bucket) {
                Some(Window::Session) => session = Some(bucket),
                Some(Window::Weekly) => weekly = Some(bucket),
                None => {}
            }
        }
        let touched = |bucket: &Option<Value>| bucket.as_ref().is_some_and(|b| used(b) > Some(0.0));
        if index != own {
            for (bucket, kind) in [(&session, "Session"), (&weekly, "Weekly")] {
                if touched(bucket) {
                    let title = format!("{short} {kind}");
                    extra.extend(window(&title, &title, bucket.as_ref()?));
                }
            }
            continue;
        }
        // The plan's own group. Its session gives way to the Gemini models'
        // own when the group is not the Gemini one, and then shows beside it
        // only once touched.
        match (&gemini_model, is_gemini(group)) {
            (Some(model), false) => {
                limits.extend(window("Session", "Session (5-hour)", model));
                if touched(&session) {
                    let title = format!("{short} Session");
                    extra.extend(window(&title, &title, session.as_ref()?));
                }
            }
            (Some(model), true) => {
                let spent = |bucket: &Value| used(bucket).unwrap_or(0.0);
                let fuller = session
                    .as_ref()
                    .filter(|session| spent(session) >= spent(model))
                    .unwrap_or(model);
                limits.extend(window("Session", "Session (5-hour)", fuller));
            }
            (None, _) => {
                if let Some(session) = &session {
                    limits.extend(window("Session", "Session (5-hour)", session));
                }
            }
        }
        if let Some(weekly) = &weekly {
            limits.extend(window("Weekly", "Weekly (7-day)", weekly));
        }
    }
    // No summary group at all still leaves the Gemini models' session.
    if groups.is_empty() {
        if let Some(model) = &gemini_model {
            limits.extend(window("Session", "Session (5-hour)", model));
        }
    }
    limits.extend(extra);
    Some(limits)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{env, write, Scripted};
    use super::*;
    use crate::test_support::TempDir;

    const ASSIST: &str = r#"{"cloudaicompanionProject":"proj-1",
        "paidTier":{"id":"g1-pro-tier","name":"Google AI Pro"},
        "currentTier":{"id":"free-tier","name":"Antigravity"}}"#;
    /// The shape a Pro account answers with: one third-party group, no
    /// Gemini group.
    const SUMMARY: &str = r#"{"description":"…","groups":[{"displayName":"Claude and GPT models",
        "buckets":[
          {"bucketId":"3p-weekly","displayName":"Weekly Limit Remaining","window":"weekly",
           "resetTime":"2026-11-20T12:19:23Z","remainingFraction":0.493979},
          {"bucketId":"3p-5h","displayName":"Five Hour Limit Remaining","window":"5h",
           "resetTime":"2026-11-17T12:47:17Z","remainingFraction":1}]}]}"#;
    const MODELS: &str = r#"{"buckets":[
        {"tokenType":"WTUS","modelId":"chat_20706","remainingFraction":1},
        {"resetTime":"2026-11-20T12:19:23Z","modelId":"claude-sonnet-4-6","remainingFraction":0.49},
        {"resetTime":"2026-11-17T12:47:17Z","modelId":"gemini-2.5-pro","remainingFraction":1},
        {"resetTime":"2026-11-17T13:00:00Z","modelId":"gemini-3.8-flash-tiered","remainingFraction":0.75},
        {"modelId":"gemini-no-reset","remainingFraction":0.1}]}"#;

    fn signed_in(home: &std::path::Path) {
        write(
            &home.join(".gemini/antigravity-cli/antigravity-oauth-token"),
            r#"{"auth_method":"consumer","token":{"access_token":"tok-agy","refresh_token":"r"}}"#,
        );
    }

    fn windows(limits: &[UsageWindow]) -> Vec<(&str, f64, Option<&str>)> {
        limits
            .iter()
            .map(|w| {
                (
                    w.title.as_deref().unwrap(),
                    w.percent,
                    w.resets_at.as_deref(),
                )
            })
            .collect()
    }

    #[test]
    fn the_three_answers_become_one_record() {
        let temp = TempDir::new("agy");
        signed_in(temp.path());
        let outside = Scripted::answering(&[
            (":loadCodeAssist", Ok(ASSIST)),
            (":retrieveUserQuotaSummary", Ok(SUMMARY)),
            (":retrieveUserQuota", Ok(MODELS)),
        ]);
        let Outcome::Record(record) = collect(&env(temp.path()), &outside) else {
            panic!("expected a record");
        };
        assert_eq!(
            (record.id.as_str(), record.name.as_str()),
            ("agy", "Antigravity")
        );
        assert_eq!(record.tier_label.as_deref(), Some("Pro"));
        // Session is the most-spent Gemini model, not the third-party 5h
        // bucket; the untouched third-party session is not shown.
        assert_eq!(
            windows(&record.limits),
            [
                ("Session", 0.25, Some("2026-11-17T13:00:00Z")),
                ("Weekly", 0.506, Some("2026-11-20T12:19:23Z")),
            ]
        );
        let requests = outside.requests();
        assert_eq!(requests.len(), 3);
        assert!(requests.iter().all(|request| {
            request.starts_with("POST https://daily-cloudcode-pa.googleapis.com/v1internal:")
                && request.contains("Authorization: Bearer tok-agy")
        }));
        assert!(requests[1].ends_with(r#"{"project":"proj-1"}"#));
    }

    #[test]
    fn a_touched_third_party_session_shows_beside_the_gemini_one() {
        let summary: Value = serde_json::from_str(
            &SUMMARY.replace(r#""remainingFraction":1}"#, r#""remainingFraction":0.9}"#),
        )
        .unwrap();
        let models: Value = serde_json::from_str(MODELS).unwrap();
        let limits = limits(&summary, Some(&models)).unwrap();
        assert_eq!(
            windows(&limits),
            [
                ("Session", 0.25, Some("2026-11-17T13:00:00Z")),
                ("Weekly", 0.506, Some("2026-11-20T12:19:23Z")),
                ("Claude/GPT Session", 0.1, Some("2026-11-17T12:47:17Z")),
            ]
        );
        // Without the per-model answer, the group's own session stands.
        assert_eq!(
            windows(&super::limits(&summary, None).unwrap())[0],
            ("Session", 0.1, Some("2026-11-17T12:47:17Z"))
        );
    }

    /// A Gemini group is the plan's own; other groups show once touched.
    #[test]
    fn a_gemini_group_is_the_plans_own() {
        let summary = json!({"groups": [
            {"displayName": "Claude and GPT models", "buckets": [
                {"window": "weekly", "resetTime": "2026-11-20T00:00:00Z", "remainingFraction": 0.8},
                {"window": "5h", "resetTime": "2026-11-17T00:00:00Z", "remainingFraction": 1}]},
            {"displayName": "Gemini models", "buckets": [
                {"window": "5h", "resetTime": "2026-11-17T01:00:00Z", "remainingFraction": 0.4},
                {"window": "weekly", "resetTime": "2026-11-21T00:00:00Z"},
                {"window": "weekly", "disabled": true, "remainingFraction": 0.1}]}]});
        assert_eq!(
            windows(&limits(&summary, None).unwrap()),
            [
                ("Session", 0.6, Some("2026-11-17T01:00:00Z")),
                // A reset time and no remainder: nothing left.
                ("Weekly", 1.0, Some("2026-11-21T00:00:00Z")),
                ("Claude/GPT Weekly", 0.2, Some("2026-11-20T00:00:00Z")),
            ]
        );
        assert_eq!(limits(&json!({"error": {"code": 403}}), None), None);
        assert_eq!(limits(&json!({"groups": "no"}), None), None);
        assert_eq!(limits(&json!({}), None), Some(Vec::new()));
    }

    #[test]
    fn no_sign_in_asks_nobody_and_a_lapsed_token_keeps_the_last_record() {
        let temp = TempDir::new("agy");
        let outside = Scripted::default();
        assert_eq!(collect(&env(temp.path()), &outside), Outcome::SignedOut);
        assert!(outside.requests().is_empty());

        signed_in(temp.path());
        let refused = Scripted::answering(&[(":loadCodeAssist", Err(HttpError::Status(401)))]);
        assert_eq!(collect(&env(temp.path()), &refused), Outcome::Unavailable);
        assert_eq!(refused.requests().len(), 1, "a refusal ends the run");
    }

    #[test]
    fn a_keyring_entry_reads_plain_or_base64() {
        let plain = r#"{"token":{"access_token":"a"}}"#;
        assert!(sign_in(plain).is_some());
        // "go-keyring-base64:" + base64 of the same JSON.
        let encoded = "go-keyring-base64:eyJ0b2tlbiI6eyJhY2Nlc3NfdG9rZW4iOiJhIn19";
        assert_eq!(sign_in(encoded), sign_in(plain));
        assert!(sign_in("not json").is_none());
    }

    #[test]
    fn the_tier_is_the_paid_plans_name() {
        let tier_of = |json: &str| tier(&serde_json::from_str(json).unwrap());
        assert_eq!(tier_of(ASSIST), "Pro");
        assert_eq!(
            tier_of(r#"{"currentTier":{"id":"free-tier","name":"Antigravity"}}"#),
            "Free"
        );
        assert_eq!(
            tier_of(r#"{"paidTier":{"name":"Google AI Business"}}"#),
            "Business"
        );
        assert_eq!(tier_of(r#"{"paidTier":{"id":"team-tier"}}"#), "Team");
        assert_eq!(tier_of("{}"), "");
    }
}
