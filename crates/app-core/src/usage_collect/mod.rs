//! Goko's own usage collectors: one per agent it launches, each working the
//! same way on macOS, Linux and Windows.
//!
//! A collector finds the agent's own sign-in on this machine, asks the
//! agent's provider how much of each quota window is spent (Devin's answer
//! is already on disk), and returns a [`UsageRecord`]. [`Collector::collect`]
//! writes the records to `GOKO_HOME/usage/collected`, where
//! [`crate::usage_records::read`] finds them.
//!
//! Three rules hold for every collector:
//!
//! - A sign-in token goes nowhere but the `Authorization` header of the
//!   request to its own provider, over https. It is never logged, never
//!   written, and never passed on a command line.
//! - Nothing here signs in or refreshes a token: each agent keeps its own
//!   sign-in current while it runs. A lapsed one means no fresh answer until
//!   the agent is started again.
//! - A collector never fails the caller. No sign-in removes the provider's
//!   record; a provider that cannot be reached leaves the last one in place.
//!
//! Codex has no collector: `crate::usage` reads its limits from the rollout
//! files Codex writes, with no request at all.

mod agy;
mod claude;
mod devin;
mod opencode;

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::usage_records::{self, UsageRecord, COLLECTED_DIR};

/// How soon a provider may be asked again when nobody pressed Refresh: an
/// opened dropdown must not turn into a request per open.
const ASK_AGAIN: Duration = Duration::from_secs(300);
/// How soon a provider may be asked again when someone did.
const ASK_AGAIN_FORCED: Duration = Duration::from_secs(30);
/// The longest one request may take.
const REQUEST_SECONDS: &str = "10";
/// The longest a credential-store lookup may take; a locked keyring asks the
/// user and would otherwise hang the run.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);

/// One https request a collector makes.
pub struct Request {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Option<String>,
}

/// Why a request has no answer to read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HttpError {
    /// The provider answered with this status instead of the data.
    Status(u16),
    /// No answer at all: no route, no DNS, no `curl`.
    Transport,
}

/// What a collector reaches outside the process for, so tests reach for
/// nothing.
pub trait Outside: Sync {
    /// The body of a 2xx answer.
    fn http(&self, request: &Request) -> Result<String, HttpError>;
    /// What a credential-store lookup printed, when it found something.
    fn run(&self, program: &str, args: &[&str]) -> Option<String>;
}

/// What one collector found.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// The provider's current numbers.
    Record(UsageRecord),
    /// No sign-in for this agent on this machine, so no record is kept.
    SignedOut,
    /// Signed in, but no fresh answer; the last record stays as it is.
    Unavailable,
}

/// Where a collector looks. Everything is a value so tests name their own
/// folders and never read the real environment.
pub struct Env {
    pub goko_home: PathBuf,
    /// The user's home folder ([`crate::home::user_home`]).
    pub home: Option<PathBuf>,
    /// The active Claude account's config folder, when the caller knows it;
    /// otherwise `CLAUDE_CONFIG_DIR`, else `~/.claude`.
    pub claude_dir: Option<PathBuf>,
    /// Reads one environment variable.
    pub var: fn(&str) -> Option<OsString>,
    /// Now, epoch seconds.
    pub now_secs: i64,
}

impl Env {
    /// The process's own environment.
    pub fn from_process(goko_home: PathBuf, claude_dir: Option<PathBuf>) -> Self {
        Self {
            goko_home,
            home: crate::home::user_home().map(PathBuf::from),
            claude_dir,
            var: |name| std::env::var_os(name).filter(|value| !value.is_empty()),
            now_secs: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |now| now.as_secs() as i64),
        }
    }

    /// A variable naming a folder or file.
    fn path_var(&self, name: &str) -> Option<PathBuf> {
        (self.var)(name).map(PathBuf::from)
    }

    /// `$XDG_<kind>_HOME`, else the folder under home it defaults to.
    fn xdg(&self, variable: &str, default: &str) -> Option<PathBuf> {
        self.path_var(variable)
            .filter(|path| path.is_absolute())
            .or_else(|| self.home.as_ref().map(|home| home.join(default)))
    }
}

