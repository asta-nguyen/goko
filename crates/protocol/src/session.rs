use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(Uuid::from_bytes(bytes))
    }

    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }

    pub const fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Created,
    Starting,
    Running,
    WaitingForInput,
    Idle,
    Stopping,
    Exited,
    Failed,
    Disconnected,
    Interrupted,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionInfo {
    pub id: SessionId,
    pub title: String,
    pub argv: Vec<String>,
    pub cwd: String,
    pub state: SessionState,
    /// The PTY child's pid; `None` before spawn and for a failed spawn.
    #[serde(default)]
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    /// Unix epoch milliseconds.
    pub started_at: i64,
    /// Unix epoch milliseconds.
    pub ended_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::{SessionId, SessionInfo, SessionState};

    #[test]
    fn every_state_serializes_as_snake_case_and_round_trips() {
        let states = [
            (SessionState::Created, "created"),
            (SessionState::Starting, "starting"),
            (SessionState::Running, "running"),
            (SessionState::WaitingForInput, "waiting_for_input"),
            (SessionState::Idle, "idle"),
            (SessionState::Stopping, "stopping"),
            (SessionState::Exited, "exited"),
            (SessionState::Failed, "failed"),
            (SessionState::Disconnected, "disconnected"),
            (SessionState::Interrupted, "interrupted"),
            (SessionState::Unknown, "unknown"),
        ];

        for (state, name) in states {
            let json = serde_json::to_string(&state).unwrap();
            assert_eq!(json, format!("\"{name}\""));
            assert_eq!(serde_json::from_str::<SessionState>(&json).unwrap(), state);
        }
    }

    #[test]
    fn unknown_state_fails_to_deserialize() {
        assert!(serde_json::from_str::<SessionState>("\"bogus\"").is_err());
    }

    #[test]
    fn session_info_without_exit_code_round_trips() {
        let info = SessionInfo {
            id: SessionId::new(),
            title: "shell".into(),
            argv: vec!["sh".into()],
            cwd: "/tmp".into(),
            state: SessionState::Running,
            pid: None,
            exit_code: None,
            error: None,
            started_at: 1_700_000_000_000,
            ended_at: None,
        };

        let json = serde_json::to_string(&info).unwrap();
        assert_eq!(serde_json::from_str::<SessionInfo>(&json).unwrap(), info);
    }

    #[test]
    fn session_info_without_a_pid_key_deserializes_as_none() {
        let info = SessionInfo {
            id: SessionId::new(),
            title: "shell".into(),
            argv: vec!["sh".into()],
            cwd: "/tmp".into(),
            state: SessionState::Running,
            pid: None,
            exit_code: None,
            error: None,
            started_at: 1_700_000_000_000,
            ended_at: None,
        };
        let mut value = serde_json::to_value(&info).unwrap();
        value.as_object_mut().unwrap().remove("pid");

        let decoded = serde_json::from_value::<SessionInfo>(value).unwrap();

        assert_eq!(decoded.pid, None);
        assert_eq!(decoded, info);
    }

    #[test]
    fn session_info_with_a_pid_round_trips() {
        let info = SessionInfo {
            id: SessionId::new(),
            title: "shell".into(),
            argv: vec!["sh".into()],
            cwd: "/tmp".into(),
            state: SessionState::Running,
            pid: Some(4242),
            exit_code: None,
            error: None,
            started_at: 1_700_000_000_000,
            ended_at: None,
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"pid\":4242"));
        assert_eq!(serde_json::from_str::<SessionInfo>(&json).unwrap(), info);
    }

    #[test]
    fn session_id_is_a_uuid_string_and_preserves_frame_bytes() {
        let id = SessionId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{}\"", id.as_uuid()));
        assert_eq!(SessionId::from_bytes(*id.as_bytes()), id);
    }
}
