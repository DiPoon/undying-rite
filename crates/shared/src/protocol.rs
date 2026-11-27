//! 서버와 클라이언트가 주고받는 메시지 (architecture.md 6절).
//! 서버와 `client-core`가 같은 타입을 쓰고, 인코딩은 이 파일의 `encode`/`decode_*`에서만 합니다.

use crate::sim::{Character, CharacterInput, InputError, OtherCharacter, PlayerId};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const PROTOCOL_VERSION: u32 = 1;

pub const ROOM_CODE_MAX_CHARS: usize = 12;
pub const NAME_MAX_CHARS: usize = 16;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Join {
        protocol: u32,
        room: String,
        name: String,
    },
    Input {
        seq: u32,
        #[serde(flatten)]
        input: CharacterInput,
    },
    Ping {
        client_time: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Welcome {
        protocol: u32,
        player_id: PlayerId,
        room: String,
        /// `config/balance.toml`의 원문. 클라이언트도 같은 파서로 읽어서 같은 틱 단위 값을 얻습니다.
        balance: String,
        /// 맵 텍스트 원문
        map: String,
    },
    Rejected {
        reason: RejectReason,
    },
    Roster {
        players: Vec<RosterEntry>,
    },
    State {
        tick: u64,
        /// 서버가 마지막으로 처리한 이 플레이어의 입력 일련번호. 아직 없으면 0
        ack: u32,
        me: Option<Character>,
        others: Vec<OtherCharacter>,
    },
    Pong {
        client_time: f64,
        /// 응답을 보낸 순간의 서버 틱 (소수 부분 포함)
        server_tick: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum RejectReason {
    ProtocolMismatch { server: u32 },
    RoomFull,
    BadRoomCode,
    BadName,
    NotJoined,
    ServerError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterEntry {
    pub id: PlayerId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProtocolError {
    TooLarge { bytes: usize, max: usize },
    Malformed(String),
    BadInput(InputError),
    NotFinite,
    Encode(String),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::TooLarge { bytes, max } => {
                write!(f, "메시지가 너무 큽니다 ({bytes}바이트, 최대 {max})")
            }
            ProtocolError::Malformed(m) => write!(f, "메시지 형식이 틀렸습니다: {m}"),
            ProtocolError::BadInput(e) => write!(f, "입력 값이 틀렸습니다: {e:?}"),
            ProtocolError::NotFinite => write!(f, "유한하지 않은 숫자가 들어 있습니다"),
            ProtocolError::Encode(m) => write!(f, "메시지를 만들지 못했습니다: {m}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

pub fn encode<T: Serialize>(msg: &T) -> Result<String, ProtocolError> {
    serde_json::to_string(msg).map_err(|e| ProtocolError::Encode(e.to_string()))
}

/// 클라이언트가 보낸 메시지를 해석하고 값의 범위까지 검증합니다.
pub fn decode_client(text: &str, max_bytes: usize) -> Result<ClientMsg, ProtocolError> {
    if text.len() > max_bytes {
        return Err(ProtocolError::TooLarge {
            bytes: text.len(),
            max: max_bytes,
        });
    }
    let msg: ClientMsg =
        serde_json::from_str(text).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
    match &msg {
        ClientMsg::Input { input, .. } => input.validate().map_err(ProtocolError::BadInput)?,
        ClientMsg::Ping { client_time } => {
            if !client_time.is_finite() {
                return Err(ProtocolError::NotFinite);
            }
        }
        ClientMsg::Join { .. } => {}
    }
    Ok(msg)
}

/// 서버가 보낸 메시지를 해석합니다. 서버도 믿을 수 있는 쪽이지만, 클라이언트가 패닉하지 않게 숫자는 검사합니다.
pub fn decode_server(text: &str) -> Result<ServerMsg, ProtocolError> {
    let msg: ServerMsg =
        serde_json::from_str(text).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
    let finite = match &msg {
        ServerMsg::State { me, others, .. } => {
            me.as_ref().is_none_or(Character::is_finite)
                && others
                    .iter()
                    .all(|o| o.pos.is_finite() && o.facing.is_finite())
        }
        ServerMsg::Pong {
            client_time,
            server_tick,
        } => client_time.is_finite() && server_tick.is_finite(),
        _ => true,
    };
    if finite {
        Ok(msg)
    } else {
        Err(ProtocolError::NotFinite)
    }
}

/// 방 코드는 영문자와 숫자 1~12자이고 대문자로 맞춥니다.
pub fn normalize_room_code(raw: &str) -> Option<String> {
    let code = raw.trim();
    let ok = !code.is_empty()
        && code.chars().count() <= ROOM_CODE_MAX_CHARS
        && code.chars().all(|c| c.is_ascii_alphanumeric());
    ok.then(|| code.to_ascii_uppercase())
}

/// 이름은 앞뒤 공백을 뺀 1~16자이고 제어 문자를 넣을 수 없습니다.
pub fn normalize_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    let ok = !name.is_empty()
        && name.chars().count() <= NAME_MAX_CHARS
        && !name.chars().any(char::is_control);
    ok.then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vec2;

    #[test]
    fn input_round_trip_is_flat_and_exact() {
        let msg = ClientMsg::Input {
            seq: 7,
            input: CharacterInput {
                move_x: -1,
                move_y: 1,
                aim: 0.1 + 0.2,
            },
        };
        let text = encode(&msg).unwrap();
        assert_eq!(
            text,
            r#"{"type":"input","seq":7,"move_x":-1,"move_y":1,"aim":0.30000000000000004}"#
        );
        assert_eq!(decode_client(&text, 2048).unwrap(), msg);
    }

    #[test]
    fn state_round_trip_keeps_every_bit() {
        let me = Character {
            id: PlayerId(3),
            pos: Vec2::new(12.345_678_901_234_567, 1.0 / 3.0),
            vel: Vec2::new(-0.133_333_333_333_333_33, 1e-17),
            facing: std::f64::consts::FRAC_PI_4,
        };
        let msg = ServerMsg::State {
            tick: 99,
            ack: 5,
            me: Some(me.clone()),
            others: vec![],
        };
        match decode_server(&encode(&msg).unwrap()).unwrap() {
            ServerMsg::State { me: Some(got), .. } => {
                assert_eq!(got.pos.x.to_bits(), me.pos.x.to_bits());
                assert_eq!(got.pos.y.to_bits(), me.pos.y.to_bits());
                assert_eq!(got.vel.x.to_bits(), me.vel.x.to_bits());
                assert_eq!(got.vel.y.to_bits(), me.vel.y.to_bits());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bad_client_messages_are_errors_not_panics() {
        let cases = [
            r#"{"type":"input","seq":1,"move_x":2,"move_y":0,"aim":0}"#,
            r#"{"type":"input","seq":1,"move_x":300,"move_y":0,"aim":0}"#,
            r#"{"type":"input","seq":1,"move_x":0,"move_y":0,"aim":1e999}"#,
            r#"{"type":"input","seq":-1,"move_x":0,"move_y":0,"aim":0}"#,
            r#"{"type":"teleport","x":1}"#,
            r#"{"type":"ping"}"#,
            "not json",
            "",
        ];
        for c in cases {
            assert!(decode_client(c, 2048).is_err(), "{c}");
        }
        let big = format!(
            r#"{{"type":"join","protocol":1,"room":"A","name":"{}"}}"#,
            "x".repeat(4000)
        );
        assert!(matches!(
            decode_client(&big, 2048),
            Err(ProtocolError::TooLarge { .. })
        ));
    }

    #[test]
    fn room_code_and_name_rules() {
        assert_eq!(normalize_room_code(" ab12 ").as_deref(), Some("AB12"));
        assert_eq!(normalize_room_code(""), None);
        assert_eq!(normalize_room_code("방"), None);
        assert_eq!(normalize_room_code("ABCDEFGHIJKLM"), None);
        assert_eq!(normalize_name("  민수 ").as_deref(), Some("민수"));
        assert_eq!(normalize_name("a\u{0007}b"), None);
        assert_eq!(normalize_name(&"가".repeat(17)), None);
    }
}
