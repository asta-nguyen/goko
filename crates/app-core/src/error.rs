//! The typed error every bridge command returns to the webview as `{code, message, hint}`.

use protocol::message::{ErrorCode, ProtoError};
use serde::{Serialize, Serializer};

/// Failures the bridge detects itself, plus the daemon's own codes passed through.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Code {
    DaemonUnavailable,
    DaemonStartFailed,
    Timeout,
    Disconnected,
    Storage,
    Daemon(ErrorCode),
}

impl Serialize for Code {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::DaemonUnavailable => serializer.serialize_str("daemon_unavailable"),
            Self::DaemonStartFailed => serializer.serialize_str("daemon_start_failed"),
            Self::Timeout => serializer.serialize_str("timeout"),
            Self::Disconnected => serializer.serialize_str("disconnected"),
            Self::Storage => serializer.serialize_str("storage"),
            Self::Daemon(code) => code.serialize(serializer),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BridgeError {
    pub code: Code,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl BridgeError {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BridgeError {}

impl From<ProtoError> for BridgeError {
    fn from(error: ProtoError) -> Self {
        Self {
            code: Code::Daemon(error.code),
            message: error.message,
            hint: error.hint,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BridgeError, Code};
    use protocol::message::ErrorCode;
    use serde_json::json;

    #[test]
    fn serializes_as_code_message_and_optional_hint() {
        let timeout = serde_json::to_value(BridgeError::new(Code::Timeout, "slow")).unwrap();
        assert_eq!(timeout, json!({"code": "timeout", "message": "slow"}));

        let passthrough =
            BridgeError::new(Code::Daemon(ErrorCode::AuthFailed), "no").with_hint("h");
        assert_eq!(
            serde_json::to_value(passthrough).unwrap(),
            json!({"code": "auth_failed", "message": "no", "hint": "h"})
        );
    }
}