struct Provider {
    id: &'static str,
    /// Whether collecting asks the provider over the network, and so is
    /// spaced out.
    network: bool,
    collect: fn(&Env, &dyn Outside) -> Outcome,
}

const PROVIDERS: [Provider; 4] = [
    Provider {
        id: "claude",
        network: true,
        collect: claude::collect,
    },
    Provider {
        id: "devin",
        network: false,
        collect: devin::collect,
    },
    Provider {
        id: "agy",
        network: true,
        collect: agy::collect,
    },
    Provider {
        id: "opencode",
        network: true,
        collect: opencode::collect,
    },
];

/// Runs the collectors, one run at a time, and remembers when each provider
/// was last asked.
#[derive(Default)]
pub struct Collector {
    running: AtomicBool,
    asked: Mutex<HashMap<&'static str, Instant>>,
}

impl Collector {
    /// Collects every provider that is due and writes what it found under
    /// `GOKO_HOME/usage/collected`. `force` is a person asking for fresh
    /// numbers: providers are asked again sooner. Returns `false`, having
    /// done nothing, when a run is already going. Blocks for as long as the
    /// requests take, so it belongs on a thread of its own.
    pub fn collect(&self, env: &Env, outside: &dyn Outside, force: bool) -> bool {
        if self.running.swap(true, Ordering::AcqRel) {
            return false;
        }
        let dir = env.goko_home.join(COLLECTED_DIR);
        for provider in &PROVIDERS {
            if !self.due(provider, force) {
                continue;
            }
            match (provider.collect)(env, outside) {
                Outcome::Record(record) => {
                    let _ = usage_records::write(&dir, &record);
                }
                Outcome::SignedOut => {
                    let _ = fs::remove_file(dir.join(format!("{}.json", provider.id)));
                }
                Outcome::Unavailable => {}
            }
        }
        self.running.store(false, Ordering::Release);
        true
    }

    /// Whether `provider` may be asked now, noting that it was.
    fn due(&self, provider: &Provider, force: bool) -> bool {
        if !provider.network {
            return true;
        }
        let wait = if force { ASK_AGAIN_FORCED } else { ASK_AGAIN };
        let mut asked = self.asked.lock().unwrap_or_else(|lock| lock.into_inner());
        let now = Instant::now();
        if asked
            .get(provider.id)
            .is_some_and(|at| now.duration_since(*at) < wait)
        {
            return false;
        }
        asked.insert(provider.id, now);
        true
    }
}

/// The real outside: `curl` for requests, the OS's own tools for a
/// credential store.
pub struct System;

