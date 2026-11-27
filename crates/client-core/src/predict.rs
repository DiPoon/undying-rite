//! 자기 캐릭터 예측과 보정 (architecture.md 5절).

use serde::Serialize;
use shared::SimParams;
use shared::math::Vec2;
use shared::sim::{Character, CharacterInput, World, step_character};
use std::collections::VecDeque;

/// 예측 오차의 분포를 나누는 경계 (m)
const ERROR_BUCKETS_M: [f64; 5] = [1e-4, 1e-3, 1e-2, 1e-1, 1.0];
/// 자동 확인의 기준. 지형 충돌만 있을 때 예측 오차가 이보다 크면 구조의 결함입니다.
pub const ONE_CM: f64 = 0.01;

#[derive(Debug, Clone, Default, Serialize)]
pub struct PredictionStats {
    /// 예측 중에 받은 서버 상태 수 (오차를 잰 횟수)
    pub samples: u64,
    pub max_error_m: f64,
    pub sum_error_m: f64,
    /// 1cm를 넘은 횟수
    pub over_1cm: u64,
    /// 오차 분포: [0], (0,0.1mm], (0.1mm,1mm], (1mm,1cm], (1cm,10cm], (10cm,1m], 1m 초과
    pub histogram: [u64; 7],
    /// 화면 위치를 즉시 맞춘 횟수 (오차가 문턱값을 넘음)
    pub snaps: u64,
    pub last_error_m: f64,
    pub max_pending: usize,
    /// 보관 한도를 넘어 버린 미확인 입력 수
    pub dropped_pending: u64,
}

impl PredictionStats {
    fn record(&mut self, err: f64) {
        self.samples += 1;
        self.sum_error_m += err;
        self.last_error_m = err;
        if err > self.max_error_m {
            self.max_error_m = err;
        }
        if err > ONE_CM {
            self.over_1cm += 1;
        }
        let bucket = if err == 0.0 {
            0
        } else {
            1 + ERROR_BUCKETS_M
                .iter()
                .position(|&b| err <= b)
                .unwrap_or(ERROR_BUCKETS_M.len())
        };
        self.histogram[bucket] += 1;
    }

    pub fn mean_error_m(&self) -> f64 {
        if self.samples == 0 {
            0.0
        } else {
            self.sum_error_m / self.samples as f64
        }
    }
}

#[derive(Debug, Default)]
pub struct Predictor {
    next_seq: u32,
    pending: VecDeque<(u32, CharacterInput)>,
    current: Option<Character>,
    previous: Option<Character>,
    /// 화면에 그리는 위치 − 예측 위치. 보정 직후의 차이를 담고 시간이 지나며 0으로 줄어듭니다.
    visual_offset: Vec2,
    pub stats: PredictionStats,
}

pub struct CorrectionResult {
    pub error_m: f64,
}

