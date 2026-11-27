//! 게임 규칙 전체와 프로토콜 타입, 설정과 맵의 파싱. 파일, 네트워크, 시계, 스레드를 쓰지 않습니다.

pub mod config;
pub mod hash;
pub mod map;
pub mod math;
pub mod protocol;
pub mod rng;
pub mod scenario;
pub mod sim;

pub use config::{BalanceConfig, ConfigError, SimParams};
pub use hash::hash_state;
pub use map::{Map, MapError, Tile};
pub use math::Vec2;
pub use sim::{
    Character, CharacterInput, OtherCharacter, PlayerId, PlayerInputs, RoundState, View, World,
    create_round, step, step_character, view_for,
};
