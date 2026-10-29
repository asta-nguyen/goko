//! Authenticated Unix-socket server.
//!
//! Every connection starts with a `Hello` frame. Errors that cannot be tied to a
//! request (bad hello, bad frame) are sent as a `Response` with `id` 0 and an error,
//! so clients must number their requests from 1.

use crate::auth::Token;
use crate::paths::{remove_if_exists, Paths};
use protocol::frame::{self, Frame, FrameError, FrameKind, MAX_FRAME_PAYLOAD};
use protocol::message::{
    ErrorCode, Hello, HelloOk, ProtoError, Request, Response, ResponseOutcome,
};
use protocol::PROTOCOL_VERSION;
use serde::Serialize;
use serde_json::{json, Value};
use std::fs::{self, Permissions};
use std::future::Future;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinSet;
use tracing::{debug, warn};

struct State {
    token: Token,
    started: Instant,
}

/// Replace any stale `daemon.sock` and bind a fresh one with mode 0600. The caller must
/// hold the instance lock, which proves no live daemon owns the old socket.
pub fn bind(paths: &Paths) -> io::Result<UnixListener> {
    remove_if_exists(&paths.socket())?;
    let listener = UnixListener::bind(paths.socket())?;
    fs::set_permissions(paths.socket(), Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Accept clients until `shutdown` completes. Open connections are dropped on return.
pub async fn serve(
    listener: UnixListener,
    token: Token,
    shutdown: impl Future<Output = ()>,
) -> io::Result<()> {
    let state = Arc::new(State {
        token,
        started: Instant::now(),
    });
    let mut connections = JoinSet::new();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            () = &mut shutdown => return Ok(()),
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    connections.spawn(handle(stream, Arc::clone(&state)));
                }
                Err(error) => {
                    warn!("could not accept a connection: {error}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            Some(_) = connections.join_next() => {}
        }
    }
}

async fn handle(mut stream: UnixStream, state: Arc<State>) {
    if let Err(error) = connection(&mut stream, &state).await {
        match error.kind() {
            io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset => {
                debug!("client went away: {error}");
            }
            _ => warn!("connection failed: {error}"),
        }
    }
}

async fn connection(stream: &mut UnixStream, state: &State) -> io::Result<()> {
    let Some(payload) = next_control(stream).await? else {
        return Ok(());
    };
    let hello = match serde_json::from_slice::<Hello>(&payload) {
        Ok(hello) => hello,
        Err(error) => {
            let message = format!("the first frame must be a hello: {error}");
            return send(stream, &error_response(0, ErrorCode::Malformed, message)).await;
        }
    };
    if !state.token.verify(&hello.token) {
        let response = error_response(0, ErrorCode::AuthFailed, "wrong token");
        return send(stream, &response).await;
    }
    if hello.protocol_version != PROTOCOL_VERSION {
        let message = format!(
            "protocol version {} is not supported; this daemon speaks {PROTOCOL_VERSION}",
            hello.protocol_version
        );
        let response = error_response(0, ErrorCode::VersionUnsupported, message);
        return send(stream, &response).await;
    }
    send(
        stream,
        &HelloOk {
            daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
    )
    .await?;

    while let Some(payload) = next_control(stream).await? {
        send(stream, &respond(state, &payload)).await?;
    }
    Ok(())
}

fn respond(state: &State, payload: &[u8]) -> Response {
    let value = match serde_json::from_slice::<Value>(payload) {
        Ok(value) => value,
        Err(error) => {
            return error_response(0, ErrorCode::Malformed, format!("invalid JSON: {error}"))
        }
    };
    let id = value.get("id").and_then(Value::as_u64).unwrap_or(0);
    match serde_json::from_value::<Request>(value) {
        Ok(request) => dispatch(state, id, request),
        Err(error) => error_response(
            id,
            ErrorCode::Malformed,
            format!("invalid request: {error}"),
        ),
    }
}

fn dispatch(state: &State, id: u64, request: Request) -> Response {
    let result = match request {
        Request::SystemHealth { .. } => json!({
            "status": "ok",
            "uptime_seconds": state.started.elapsed().as_secs(),
        }),
        Request::SystemVersion { .. } => json!({
            "daemon_version": env!("CARGO_PKG_VERSION"),
            "protocol_version": PROTOCOL_VERSION,
        }),
        _ => return error_response(id, ErrorCode::NotFound, "this request is not implemented"),
    };
    Response {
        id,
        outcome: ResponseOutcome::Result { result },
    }
}

fn error_response(id: u64, code: ErrorCode, message: impl Into<String>) -> Response {
    Response {
        id,
        outcome: ResponseOutcome::Error {
            error: ProtoError {
                code,
                message: message.into(),
                hint: None,
            },
        },
    }
}

/// The next control payload, or `None` once the connection is finished: the peer left,
/// or the frame was invalid and an error has been sent.
async fn next_control(stream: &mut UnixStream) -> io::Result<Option<Vec<u8>>> {
    let error = match read_frame(stream).await {
        Ok(Some(Frame {
            kind: FrameKind::Control,
            payload,
        })) => return Ok(Some(payload)),
        Ok(Some(_)) => FrameError::Malformed("clients may only send control frames"),
        Ok(None) => return Ok(None),
        Err(ReadError::Io(error)) => return Err(error),
        Err(ReadError::Frame(error)) => error,
    };
    let code = match error {
        FrameError::FrameTooLarge(_) => ErrorCode::FrameTooLarge,
        _ => ErrorCode::Malformed,
    };
    send(stream, &error_response(0, code, error.to_string())).await?;
    Ok(None)
}

enum ReadError {
    Io(io::Error),
    Frame(FrameError),
}

/// Read one frame; `None` means the peer closed the connection, even mid-frame.
async fn read_frame(stream: &mut UnixStream) -> Result<Option<Frame>, ReadError> {
    let mut header = [0_u8; 5];
    if !read_exact(stream, &mut header).await? {
        return Ok(None);
    }
    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    if length > MAX_FRAME_PAYLOAD {
        return Err(ReadError::Frame(FrameError::FrameTooLarge(length)));
    }

    let mut bytes = vec![0_u8; header.len() + length];
    bytes[..header.len()].copy_from_slice(&header);
    if !read_exact(stream, &mut bytes[header.len()..]).await? {
        return Ok(None);
    }
    frame::decode(&bytes)
        .map(|(frame, _)| Some(frame))
        .map_err(ReadError::Frame)
}

/// `Ok(false)` when the peer closed before `buffer` was full.
async fn read_exact(stream: &mut UnixStream, buffer: &mut [u8]) -> Result<bool, ReadError> {
    match stream.read_exact(buffer).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(ReadError::Io(error)),
    }
}

async fn send(stream: &mut UnixStream, message: &impl Serialize) -> io::Result<()> {
    let payload = serde_json::to_vec(message).map_err(io::Error::other)?;
    let bytes = frame::encode(&Frame {
        kind: FrameKind::Control,
        payload,
    })
    .map_err(io::Error::other)?;
    stream.write_all(&bytes).await
}
