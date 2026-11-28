//! 라운드 상태와 한 틱의 진행. 서버, 클라이언트 예측, 도구, 테스트가 모두 이 코드를 돌립니다.
//!
//! 결정론 규칙 (architecture.md 3절): 시계와 스레드 난수를 쓰지 않고, 시간은 틱으로만 세고,
//! 캐릭터는 식별자 순으로 처리하고, 사칙연산 밖의 수학 함수는 `libm`을 씁니다.

use crate::config::{BalanceConfig, SimParams};
use crate::map::{Map, Tile, TilePos};
use crate::math::Vec2;
use crate::rng::Rng;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlayerId(pub u32);

/// 한 틱의 입력. 이동은 8방향(각 축 -1, 0, 1), 조준은 라디안입니다.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct CharacterInput {
    pub move_x: i8,
    pub move_y: i8,
    pub aim: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputError {
    MoveOutOfRange,
    AimNotFinite,
}

impl CharacterInput {
    pub fn validate(&self) -> Result<(), InputError> {
        if !(-1..=1).contains(&self.move_x) || !(-1..=1).contains(&self.move_y) {
            return Err(InputError::MoveOutOfRange);
        }
        if !self.aim.is_finite() {
            return Err(InputError::AimNotFinite);
        }
        Ok(())
    }

