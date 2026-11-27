//! 결정론 확인용 정해진 시나리오. 네이티브(`tools determinism`)와 WebAssembly(Node)에서 같은 함수를 돌려
//! 결과 해시를 비교합니다.

use crate::config::{BalanceConfig, SimParams};
use crate::hash::{StateHasher, hash_state};
use crate::map::Map;
use crate::rng::Rng;
use crate::sim::{CharacterInput, PlayerId, PlayerInputs, World, create_round, step};

pub const PLAYERS: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScenarioResult {
    /// 마지막 상태의 해시
    pub final_hash: u64,
    /// 틱마다의 해시를 차례로 접은 값. 중간에 한 번이라도 어긋나면 달라집니다.
    pub trace_hash: u64,
    pub ticks: u32,
}

impl ScenarioResult {
    pub fn to_line(&self) -> String {
        format!(
            "ticks={} final={:016x} trace={:016x}",
            self.ticks, self.final_hash, self.trace_hash
        )
    }
}

/// 입력열은 `seed`에서 만듭니다. 플레이어마다 가끔 방향을 바꾸고, 가끔 한 틱에 입력이 둘이거나(따라잡기)
/// 없게(입력 없음) 해서 서버가 실제로 만드는 입력 모양을 흉내 냅니다.
pub fn run(
    balance_toml: &str,
    map_text: &str,
    seed: u64,
    ticks: u32,
) -> Result<ScenarioResult, String> {
    let cfg = BalanceConfig::from_toml_str(balance_toml).map_err(|e| e.to_string())?;
    let map = Map::parse(map_text, &cfg).map_err(|e| e.to_string())?;
    let params = SimParams::from_config(&cfg);
    let ids: Vec<PlayerId> = (1..=PLAYERS).map(PlayerId).collect();
    let mut state = create_round(&cfg, &map, seed, &ids).map_err(|e| format!("{e:?}"))?;
    let world = World { map: &map };
    let mut input_rng = Rng::new(seed, 1);
    let mut held: Vec<CharacterInput> = vec![CharacterInput::default(); ids.len()];
    let mut trace = StateHasher::default();
    for _ in 0..ticks {
        let mut frame = Vec::with_capacity(ids.len());
        for (i, &id) in ids.iter().enumerate() {
            if input_rng.below(12) == 0 {
                held[i] = CharacterInput {
                    move_x: input_rng.below(3) as i8 - 1,
                    move_y: input_rng.below(3) as i8 - 1,
                    aim: input_rng.next_f64() * std::f64::consts::TAU - std::f64::consts::PI,
                };
            }
            let count = match input_rng.below(20) {
                0 => 0,
                1 => 2,
                _ => 1,
            };
            frame.push(PlayerInputs {
                player: id,
                inputs: vec![held[i]; count],
            });
        }
        step(&mut state, &world, &params, &frame);
        trace.u64(hash_state(&state));
    }
    Ok(ScenarioResult {
        final_hash: hash_state(&state),
        trace_hash: trace.finish(),
        ticks,
    })
}