impl Outside for System {
    fn http(&self, request: &Request) -> Result<String, HttpError> {
        // The request, token included, travels to curl on its stdin: a
        // command line is readable by every process on the machine.
        let config = curl_config(request).ok_or(HttpError::Transport)?;
        let mut command = Command::new("curl");
        command
            .args([
                "--silent",
                "--proto",
                "=https",
                "--max-time",
                REQUEST_SECONDS,
            ])
            .args(["--write-out", "\n%{http_code}", "--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        hide_console(&mut command);
        let mut child = command.spawn().map_err(|_| HttpError::Transport)?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(config.as_bytes());
        }
        let output = child.wait_with_output().map_err(|_| HttpError::Transport)?;
        if !output.status.success() {
            return Err(HttpError::Transport);
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let (body, status) = text.rsplit_once('\n').ok_or(HttpError::Transport)?;
        match status.trim().parse::<u16>() {
            Ok(status) if (200..300).contains(&status) => Ok(body.to_string()),
            Ok(status) if status >= 100 => Err(HttpError::Status(status)),
            _ => Err(HttpError::Transport),
        }
    }

    fn run(&self, program: &str, args: &[&str]) -> Option<String> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        hide_console(&mut command);
        let mut child = command.spawn().ok()?;
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() < LOOKUP_TIMEOUT => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        };
        let output = child.wait_with_output().ok()?;
        if !status.success() {
            return None;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }
}

/// A GUI app's child gets no console window of its own on Windows.
fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW
        command.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// `request` as a curl config file. `None` when a value could not be written
/// as one line of it, which no real request has.
fn curl_config(request: &Request) -> Option<String> {
    if !request.url.starts_with("https://") {
        return None;
    }
    let quoted = |value: &str| -> Option<String> {
        if value.contains(['\n', '\r']) {
            return None;
        }
        Some(format!(
            "\"{}\"",
            value.replace('\\', "\\\\").replace('"', "\\\"")
        ))
    };
    let mut config = format!(
        "url = {}\nrequest = {}\n",
        quoted(&request.url)?,
        quoted(request.method)?
    );
    for (name, value) in &request.headers {
        config.push_str(&format!(
            "header = {}\n",
            quoted(&format!("{name}: {value}"))?
        ));
    }
    if let Some(body) = &request.body {
        config.push_str(&format!("data = {}\n", quoted(body)?));
    }
    Some(config)
}

/// A JSON file's value, when it is one.
fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// A JSON string field, trimmed; empty when it is absent or not a string.
fn text_of(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// `Bearer <token>` asking for JSON.
fn bearer(token: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Authorization", format!("Bearer {token}")),
        ("Accept", "application/json".to_string()),
    ]
}

/// Epoch seconds as an RFC 3339 UTC time. `None` outside 2000–2100: a stamp
/// past those is garbage, not a time.
fn rfc3339_utc(secs: i64) -> Option<String> {
    const EARLIEST: i64 = 946_684_800;
    const LATEST: i64 = 4_102_444_800;
    if !(EARLIEST..=LATEST).contains(&secs) {
        return None;
    }
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    ))
}

/// Standard or URL-safe base64, padded or not. `None` when it is neither.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut bits, mut count) = (0u32, 0u8);
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        bits = bits << 6 | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
pub(crate) mod testing {
    //! A scripted [`Outside`]: answers by URL suffix, lookups by program.
    use super::*;