    /// 크기 1 이하의 이동 방향. 대각선도 길이가 1이라 더 빠르지 않습니다.
    pub fn move_dir(&self) -> Vec2 {
        let x = f64::from(self.move_x.signum());
        let y = f64::from(self.move_y.signum());
        if x != 0.0 && y != 0.0 {
            Vec2::new(x, y) * std::f64::consts::FRAC_1_SQRT_2
        } else {
            Vec2::new(x, y)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Character {
    pub id: PlayerId,
    pub pos: Vec2,
    /// m/틱
    pub vel: Vec2,
    /// 몸 방향 (라디안, x축 기준 시계 방향이 양수인 화면 좌표계)
    pub facing: f64,
}

impl Character {
    pub fn is_finite(&self) -> bool {
        self.pos.is_finite() && self.vel.is_finite() && self.facing.is_finite()
    }
}

/// 이동 판정에 필요한 지형. 판자 상태처럼 바뀌는 것은 M1에서 여기에 더합니다.
#[derive(Debug, Clone, Copy)]
pub struct World<'a> {
    pub map: &'a Map,
}

impl World<'_> {
    pub fn blocks_movement(&self, x: i32, y: i32) -> bool {
        match self.map.tile(x, y) {
            Tile::HighWall | Tile::LowWall | Tile::Railing | Tile::Window => true,
            // 판자는 처음에 서 있어서 지나갈 수 있습니다. 내리는 규칙은 M1에서 만듭니다.
            Tile::BarrierSpot => false,
            Tile::Floor
            | Tile::Objective
            | Tile::RestraintPoint
            | Tile::SurvivorSpawn
            | Tile::KillerStart => false,
        }
    }
}

/// 부동소수점 반올림 때문에 생기는 아주 작은 겹침은 무시합니다.
const PENETRATION_EPSILON: f64 = 1e-9;
/// 밀어내기 반복 횟수. 모서리 하나와 벽 두 면을 동시에 풀기에 충분합니다.
const RESOLVE_ITERATIONS: usize = 4;
/// 한 틱의 이동을 나누는 최대 개수. 비정상적인 속도 값이 들어와도 계산량을 묶어 둡니다.
const MAX_SUBSTEPS: f64 = 64.0;

/// 캐릭터 한 명을 한 틱 진행합니다. 클라이언트 예측도 이 함수를 그대로 씁니다.
pub fn step_character(ch: &mut Character, input: &CharacterInput, world: &World, p: &SimParams) {
    let dir = input.move_dir();
    let target = dir * p.run_speed;
    let dv = target - ch.vel;
    let dv_len = dv.length();
    if dv_len <= p.acceleration {
        ch.vel = target;
    } else {
        ch.vel += dv * (p.acceleration / dv_len);
    }
    if dir != Vec2::ZERO {
        // 생존자의 몸 방향은 이동 방향을 따릅니다 (design.md 5절). M0에서는 모든 캐릭터가 이렇습니다.
        ch.facing = libm::atan2(dir.y, dir.x);
    }
    move_with_collision(ch, world, p.radius);
}

fn move_with_collision(ch: &mut Character, world: &World, radius: f64) {
    let dist = ch.vel.length();
    if dist == 0.0 {
        return;
    }
    // 한 번에 반지름의 절반보다 많이 움직이지 않게 나눠서 벽을 뚫고 지나가지 못하게 합니다.
    let n = libm::ceil(dist / (radius * 0.5)).clamp(1.0, MAX_SUBSTEPS) as u32;
    let mut step = ch.vel * (1.0 / f64::from(n));
    for _ in 0..n {
        ch.pos += step;
        for _ in 0..RESOLVE_ITERATIONS {
            let Some((normal, depth)) = deepest_penetration(ch.pos, radius, world) else {
                break;
            };
            ch.pos += normal * depth;
            // 벽으로 향하는 성분만 없애서 벽을 따라 미끄러지게 합니다.
            let into = ch.vel.dot(normal);
            if into < 0.0 {
                ch.vel -= normal * into;
            }
            let into = step.dot(normal);
            if into < 0.0 {
                step -= normal * into;
            }
        }
    }
}

/// 원과 겹치는 막힌 타일 중 가장 깊이 겹친 것의 밀어낼 방향과 깊이
fn deepest_penetration(pos: Vec2, r: f64, world: &World) -> Option<(Vec2, f64)> {
    let x0 = libm::floor(pos.x - r) as i32;
    let x1 = libm::floor(pos.x + r) as i32;
    let y0 = libm::floor(pos.y - r) as i32;
    let y1 = libm::floor(pos.y + r) as i32;
    let mut best: Option<(Vec2, f64)> = None;
    for ty in y0..=y1 {
        for tx in x0..=x1 {
            if !world.blocks_movement(tx, ty) {
                continue;
            }
            let (left, top) = (f64::from(tx), f64::from(ty));
            let (right, bottom) = (left + 1.0, top + 1.0);
            let closest = Vec2::new(pos.x.max(left).min(right), pos.y.max(top).min(bottom));
            let d = pos - closest;
            let d2 = d.length_squared();
            let hit = if d2 > 0.0 {
                if d2 >= r * r {
                    continue;
                }
                let len = libm::sqrt(d2);
                (d * (1.0 / len), r - len)
            } else {
                // 중심이 타일 안에 있으면 가장 가까운 면으로 내보냅니다.
                let exits = [
                    (pos.x - left, Vec2::new(-1.0, 0.0)),
                    (right - pos.x, Vec2::new(1.0, 0.0)),
                    (pos.y - top, Vec2::new(0.0, -1.0)),
                    (bottom - pos.y, Vec2::new(0.0, 1.0)),
                ];
                let mut e = exits[0];
                for cand in &exits[1..] {
                    if cand.0 < e.0 {
                        e = *cand;
                    }
                }
                (e.1, e.0 + r)
            };
            if hit.1 > PENETRATION_EPSILON && best.is_none_or(|b| hit.1 > b.1) {
                best = Some(hit);
            }
        }
    }
    best
}

/// M0에는 아직 사건이 없습니다. 피격, 다운 등은 M1부터 더합니다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundState {
    pub tick: u64,
    pub rng: Rng,
    /// 식별자 순으로 정렬되어 있습니다.
    pub characters: Vec<Character>,
}

