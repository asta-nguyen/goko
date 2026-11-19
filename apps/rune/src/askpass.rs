//! SSH askpass bridge for the Editor's Push: the push thread listens on
//! loopback, `goko-rune` re-execs as ssh's askpass helper, and the UI answers
//! through a masked modal. Only passphrase prompts are accepted; the answer
//! goes to that one push and is never stored.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, SyncSender, sync_channel};
use std::time::Duration;

use rune::edit::TextEdit;
use rune_kit::{Dialog, TextInput};

use crate::State;
use crate::theme::Ink;

/// Set on the Git child: this binary is being run as ssh's askpass helper.
pub const HELPER_ENV: &str = "GOKO_ASKPASS_HELPER";
/// The loopback port the push thread listens on.
pub const PORT_ENV: &str = "GOKO_ASKPASS_PORT";
/// The one-time token the helper presents before it may ask.
pub const TOKEN_ENV: &str = "GOKO_ASKPASS_TOKEN";

/// How often the accept loop looks for shutdown while no helper connects.
const ACCEPT_POLL: Duration = Duration::from_millis(25);
/// How often a connected helper's answer wait looks for shutdown.
const ANSWER_POLL: Duration = Duration::from_millis(100);
/// Handshake deadline for one helper connection.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// A prompt longer than this is not ssh's.
const MAX_PROMPT: usize = 4096;
/// A passphrase longer than this is refused instead of buffered.
const MAX_PASSPHRASE: usize = 1024;

/// One ssh prompt the UI must answer: the prompt text and the one-shot
/// reply channel the push thread waits on.
pub struct AskpassRequest {
    pub prompt: String,
    pub answer: SyncSender<Option<String>>,
}

/// The push thread's half of one `git push`: a loopback listener that lives
/// until the push returns and turns each helper connection into one request.
pub struct Session {
    envs: Vec<(&'static str, String)>,
    shutdown: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Session {
    /// Bind the loopback listener and start serving helper connections.
    /// `ask` hands each passphrase prompt to the UI; it must not block.
    pub fn start(ask: impl Fn(AskpassRequest) + Send + 'static) -> std::io::Result<Session> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let token = token()?;
        let helper = std::env::current_exe()?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let thread = {
            let shutdown = Arc::clone(&shutdown);
            let cancelled = Arc::clone(&cancelled);
            let token = token.clone();
            std::thread::Builder::new()
                .name("goko-askpass".into())
                .spawn(move || serve(listener, token, ask, shutdown, cancelled))?
        };
        Ok(Session {
            envs: vec![
                ("SSH_ASKPASS", helper.display().to_string()),
                ("SSH_ASKPASS_REQUIRE", "force".into()),
                (HELPER_ENV, "1".into()),
                (PORT_ENV, port.to_string()),
                (TOKEN_ENV, token),
            ],
            shutdown,
            cancelled,
            thread: Some(thread),
        })
    }

    /// The environment for the Git child: ssh runs `SSH_ASKPASS` with these
    /// values and the helper inherits them.
    pub fn envs(&self) -> Vec<(&str, &str)> {
        self.envs.iter().map(|(k, v)| (*k, v.as_str())).collect()
    }

    /// True when the user dismissed the modal instead of submitting.
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn token() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn serve(
    listener: TcpListener,
    token: String,
    ask: impl Fn(AskpassRequest),
    shutdown: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => handle(stream, &token, &ask, &shutdown, &cancelled),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(ACCEPT_POLL);
            }
            Err(_) => break,
        }
    }
}

fn handle(
    mut stream: TcpStream,
    token: &str,
    ask: &impl Fn(AskpassRequest),
    shutdown: &AtomicBool,
    cancelled: &AtomicBool,
) {
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
    let Some((sent, prompt)) = read_request(&mut stream) else {
        return;
    };
    if sent != token || !prompt.to_ascii_lowercase().contains("passphrase") {
        let _ = write_cancel(&mut stream);
        return;
    }
    let (answer_tx, answer_rx) = sync_channel::<Option<String>>(1);
    ask(AskpassRequest {
        prompt,
        answer: answer_tx,
    });
    loop {
        if shutdown.load(Ordering::SeqCst) {
            let _ = write_cancel(&mut stream);
            return;
        }
        match answer_rx.recv_timeout(ANSWER_POLL) {
            Ok(Some(passphrase)) => {
                let _ = write_passphrase(&mut stream, &passphrase);
                return;
            }
            Ok(None) => {
                cancelled.store(true, Ordering::SeqCst);
                let _ = write_cancel(&mut stream);
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                let _ = write_cancel(&mut stream);
                return;
            }
        }
    }
}