    #[derive(Default)]
    pub struct Scripted {
        pub answers: Vec<(&'static str, Result<String, HttpError>)>,
        pub lookups: Vec<(&'static str, String)>,
        pub requests: Mutex<Vec<String>>,
    }

    impl Scripted {
        pub fn answering(answers: &[(&'static str, Result<&str, HttpError>)]) -> Self {
            Self {
                answers: answers
                    .iter()
                    .map(|(url, answer)| (*url, answer.map(str::to_string)))
                    .collect(),
                ..Default::default()
            }
        }

        /// Every request made, as `METHOD url | header: value | … | body`.
        pub fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Outside for Scripted {
        fn http(&self, request: &Request) -> Result<String, HttpError> {
            let mut line = format!("{} {}", request.method, request.url);
            for (name, value) in &request.headers {
                line.push_str(&format!(" | {name}: {value}"));
            }
            if let Some(body) = &request.body {
                line.push_str(&format!(" | {body}"));
            }
            self.requests.lock().unwrap().push(line);
            self.answers
                .iter()
                .find(|(url, _)| request.url.ends_with(url))
                .map(|(_, answer)| answer.clone())
                .unwrap_or(Err(HttpError::Transport))
        }

        fn run(&self, program: &str, _args: &[&str]) -> Option<String> {
            self.lookups
                .iter()
                .find(|(known, _)| *known == program)
                .map(|(_, output)| output.clone())
        }
    }

    /// An environment rooted in `home`, with no variables set.
    pub fn env(home: &Path) -> Env {
        Env {
            goko_home: home.join("goko"),
            home: Some(home.to_path_buf()),
            claude_dir: None,
            var: |_| None,
            now_secs: 1_700_000_000,
        }
    }

    pub fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{env, write, Scripted};
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn epoch_seconds_format_as_rfc3339() {
        assert_eq!(
            rfc3339_utc(1_700_000_000).as_deref(),
            Some("2023-11-14T22:13:20Z")
        );
        assert_eq!(
            rfc3339_utc(951_782_400).as_deref(),
            Some("2000-02-29T00:00:00Z")
        );
        assert_eq!(
            rfc3339_utc(1_798_761_599).as_deref(),
            Some("2026-12-31T23:59:59Z")
        );
        assert_eq!(rfc3339_utc(0), None, "before 2000 is not a reset time");
        assert_eq!(rfc3339_utc(i64::MAX), None);
    }

    #[test]
    fn base64_decodes_both_alphabets_with_or_without_padding() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVsbG8").unwrap(), b"hello");
        assert_eq!(base64_decode("-_8=").unwrap(), [0xfb, 0xff]);
        assert_eq!(base64_decode("+/8=").unwrap(), [0xfb, 0xff]);
        assert_eq!(base64_decode("not base64!"), None);
    }

    /// The token travels in the config on stdin, quoted so that nothing in a
    /// value can start another option; only https is ever asked.
    #[test]
    fn a_request_becomes_a_curl_config() {
        let request = Request {
            method: "POST",
            url: "https://example.test/v1:ask".to_string(),
            headers: bearer("to\"ken\\"),
            body: Some(r#"{"project":"p"}"#.to_string()),
        };
        assert_eq!(
            curl_config(&request).unwrap(),
            "url = \"https://example.test/v1:ask\"\nrequest = \"POST\"\n\
             header = \"Authorization: Bearer to\\\"ken\\\\\"\n\
             header = \"Accept: application/json\"\n\
             data = \"{\\\"project\\\":\\\"p\\\"}\"\n"
        );
        let plain = Request {
            url: "http://example.test".to_string(),
            ..request
        };
        assert!(curl_config(&plain).is_none(), "never over plain http");
        let split = Request {
            method: "GET",
            url: "https://example.test".to_string(),
            headers: bearer("a\nurl = \"https://evil.test\""),
            body: None,
        };
        assert!(curl_config(&split).is_none(), "a value is one line");
    }

    /// A machine with no agent signed in asks nobody and keeps no record;
    /// a second run while one is going does nothing.
    #[test]
    fn a_machine_with_no_sign_in_asks_nobody_and_keeps_no_record() {
        let temp = TempDir::new("collect");
        let env = env(temp.path());
        let outside = Scripted::default();
        let collected = env.goko_home.join(COLLECTED_DIR);
        write(&collected.join("claude.json"), "{}");

        let collector = Collector::default();
        assert!(collector.collect(&env, &outside, false));
        assert!(outside.requests().is_empty());
        assert!(
            !collected.join("claude.json").exists(),
            "a signed-out provider's record is removed"
        );

        collector.running.store(true, Ordering::Release);
        assert!(!collector.collect(&env, &outside, true));
    }

    /// Opening the dropdown again soon does not ask the provider again;
    /// Refresh may, once its own shorter wait has passed.
    #[test]
    fn a_provider_is_not_asked_again_too_soon() {
        let collector = Collector::default();
        let claude = &PROVIDERS[0];
        assert!(collector.due(claude, false));
        assert!(!collector.due(claude, false));
        assert!(!collector.due(claude, true), "not within the forced wait");
        let long_ago = Instant::now() - ASK_AGAIN_FORCED - Duration::from_secs(1);
        collector.asked.lock().unwrap().insert("claude", long_ago);
        assert!(!collector.due(claude, false));
        assert!(collector.due(claude, true));
        // Devin's answer is a file on disk: reading it is always due.
        let devin = &PROVIDERS[1];
        assert!(collector.due(devin, false) && collector.due(devin, false));
    }
}
