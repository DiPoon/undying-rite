//! 상태 해시 (FNV-1a 64). 실수는 비트 그대로 넣어서 0.1mm의 차이도 해시가 달라지게 합니다.

use crate::math::Vec2;
use crate::sim::RoundState;

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

pub struct StateHasher(u64);

impl Default for StateHasher {
    fn default() -> Self {
        StateHasher(OFFSET)
    }
}

impl StateHasher {
    pub fn u64(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(PRIME);
        }
    }

    pub fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }

    pub fn vec2(&mut self, v: Vec2) {
        self.f64(v.x);
        self.f64(v.y);
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

pub fn hash_state(state: &RoundState) -> u64 {
    let mut h = StateHasher::default();
    h.u64(state.tick);
    let (s, inc) = state.rng.state_words();
    h.u64(s);
    h.u64(inc);
    h.u64(state.characters.len() as u64);
    for c in &state.characters {
        h.u64(u64::from(c.id.0));
        h.vec2(c.pos);
        h.vec2(c.vel);
        h.f64(c.facing);
    }
    h.finish()
}