/// `token\n<len>\n<prompt bytes>` — the helper's one request.
fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let sent = read_line(&mut reader)?;
    if sent.len() > 128 {
        return None;
    }
    let len: usize = read_line(&mut reader)?.parse().ok()?;
    if len > MAX_PROMPT {
        return None;
    }
    let mut prompt = vec![0u8; len];
    reader.read_exact(&mut prompt).ok()?;
    Some((sent, String::from_utf8(prompt).ok()?))
}

fn write_passphrase(stream: &mut TcpStream, passphrase: &str) -> std::io::Result<()> {
    write!(stream, "1\n{}\n", passphrase.len())?;
    stream.write_all(passphrase.as_bytes())
}

fn write_cancel(stream: &mut TcpStream) -> std::io::Result<()> {
    stream.write_all(b"0\n")
}

fn read_line(reader: &mut impl BufRead) -> Option<String> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim_end_matches(['\r', '\n']).to_string()),
    }
}

/// True when this process is ssh's askpass helper, not the app.
pub fn is_helper() -> bool {
    std::env::var_os(HELPER_ENV).is_some()
}

/// Run as ssh's askpass helper and exit: print the passphrase the UI
/// submitted, or exit 1 for a cancel or any failure.
pub fn run_helper() -> ! {
    std::process::exit(helper());
}

fn helper() -> i32 {
    let Some(prompt) = std::env::args().nth(1) else {
        return 1;
    };
    let Some(port) = std::env::var(PORT_ENV)
        .ok()
        .and_then(|port| port.parse::<u16>().ok())
    else {
        return 1;
    };
    let Ok(token) = std::env::var(TOKEN_ENV) else {
        return 1;
    };
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return 1;
    };
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
    if write!(stream, "{token}\n{}\n", prompt.len()).is_err() {
        return 1;
    }
    if stream.write_all(prompt.as_bytes()).is_err() {
        return 1;
    }
    // The answer waits for the user, not the handshake: the push's own
    // 120-second limit and the server closing the socket bound it.
    let _ = stream.set_read_timeout(None);
    let mut reader = BufReader::new(stream);
    if read_line(&mut reader).as_deref() != Some("1") {
        return 1;
    }
    let Some(len) = read_line(&mut reader).and_then(|line| line.parse::<usize>().ok()) else {
        return 1;
    };
    if len > MAX_PASSPHRASE {
        return 1;
    }
    let mut passphrase = vec![0u8; len];
    if reader.read_exact(&mut passphrase).is_err() {
        return 1;
    }
    let Ok(passphrase) = String::from_utf8(passphrase) else {
        return 1;
    };
    println!("{passphrase}");
    0
}

/// The UI's half of one prompt: the text being typed and the reply channel.
pub struct AskpassState {
    pub prompt: String,
    pub input: TextEdit,
    pub answer: Option<SyncSender<Option<String>>>,
}

impl AskpassState {
    pub fn new(request: AskpassRequest) -> AskpassState {
        let mut input = TextEdit::new("");
        input.set_secure(true);
        input.focused = true;
        AskpassState {
            prompt: clean_prompt(&request.prompt),
            input,
            answer: Some(request.answer),
        }
    }
}

fn clean_prompt(prompt: &str) -> String {
    prompt.trim().trim_end_matches(':').trim_end().to_string()
}

/// Submit the typed passphrase: hand it to the waiting push and clear it.
pub fn submit(s: &mut State) {
    let Some(mut askpass) = s.askpass.take() else {
        return;
    };
    let passphrase = std::mem::take(&mut askpass.input.text);
    if let Some(answer) = askpass.answer.take() {
        let _ = answer.send(Some(passphrase));
    }
}

/// Dismiss the prompt: the push stops with "Push cancelled".
pub fn cancel(s: &mut State) {
    let Some(askpass) = s.askpass.take() else {
        return;
    };
    if let Some(answer) = askpass.answer {
        let _ = answer.send(None);
    }
}

