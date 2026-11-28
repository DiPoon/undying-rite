//! 다른 캐릭터의 보간. 받은 서버 상태를 틱 순으로 쌓아 두고 재생 시각을 감싸는 두 상태 사이를 보간합니다.

use shared::math::lerp_angle;
use shared::sim::OtherCharacter;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
struct Snapshot {
    tick: u64,
    others: Vec<OtherCharacter>,
}

#[derive(Debug, Default)]
pub struct Interpolator {
    snaps: VecDeque<Snapshot>,
    /// 재생 시각이 가장 새 상태보다 앞서서 멈춰 그린 프레임 수 (끊김의 지표)
    pub starved_frames: u64,
    pub frames: u64,
}

/// 보관할 상태의 최대 개수 (30Hz에서 약 4초)
const MAX_SNAPSHOTS: usize = 120;

impl Interpolator {
    pub fn push(&mut self, tick: u64, others: Vec<OtherCharacter>) {
        if self.snaps.back().is_some_and(|s| s.tick >= tick) {
            return;
        }
        self.snaps.push_back(Snapshot { tick, others });
        while self.snaps.len() > MAX_SNAPSHOTS {
            self.snaps.pop_front();
        }
    }

    pub fn newest_tick(&self) -> Option<u64> {
        self.snaps.back().map(|s| s.tick)
    }

    /// 재생 시각 `t`(틱, 소수 포함)의 다른 캐릭터들. 두 상태 모두에 있는 캐릭터만 보간하고,
    /// 새 상태에만 있는 캐릭터는 재생 시각이 그 상태에 닿을 때 나타납니다.
    pub fn sample(&mut self, t: f64) -> Vec<OtherCharacter> {
        self.frames += 1;
        // 재생 시각보다 두 개 이상 오래된 상태는 다시 쓸 일이 없습니다.
        while self.snaps.len() > 2 && self.snaps[1].tick as f64 <= t {
            self.snaps.pop_front();
        }
        let Some(last) = self.snaps.back() else {
            return Vec::new();
        };
        if t >= last.tick as f64 {
            if t > last.tick as f64 {
                self.starved_frames += 1;
            }
            return last.others.clone();
        }
        let (Some(first), Some(next)) = (self.snaps.front(), self.snaps.get(1)) else {
            return last.others.clone();
        };
        if t <= first.tick as f64 {
            return first.others.clone();
        }
        let span = (next.tick - first.tick) as f64;
        let alpha = ((t - first.tick as f64) / span).clamp(0.0, 1.0);
        first
            .others
            .iter()
            .map(|a| match next.others.iter().find(|b| b.id == a.id) {
                Some(b) => OtherCharacter {
                    id: a.id,
                    pos: a.pos.lerp(b.pos, alpha),
                    facing: lerp_angle(a.facing, b.facing, alpha),
                },
                None => a.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::math::Vec2;
    use shared::sim::PlayerId;

    fn other(x: f64) -> OtherCharacter {
        OtherCharacter {
            id: PlayerId(2),
            pos: Vec2::new(x, 0.0),
            facing: 0.0,
        }
    }

    #[test]
    fn interpolates_between_bracketing_snapshots() {
        let mut i = Interpolator::default();
        i.push(10, vec![other(1.0)]);
        i.push(11, vec![other(2.0)]);
        i.push(12, vec![other(4.0)]);
        assert_eq!(i.sample(10.5)[0].pos.x, 1.5);
        assert_eq!(i.sample(11.25)[0].pos.x, 2.5);
        assert_eq!(i.starved_frames, 0);
        // 가장 새 상태를 넘어가면 그 자리에 멈추고 끊김으로 셉니다.
        assert_eq!(i.sample(13.0)[0].pos.x, 4.0);
        assert_eq!(i.starved_frames, 1);
    }

    #[test]
    fn old_or_duplicate_ticks_are_ignored() {
        let mut i = Interpolator::default();
        i.push(5, vec![other(1.0)]);
        i.push(5, vec![other(9.0)]);
        i.push(4, vec![other(9.0)]);
        assert_eq!(i.newest_tick(), Some(5));
        assert_eq!(i.sample(5.0)[0].pos.x, 1.0);
    }
}
