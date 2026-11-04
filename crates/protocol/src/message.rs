use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::session::{SessionId, SessionInfo};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Hello {
    pub protocol_version: u32,
    pub token: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HelloOk {
    pub daemon_version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum Request {
    #[serde(rename = "system.shutdown")]
    SystemShutdown { id: u64 },
    #[serde(rename = "system.health")]
    SystemHealth { id: u64 },
    #[serde(rename = "system.version")]
    SystemVersion { id: u64 },
    #[serde(rename = "events.subscribe")]
    EventsSubscribe { id: u64 },
    #[serde(rename = "session.create")]
    SessionCreate {
        id: u64,
        argv: Vec<String>,
        cwd: String,
        cols: u16,
        rows: u16,
        title: String,
    },
    #[serde(rename = "session.list")]
    SessionList { id: u64 },
    #[serde(rename = "session.get")]
    SessionGet { id: u64, session_id: SessionId },
    #[serde(rename = "session.stop")]
    SessionStop {
        id: u64,
        session_id: SessionId,
        force: bool,
    },
    #[serde(rename = "session.delete")]
    SessionDelete { id: u64, session_id: SessionId },
    #[serde(rename = "terminal.write")]
    TerminalWrite {
        id: u64,
        session_id: SessionId,
        data_b64: String,
    },
    #[serde(rename = "terminal.resize")]
    TerminalResize {
        id: u64,
        session_id: SessionId,
        cols: u16,
        rows: u16,
    },
    #[serde(rename = "terminal.subscribe")]
    TerminalSubscribe { id: u64, session_id: SessionId },
    #[serde(rename = "terminal.unsubscribe")]
    TerminalUnsubscribe { id: u64, session_id: SessionId },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Response {
    pub id: u64,
    #[serde(flatten)]
    pub outcome: ResponseOutcome,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ResponseOutcome {
    Result { result: Value },
    Error { error: ProtoError },
}

impl<'de> Deserialize<'de> for ResponseOutcome {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OutcomeVisitor;

        impl<'de> Visitor<'de> for OutcomeVisitor {
            type Value = ResponseOutcome;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an object containing exactly one of `result` or `error`")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut result = None;
                let mut error = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "result" if result.is_none() => result = Some(map.next_value()?),
                        "error" if error.is_none() => error = Some(map.next_value()?),
                        "result" | "error" => {
                            return Err(de::Error::duplicate_field(if key == "result" {
                                "result"
                            } else {
                                "error"
                            }));
                        }
                        _ => return Err(de::Error::unknown_field(&key, &["result", "error"])),
                    }
                }

                match (result, error) {
                    (Some(result), None) => Ok(ResponseOutcome::Result { result }),
                    (None, Some(error)) => Ok(ResponseOutcome::Error { error }),
                    _ => Err(de::Error::custom(
                        "response must contain exactly one of `result` or `error`",
                    )),
                }
            }
        }

        deserializer.deserialize_map(OutcomeVisitor)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum Event {
    #[serde(rename = "session.updated")]
    SessionUpdated { session: SessionInfo },
    #[serde(rename = "session.exited")]
    SessionExited {
        session_id: SessionId,
        exit_code: Option<i32>,
    },
    #[serde(rename = "terminal.lagged")]
    TerminalLagged { session_id: SessionId },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    AuthFailed,
    VersionUnsupported,
    FrameTooLarge,
    Malformed,
    InvalidArgument,
    NotFound,
    SpawnFailed,
    Internal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProtoError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{ErrorCode, Event, Hello, HelloOk, ProtoError, Request, Response, ResponseOutcome};
    use crate::session::{SessionId, SessionInfo, SessionState};
    use serde::de::DeserializeOwned;
    use serde::Serialize;

    fn assert_round_trip<T>(value: &T)
    where
        T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).unwrap();
        assert_eq!(serde_json::from_str::<T>(&json).unwrap(), *value);
    }

    fn session() -> SessionInfo {
        SessionInfo {
            id: SessionId::new(),
            title: "shell".into(),
            argv: vec!["sh".into()],
            cwd: "/tmp".into(),
            state: SessionState::Running,
            exit_code: None,
            error: None,
            started_at: 1_700_000_000_000,
            ended_at: None,
        }
    }

    #[test]
    fn hello_and_hello_ok_round_trip() {
        assert_round_trip(&Hello {
            protocol_version: 1,
            token: "token".into(),
        });
        assert_round_trip(&HelloOk {
            daemon_version: "0.1.0".into(),
        });
    }

    #[test]
    fn every_request_round_trips_with_numeric_id() {
        let session_id = SessionId::new();
        let requests = [
            Request::SystemShutdown { id: 1 },
            Request::SystemHealth { id: 2 },
            Request::SystemVersion { id: 3 },
            Request::EventsSubscribe { id: 4 },
            Request::SessionCreate {
                id: 5,
                argv: vec!["sh".into()],
                cwd: "/tmp".into(),
                cols: 80,
                rows: 24,
                title: "shell".into(),
            },
            Request::SessionList { id: 6 },
            Request::SessionGet { id: 7, session_id },
            Request::SessionStop {
                id: 8,
                session_id,
                force: false,
            },
            Request::SessionDelete { id: 13, session_id },
            Request::TerminalWrite {
                id: 9,
                session_id,
                data_b64: "aGVsbG8=".into(),
            },
            Request::TerminalResize {
                id: 10,
                session_id,
                cols: 100,
                rows: 30,
            },
            Request::TerminalSubscribe { id: 11, session_id },
            Request::TerminalUnsubscribe { id: 12, session_id },
        ];

        for request in requests {
            assert_round_trip(&request);
        }
    }

    #[test]
    fn request_requires_id_and_rejects_unknown_type() {
        assert!(serde_json::from_str::<Request>(r#"{"type":"system.health"}"#).is_err());
        assert!(serde_json::from_str::<Request>(r#"{"type":"other.call","id":1}"#).is_err());
    }

    #[test]
    fn response_result_and_error_round_trip_and_echo_numeric_id() {
        for response in [
            Response {
                id: 42,
                outcome: ResponseOutcome::Result {
                    result: serde_json::json!({"uptime_seconds": 2}),
                },
            },
            Response {
                id: 43,
                outcome: ResponseOutcome::Error {
                    error: ProtoError {
                        code: ErrorCode::NotFound,
                        message: "missing".into(),
                        hint: Some("check the session id".into()),
                    },
                },
            },
        ] {
            assert_round_trip(&response);
            let value: serde_json::Value = serde_json::to_value(&response).unwrap();
            assert_eq!(value["id"].as_u64(), Some(response.id));
        }
    }

    #[test]
    fn response_rejects_both_or_neither_outcome_fields() {
        assert!(serde_json::from_str::<Response>(
            r#"{"id":1,"result":{},"error":{"code":"internal","message":"failed"}}"#
        )
        .is_err());
        assert!(serde_json::from_str::<Response>(r#"{"id":1}"#).is_err());
    }

    #[test]
    fn every_event_round_trips() {
        let session = session();
        let events = [
            Event::SessionUpdated {
                session: session.clone(),
            },
            Event::SessionExited {
                session_id: session.id,
                exit_code: Some(7),
            },
            Event::TerminalLagged {
                session_id: session.id,
            },
        ];

        for event in events {
            assert_round_trip(&event);
        }
    }

    #[test]
    fn error_codes_are_snake_case() {
        assert_eq!(
            serde_json::to_string(&ErrorCode::AuthFailed).unwrap(),
            "\"auth_failed\""
        );
        assert_eq!(
            serde_json::to_string(&ErrorCode::FrameTooLarge).unwrap(),
            "\"frame_too_large\""
        );
    }
}