impl Predictor {
    pub fn is_active(&self) -> bool {
        self.current.is_some()
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn last_seq(&self) -> u32 {
        self.next_seq
    }

    /// 로컬 틱 하나: 입력에 일련번호를 붙여 보관하고 즉시 자기 캐릭터를 진행합니다.
    /// 아직 서버 상태를 받지 못했으면 아무것도 하지 않고 `None`을 돌려줍니다.
    pub fn local_tick(
        &mut self,
        input: CharacterInput,
        world: &World,
        params: &SimParams,
        max_pending: usize,
    ) -> Option<u32> {
        let cur = self.current.as_mut()?;
        self.previous = Some(cur.clone());
        step_character(cur, &input, world, params);
        self.next_seq = self.next_seq.wrapping_add(1).max(1);
        self.pending.push_back((self.next_seq, input));
        while self.pending.len() > max_pending {
            self.pending.pop_front();
            self.stats.dropped_pending += 1;
        }
        self.stats.max_pending = self.stats.max_pending.max(self.pending.len());
        Some(self.next_seq)
    }

    /// 서버 상태를 받았을 때: 서버 값으로 되돌리고 서버가 아직 처리하지 않은 입력을 다시 적용합니다.
    pub fn on_server_state(
        &mut self,
        ack: u32,
        server_me: Option<&Character>,
        world: &World,
        params: &SimParams,
        snap_distance: f64,
    ) -> Option<CorrectionResult> {
        while self.pending.front().is_some_and(|&(seq, _)| seq <= ack) {
            self.pending.pop_front();
        }
        let Some(server_me) = server_me else {
            self.current = None;
            self.previous = None;
            self.pending.clear();
            return None;
        };
        let mut reconciled = server_me.clone();
        for (_, input) in &self.pending {
            step_character(&mut reconciled, input, world, params);
        }
        let Some(current) = self.current.as_ref() else {
            self.previous = Some(reconciled.clone());
            self.current = Some(reconciled);
            return None;
        };
        let delta = reconciled.pos - current.pos;
        let error_m = delta.length();
        self.stats.record(error_m);
        if error_m > 0.0 {
            if error_m >= snap_distance {
                self.visual_offset = Vec2::ZERO;
                self.stats.snaps += 1;
                self.previous = Some(reconciled.clone());
            } else {
                // 그리는 위치가 그대로 이어지도록 차이를 오프셋으로 옮깁니다.
                self.visual_offset -= delta;
                if let Some(prev) = self.previous.as_mut() {
                    prev.pos += delta;
                }
            }
        }
        self.current = Some(reconciled);
        Some(CorrectionResult { error_m })
    }

    /// 화면 보정 오프셋을 시간 상수 `smoothing_ms`로 줄입니다.
    pub fn decay(&mut self, dt_ms: f64, smoothing_ms: f64) {
        if smoothing_ms <= 0.0 {
            self.visual_offset = Vec2::ZERO;
        } else {
            self.visual_offset = self.visual_offset * libm::exp(-dt_ms / smoothing_ms);
        }
    }

    pub fn visual_offset(&self) -> Vec2 {
        self.visual_offset
    }

    pub fn current(&self) -> Option<&Character> {
        self.current.as_ref()
    }

    /// 두 로컬 틱 사이를 `alpha`(0~1)로 보간한 화면 위치와 방향
    pub fn render_pose(&self, alpha: f64) -> Option<(Vec2, f64)> {
        let cur = self.current.as_ref()?;
        let prev = self.previous.as_ref().unwrap_or(cur);
        let pos = prev.pos.lerp(cur.pos, alpha.clamp(0.0, 1.0)) + self.visual_offset;
        Some((pos, cur.facing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::sim::PlayerId;
    use shared::{BalanceConfig, Map};

    fn setup() -> (Map, SimParams) {
        let cfg =
            BalanceConfig::from_toml_str(include_str!("../../../config/balance.toml")).unwrap();
        let map = Map::parse(include_str!("../../../maps/test.txt"), &cfg).unwrap();
        (map, SimParams::from_config(&cfg))
    }

    fn start() -> Character {
        Character {
            id: PlayerId(1),
            pos: Vec2::new(1.5, 9.5),
            vel: Vec2::ZERO,
            facing: 0.0,
        }
    }

    fn right() -> CharacterInput {
        CharacterInput {
            move_x: 1,
            move_y: 0,
            aim: 0.0,
        }
    }

    #[test]
    fn honest_server_gives_zero_error_under_latency() {
        let (map, p) = setup();
        let world = World { map: &map };
        let mut pred = Predictor::default();
        pred.on_server_state(0, Some(&start()), &world, &p, 1.0);
        // 서버는 입력을 5틱 늦게 받아 같은 함수로 처리합니다.
        let mut server = start();
        let mut sent = Vec::new();
        for t in 0..120u32 {
            let input = if (t / 20) % 2 == 0 {
                right()
            } else {
                CharacterInput {
                    move_x: 0,
                    move_y: 1,
                    aim: 0.0,
                }
            };
            let seq = pred.local_tick(input, &world, &p, 120).unwrap();
            sent.push((seq, input));
            if t >= 5 {
                let (seq, input) = sent[(t - 5) as usize];
                step_character(&mut server, &input, &world, &p);
                pred.on_server_state(seq, Some(&server), &world, &p, 1.0);
            }
        }
        assert_eq!(pred.stats.max_error_m, 0.0, "{:?}", pred.stats);
        assert!(pred.stats.samples > 100);
        assert_eq!(pred.pending_len(), 5);
    }

    #[test]
    fn divergence_is_corrected_smoothly_or_snapped() {
        let (map, p) = setup();
        let world = World { map: &map };
        let mut pred = Predictor::default();
        pred.on_server_state(0, Some(&start()), &world, &p, 1.0);
        pred.local_tick(right(), &world, &p, 120);
        let before = pred.render_pose(1.0).unwrap().0;
        // 서버가 5cm 다른 곳에 있다고 알려 옴
        let mut srv = start();
        srv.pos.y += 0.05;
        let r = pred
            .on_server_state(0, Some(&srv), &world, &p, 1.0)
            .unwrap();
        assert!((r.error_m - 0.05).abs() < 1e-9);
        let after = pred.render_pose(1.0).unwrap().0;
        assert!(
            before.distance(after) < 1e-9,
            "보정 직후 화면 위치는 이어져야 함"
        );
        pred.decay(1000.0, 100.0);
        assert!(
            pred.visual_offset().length() < 1e-5,
            "시간이 지나면 오프셋이 사라짐"
        );
        // 문턱값보다 크면 즉시 맞춤
        srv.pos.x += 3.0;
        pred.on_server_state(0, Some(&srv), &world, &p, 1.0);
        assert_eq!(pred.stats.snaps, 1);
        assert_eq!(pred.visual_offset(), Vec2::ZERO);
    }
}