/// The masked modal, mounted above every view so a push that started in the
/// Editor still asks after the user switches to Terminal or Settings.
pub fn modal(s: &State, ink: &Ink, k: &rune_kit::Kit, layer: &mut Vec<rune::El>) {
    let Some(askpass) = &s.askpass else {
        return;
    };
    let input = TextInput::new(&askpass.input)
        .secure()
        .placeholder("Passphrase")
        .on_edit(|st: &mut State| -> &mut TextEdit { &mut st.askpass.as_mut().unwrap().input })
        .on_submit(|st: &mut State, _tx| submit(st))
        .build(k);
    let buttons = rune::row()
        .gap(8.0)
        .justify_end()
        .w_full()
        .child(
            crate::theme::btn_sm("Cancel", None, true, ink)
                .on_press_tx(|st: &mut State, _tx| cancel(st)),
        )
        .child(
            crate::theme::btn_sm("Unlock", None, false, ink)
                .on_press_tx(|st: &mut State, _tx| submit(st)),
        );
    Dialog::new("editor-askpass", "SSH key passphrase")
        .open(true)
        .fit()
        .width(420.0)
        .description(askpass.prompt.clone())
        .click_outside(false)
        .on_close(|st: &mut State| cancel(st))
        .child(
            rune::col()
                .gap(12.0)
                .w_full()
                .child(input.w_full())
                .child(buttons),
        )
        .build(k, layer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn env_value(envs: &[(&str, &str)], key: &str) -> String {
        envs.iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| (*value).to_string())
            .unwrap()
    }

    fn connect(session: &Session) -> (TcpStream, String) {
        let envs = session.envs();
        let port = env_value(&envs, PORT_ENV);
        let token = env_value(&envs, TOKEN_ENV);
        let stream = TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap())).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        (stream, token)
    }

    fn send_request(stream: &mut TcpStream, token: &str, prompt: &str) {
        write!(stream, "{token}\n{}\n", prompt.len()).unwrap();
        stream.write_all(prompt.as_bytes()).unwrap();
    }

    fn read_answer(stream: TcpStream) -> Option<String> {
        let mut reader = BufReader::new(stream);
        if read_line(&mut reader).as_deref() != Some("1") {
            return None;
        }
        let len: usize = read_line(&mut reader).unwrap().parse().unwrap();
        let mut passphrase = vec![0u8; len];
        reader.read_exact(&mut passphrase).unwrap();
        Some(String::from_utf8(passphrase).unwrap())
    }

    #[test]
    fn a_passphrase_prompt_reaches_the_ui_and_returns_the_answer() {
        let (prompt_tx, prompt_rx) = std::sync::mpsc::channel();
        let session = Session::start(move |request| {
            prompt_tx.send(request.prompt.clone()).unwrap();
            request.answer.send(Some("secret".into())).unwrap();
        })
        .unwrap();
        let (mut stream, token) = connect(&session);
        send_request(&mut stream, &token, "Enter passphrase for key '/k': ");
        assert_eq!(read_answer(stream).as_deref(), Some("secret"));
        assert_eq!(prompt_rx.recv().unwrap(), "Enter passphrase for key '/k': ");
        assert!(!session.cancelled());
    }

    #[test]
    fn cancelling_the_modal_marks_the_push_cancelled() {
        let session = Session::start(|request| {
            request.answer.send(None).unwrap();
        })
        .unwrap();
        let (mut stream, token) = connect(&session);
        send_request(&mut stream, &token, "Enter passphrase for key '/k': ");
        let mut reader = BufReader::new(stream);
        assert_eq!(read_line(&mut reader).as_deref(), Some("0"));
        assert!(session.cancelled());
    }

    #[test]
    fn a_wrong_token_never_reaches_the_ui() {
        let asked = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&asked);
        let session = Session::start(move |_request| {
            seen.store(true, Ordering::SeqCst);
        })
        .unwrap();
        let (mut stream, _token) = connect(&session);
        send_request(
            &mut stream,
            "not-the-token",
            "Enter passphrase for key '/k': ",
        );
        let mut reader = BufReader::new(stream);
        assert_eq!(read_line(&mut reader).as_deref(), Some("0"));
        assert!(!asked.load(Ordering::SeqCst));
    }

    #[test]
    fn only_passphrase_prompts_reach_the_ui() {
        let asked = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&asked);
        let session = Session::start(move |_request| {
            seen.store(true, Ordering::SeqCst);
        })
        .unwrap();
        let (mut stream, token) = connect(&session);
        send_request(&mut stream, &token, "nus@example.test's password: ");
        let mut reader = BufReader::new(stream);
        assert_eq!(read_line(&mut reader).as_deref(), Some("0"));
        assert!(!asked.load(Ordering::SeqCst));
    }
}
