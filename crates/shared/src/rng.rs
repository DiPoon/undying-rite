//! 시드 기반 난수 (PCG32, XSH-RR). 규칙 코드의 난수는 전부 상태에 들어 있는 이 생성기에서 얻습니다.

use serde::{Deserialize, Serialize};

const MULTIPLIER: u64 = 6_364_136_223_846_793_005;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rng {
    state: u64,
    inc: u64,
}

impl Rng {
    pub fn new(seed: u64, stream: u64) -> Self {
        let mut rng = Rng {
            state: 0,
            inc: (stream << 1) | 1,
        };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(MULTIPLIER).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    pub fn next_u64(&mut self) -> u64 {
        (u64::from(self.next_u32()) << 32) | u64::from(self.next_u32())
    }

    /// [0, 1) 범위의 실수
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// [0, n) 범위의 정수. 치우침이 없도록 거절 표본을 씁니다. n이 0이면 0을 돌려줍니다.
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let threshold = n.wrapping_neg() % n;
        loop {
            let r = self.next_u32();
            if r >= threshold {
                return r % n;
            }
        }
    }

    pub fn state_words(&self) -> (u64, u64) {
        (self.state, self.inc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Rng::new(42, 7);
        let mut b = Rng::new(42, 7);
        for _ in 0..1000 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
        let mut c = Rng::new(43, 7);
        assert_ne!(Rng::new(42, 7).next_u64(), c.next_u64());
    }

    #[test]
    fn known_first_values() {
        // 구현이 바뀌면 리플레이가 깨지므로 첫 값을 고정해 둡니다.
        let mut r = Rng::new(42, 54);
        let got: Vec<u32> = (0..3).map(|_| r.next_u32()).collect();
        assert_eq!(got, vec![0xa15c_02b7, 0x7b47_f409, 0xba1d_3330]);
    }

    #[test]
    fn ranges() {
        let mut r = Rng::new(1, 1);
        for _ in 0..10_000 {
            let f = r.next_f64();
            assert!((0.0..1.0).contains(&f));
            assert!(r.below(9) < 9);
        }
        assert_eq!(r.below(0), 0);
    }
}