/// 한 틱 동안 한 플레이어에게 적용할 입력들. 서버가 밀린 입력을 따라잡을 때는 둘 이상일 수 있습니다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerInputs {
    pub player: PlayerId,
    pub inputs: Vec<CharacterInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundError {
    AlreadyPresent,
    NoSpawnPoint,
}

pub fn create_round(
    cfg: &BalanceConfig,
    map: &Map,
    seed: u64,
    players: &[PlayerId],
) -> Result<RoundState, RoundError> {
    let mut state = RoundState {
        tick: 0,
        rng: Rng::new(seed, 0),
        characters: Vec::new(),
    };
    for &id in players {
        add_character(&mut state, cfg, map, id)?;
    }
    Ok(state)
}

fn tile_center(p: TilePos) -> Vec2 {
    Vec2::new(f64::from(p.x) + 0.5, f64::from(p.y) + 0.5)
}

/// 아무도 서 있지 않은 첫 생존자 시작 지점에 캐릭터를 놓습니다. 모두 차 있으면 첫 지점에 놓습니다.
/// 다른 캐릭터가 `respawn.occupied_distance` 안에 있으면 그 지점은 차 있다고 봅니다.
pub fn add_character(
    state: &mut RoundState,
    cfg: &BalanceConfig,
    map: &Map,
    id: PlayerId,
) -> Result<(), RoundError> {
    let idx = match state.characters.binary_search_by_key(&id, |c| c.id) {
        Ok(_) => return Err(RoundError::AlreadyPresent),
        Err(i) => i,
    };
    let spawns: Vec<Vec2> = map
        .survivor_spawns
        .iter()
        .map(|&p| tile_center(p))
        .collect();
    let first = *spawns.first().ok_or(RoundError::NoSpawnPoint)?;
    let free = spawns
        .iter()
        .copied()
        .find(|s| {
            state
                .characters
                .iter()
                .all(|c| c.pos.distance(*s) >= cfg.respawn.occupied_distance)
        })
        .unwrap_or(first);
    state.characters.insert(
        idx,
        Character {
            id,
            pos: free,
            vel: Vec2::ZERO,
            facing: 0.0,
        },
    );
    Ok(())
}

pub fn remove_character(state: &mut RoundState, id: PlayerId) {
    state.characters.retain(|c| c.id != id);
}

/// 한 틱을 진행합니다. 입력이 없는 캐릭터는 입력 없음(멈춤)으로 한 번 진행합니다.
pub fn step(
    state: &mut RoundState,
    world: &World,
    p: &SimParams,
    inputs: &[PlayerInputs],
) -> Vec<Event> {
    let idle = [CharacterInput::default()];
    for ch in &mut state.characters {
        let list = inputs
            .iter()
            .find(|pi| pi.player == ch.id)
            .map_or(&idle[..], |pi| {
                if pi.inputs.is_empty() {
                    &idle[..]
                } else {
                    &pi.inputs[..]
                }
            });
        for input in list {
            step_character(ch, input, world, p);
        }
    }
    state.tick += 1;
    Vec::new()
}

/// 다른 플레이어에게 보이는 캐릭터의 정보
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OtherCharacter {
    pub id: PlayerId,
    pub pos: Vec2,
    pub facing: f64,
}

/// 한 플레이어가 알아도 되는 정보만 담은 상태
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub tick: u64,
    pub me: Option<Character>,
    pub others: Vec<OtherCharacter>,
}

/// M0에는 시야 규칙이 없어서 모든 캐릭터가 보입니다. 시야로 거르는 것은 M1에서 여기에 만듭니다.
pub fn view_for(state: &RoundState, player: PlayerId) -> View {
    View {
        tick: state.tick,
        me: state.characters.iter().find(|c| c.id == player).cloned(),
        others: state
            .characters
            .iter()
            .filter(|c| c.id != player)
            .map(|c| OtherCharacter {
                id: c.id,
                pos: c.pos,
                facing: c.facing,
            })
            .collect(),
    }
}
