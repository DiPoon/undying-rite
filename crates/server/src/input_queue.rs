//! 플레이어별 입력 대기열 (architecture.md 4절 "입력 처리").
//!
//! 서버 틱 하나는 입력 일련번호 하나의 칸입니다.
//! - 처음에는 `input_buffer_target`개가 쌓일 때까지 기다려서 도착 시각의 흔들림을 흡수합니다.
//! - 대기열이 비면 직전 입력을 유지합니다. 이때 그 틱을 다음 일련번호의 칸으로 치고, 그 번호의 입력이 늦게 오면 버립니다.
//!   같은 키를 누르고 있는 동안에는 서버가 쓴 입력과 클라이언트가 예측에 쓴 입력이 같아서 예측이 어긋나지 않습니다.
//!   이렇게 대신 쓰는 것이 `input_max_substitute`번 이어지면 클라이언트가 멈춘 것으로 보고, 멈춤 입력으로 바꾼 뒤
//!   버퍼를 다시 채울 때까지 기다립니다 (탭이 숨겨진 동안 캐릭터가 계속 달려가지 않게).
//! - 대신 쓰기가 생기면 버퍼를 한 칸 늘립니다. 칸을 쓰지 않는 틱을 끼워 넣어야 하는데, 그 틱에 적용한 입력이 상태를
//!   바꾸면 예측이 어긋나므로, 상태가 그대로인 순간(멈춰 서 있을 때 등)을 기다렸다가 넣습니다.
//!   `input_buffer_shrink_after`초 동안 대신 쓰기가 없으면 한 칸씩 되돌립니다.
//! - 대기열이 목표보다 길면 한 틱에 둘 이상 써서 따라잡지만, 소비 여유분(틱마다 1 + 허용 비율만큼 늘고
//!   `input_burst`에서 멈춤)이 있을 때만 씁니다. 그래서 입력을 아무리 빨리 보내도 실제 시간보다 빨리 움직이지 못합니다.

use serde::Serialize;
use shared::BalanceConfig;
use shared::sim::CharacterInput;
use std::collections::VecDeque;

/// 소비 여유분을 정수로 세는 단위 (입력 하나 = 1000)
const CREDIT_UNIT: i64 = 1000;

/// 설정에서 한 번 환산해 두는 대기열 수치
#[derive(Debug, Clone, Copy)]
pub struct QueueParams {
    pub buffer_target: usize,
    pub buffer_grow_max: usize,
    pub shrink_after_ticks: u64,
    pub max_per_tick: usize,
    pub max_substitute: u32,
    pub queue_max: usize,
    credit_gain: i64,
    credit_cap: i64,
}

impl QueueParams {
    pub fn from_config(cfg: &BalanceConfig) -> Self {
        let n = &cfg.net;
        QueueParams {
            buffer_target: n.input_buffer_target as usize,
            buffer_grow_max: n.input_buffer_grow_max as usize,
            shrink_after_ticks: (n.input_buffer_shrink_after * f64::from(cfg.sim.tick_rate)) as u64,
            max_per_tick: n.input_max_per_tick as usize,
            max_substitute: n.input_max_substitute,
            queue_max: n.input_queue_max as usize,
            credit_gain: CREDIT_UNIT + (n.input_drift_allowance * CREDIT_UNIT as f64) as i64,
            credit_cap: ((n.input_burst * CREDIT_UNIT as f64) as i64).max(CREDIT_UNIT),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct QueueStats {
    /// 버퍼가 찬 뒤 대기열이 비어서 직전 입력을 대신 쓴 틱 수
    pub starved_ticks: u64,
    /// 대신 쓴 칸의 입력이 늦게 도착해서 버린 수
    pub late: u64,
    /// 대신 쓰기가 상한을 넘어 버퍼를 다시 채운 횟수
    pub rebuffers: u64,
    /// 버퍼를 한 칸 늘린 횟수와 줄인 횟수
    pub grows: u64,
    pub shrinks: u64,
    /// 한 틱에 입력을 둘 이상 쓴 틱 수
    pub catchup_ticks: u64,
    /// 대기열이 넘쳐서 버린 입력 수
    pub dropped: u64,
    /// 일련번호가 앞서 받은 것보다 크지 않아서 버린 입력 수
    pub stale: u64,
    pub max_len: usize,
    /// 마지막 시점에 기본 목표에 더해 늘려 둔 버퍼 칸 수
    pub extra_buffer: usize,
}

#[derive(Debug, Default)]
pub struct InputQueue {
    queue: VecDeque<(u32, CharacterInput)>,
    newest_seq: u32,
    ack: u32,
    last_input: CharacterInput,
    credit: i64,
    primed: bool,
    substitutes: u32,
    extra: usize,
    grow_pending: bool,
    ticks: u64,
    quiet_since: u64,
    pub stats: QueueStats,
}

impl InputQueue {
    pub fn push(&mut self, seq: u32, input: CharacterInput, p: &QueueParams) {
        if seq <= self.ack {
            self.stats.late += 1;
            return;
        }
        if seq <= self.newest_seq {
            self.stats.stale += 1;
            return;
        }
        self.newest_seq = seq;
        self.queue.push_back((seq, input));
        while self.queue.len() > p.queue_max {
            self.queue.pop_front();
            self.stats.dropped += 1;
        }
        self.stats.max_len = self.stats.max_len.max(self.queue.len());
    }

    /// 마지막으로 쓴 칸의 일련번호
    pub fn ack(&self) -> u32 {
        self.ack
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// 이번 틱에 적용할 입력들. 언제나 하나 이상입니다.
    /// `is_noop(input)`은 그 입력을 지금 한 번 더 적용해도 캐릭터 상태가 그대로인지 알려 줍니다.
    pub fn take_for_tick(
        &mut self,
        p: &QueueParams,
        is_noop: impl Fn(&CharacterInput) -> bool,
    ) -> Vec<CharacterInput> {
        self.ticks += 1;
        self.credit = (self.credit + p.credit_gain).min(p.credit_cap);
        if self.extra > 0 && self.ticks - self.quiet_since > p.shrink_after_ticks {
            self.extra -= 1;
            self.quiet_since = self.ticks;
            self.stats.shrinks += 1;
        }
        self.stats.extra_buffer = self.extra;

        if !self.primed {
            if self.queue.len() >= p.buffer_target.max(1) {
                self.primed = true;
                self.substitutes = 0;
            } else {
                self.last_input.move_x = 0;
                self.last_input.move_y = 0;
                return vec![self.last_input];
            }
        }

        if self.grow_pending && self.extra < p.buffer_grow_max && is_noop(&self.last_input) {
            // 칸을 쓰지 않는 틱. 상태가 바뀌지 않으므로 클라이언트 예측과 어긋나지 않고 버퍼만 한 칸 늘어납니다.
            self.grow_pending = false;
            self.extra += 1;
            self.stats.grows += 1;
            self.stats.extra_buffer = self.extra;
            return vec![self.last_input];
        }

        let target = p.buffer_target + self.extra;
        let mut out = Vec::new();
        while out.len() < p.max_per_tick && self.credit >= CREDIT_UNIT {
            if !out.is_empty() && self.queue.len() <= target {
                break;
            }
            let Some((seq, input)) = self.queue.pop_front() else {
                break;
            };
            self.credit -= CREDIT_UNIT;
            self.ack = seq;
            self.last_input = input;
            self.substitutes = 0;
            out.push(input);
        }
        if out.is_empty() {
            if !self.queue.is_empty() {
                // 여유분이 모자라 쓰지 못한 경우. 여유분은 틱마다 1 이상 늘어나므로 일어나지 않아야 합니다.
                out.push(self.last_input);
            } else if self.substitutes < p.max_substitute {
                // 다음 칸을 직전 입력으로 채웁니다. 이 칸도 실제 시간 한 틱을 쓴 것이므로 여유분을 씁니다.
                self.stats.starved_ticks += 1;
                self.substitutes += 1;
                self.ack = self.ack.wrapping_add(1);
                self.credit -= CREDIT_UNIT;
                self.grow_pending = true;
                self.quiet_since = self.ticks;
                out.push(self.last_input);
            } else {
                self.stats.rebuffers += 1;
                self.primed = false;
                self.last_input.move_x = 0;
                self.last_input.move_y = 0;
                out.push(self.last_input);
            }
        } else if out.len() > 1 {
            self.stats.catchup_ticks += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 대기열 로직을 보는 테스트라 버퍼 시작값은 설정 파일과 상관없이 2칸으로 고정합니다.
    fn params() -> QueueParams {
        let mut p = QueueParams::from_config(
            &BalanceConfig::from_toml_str(include_str!("../../../config/balance.toml")).unwrap(),
        );
        p.buffer_target = 2;
        p
    }

    fn input(x: i8) -> CharacterInput {
        CharacterInput {
            move_x: x,
            move_y: 0,
            aim: 0.0,
        }
    }

    /// 움직이는 중이라 어떤 입력도 상태를 바꾸는 경우
    fn take(q: &mut InputQueue, p: &QueueParams) -> Vec<CharacterInput> {
        q.take_for_tick(p, |_| false)
    }

    #[test]
    fn waits_for_buffer_then_one_per_tick() {
        let p = params();
        let mut q = InputQueue::default();
        q.push(1, input(1), &p);
        assert_eq!(
            take(&mut q, &p),
            vec![CharacterInput::default()],
            "버퍼가 차기 전에는 멈춤"
        );
        assert_eq!(q.ack(), 0);
        q.push(2, input(1), &p);
        assert_eq!(take(&mut q, &p).len(), 1);
        assert_eq!(q.ack(), 1);
        q.push(3, input(-1), &p);
        assert_eq!(take(&mut q, &p), vec![input(1)]);
        assert_eq!(q.ack(), 2);
        assert_eq!(q.stats.starved_ticks, 0);
    }

    #[test]
    fn empty_queue_repeats_last_input_in_the_next_slot() {
        let p = params();
        let mut q = InputQueue::default();
        q.push(1, input(-1), &p);
        q.push(2, input(-1), &p);
        take(&mut q, &p);
        take(&mut q, &p);
        assert_eq!(take(&mut q, &p), vec![input(-1)]);
        assert_eq!(q.stats.starved_ticks, 1);
        assert_eq!(q.ack(), 3, "대신 쓴 틱은 3번 칸");
        // 3번이 늦게 오면 버리고, 4번부터 이어서 씁니다.
        q.push(3, input(1), &p);
        q.push(4, input(1), &p);
        assert_eq!(q.stats.late, 1);
        assert_eq!(take(&mut q, &p), vec![input(1)]);
        assert_eq!(q.ack(), 4);
    }

    #[test]
    fn long_stall_stops_the_character_and_rebuffers() {
        let p = params();
        let mut q = InputQueue::default();
        q.push(1, input(1), &p);
        q.push(2, input(1), &p);
        take(&mut q, &p);
        take(&mut q, &p);
        for _ in 0..p.max_substitute {
            assert_eq!(take(&mut q, &p), vec![input(1)]);
        }
        assert_eq!(take(&mut q, &p), vec![input(0)], "상한을 넘으면 멈춤");
        assert_eq!(take(&mut q, &p), vec![input(0)]);
        assert_eq!(q.stats.rebuffers, 1);
        let next = q.ack() + 1;
        q.push(next, input(-1), &p);
        assert_eq!(
            take(&mut q, &p),
            vec![input(0)],
            "버퍼가 다시 찰 때까지 기다림"
        );
        q.push(next + 1, input(-1), &p);
        assert_eq!(take(&mut q, &p), vec![input(-1)]);
    }

    #[test]
    fn stale_and_duplicate_sequence_numbers_are_ignored() {
        let p = params();
        let mut q = InputQueue::default();
        q.push(5, input(1), &p);
        q.push(5, input(-1), &p);
        q.push(3, input(-1), &p);
        assert_eq!(q.len(), 1);
        assert_eq!(q.stats.stale, 2);
    }

    #[test]
    fn buffer_grows_only_when_the_extra_tick_changes_nothing() {
        let p = params();
        let mut q = InputQueue::default();
        q.push(1, input(1), &p);
        q.push(2, input(1), &p);
        take(&mut q, &p);
        take(&mut q, &p);
        take(&mut q, &p); // 대신 쓰기 → 늘릴 준비
        q.push(4, input(0), &p);
        q.push(5, input(0), &p);
        assert_eq!(
            take(&mut q, &p),
            vec![input(0)],
            "움직이는 중에는 늘리지 않고 칸을 씀"
        );
        assert_eq!(q.ack(), 4);
        assert_eq!(q.stats.grows, 0);
        // 멈춰 서서 한 틱을 더 넣어도 상태가 그대로일 때 칸을 쓰지 않는 틱을 넣습니다.
        assert_eq!(q.take_for_tick(&p, |_| true), vec![input(0)]);
        assert_eq!(q.ack(), 4, "칸을 쓰지 않음");
        assert_eq!(q.stats.grows, 1);
        assert_eq!(q.stats.extra_buffer, 1);
    }

    #[test]
    fn extra_buffer_shrinks_after_quiet_period() {
        let p = params();
        let mut q = InputQueue::default();
        let mut seq = 0;
        let mut push = |q: &mut InputQueue, k: usize| {
            for _ in 0..k {
                seq += 1;
                q.push(seq, input(0), &p);
            }
        };
        push(&mut q, 2);
        take(&mut q, &p);
        take(&mut q, &p);
        take(&mut q, &p); // 대신 쓰기
        push(&mut q, 2);
        q.take_for_tick(&p, |_| true); // 한 칸 늘림
        assert_eq!(q.stats.extra_buffer, 1);
        for _ in 0..(p.shrink_after_ticks + 200) {
            push(&mut q, 1);
            take(&mut q, &p);
        }
        assert_eq!(q.stats.extra_buffer, 0);
        assert_eq!(q.stats.shrinks, 1);
        assert!(
            q.len() <= p.buffer_target + 1,
            "늘렸던 칸이 따라잡기로 줄어듦: {}",
            q.len()
        );
    }

    #[test]
    fn flooding_inputs_cannot_outrun_real_time() {
        let p = params();
        let cfg =
            BalanceConfig::from_toml_str(include_str!("../../../config/balance.toml")).unwrap();
        let mut q = InputQueue::default();
        let mut seq = 0;
        let mut applied = 0usize;
        let ticks = 1800; // 30Hz에서 60초
        for _ in 0..ticks {
            // 정상의 두 배 속도로 보냄
            for _ in 0..2 {
                seq += 1;
                q.push(seq, input(1), &p);
            }
            applied += take(&mut q, &p).len();
        }
        let ratio = applied as f64 / ticks as f64;
        assert!(
            ratio <= 1.0 + cfg.net.input_drift_allowance + 0.002,
            "실제 시간 대비 {ratio}"
        );
        assert!(q.len() <= p.queue_max);
        assert!(q.stats.dropped > 0);
    }

    #[test]
    fn extra_inputs_are_caught_up_gradually() {
        let p = params();
        let mut q = InputQueue::default();
        let mut seq = 0;
        let mut push = |q: &mut InputQueue, k: usize| {
            for _ in 0..k {
                seq += 1;
                q.push(seq, input(1), &p);
            }
        };
        push(&mut q, 2);
        for _ in 0..150 {
            take(&mut q, &p);
            push(&mut q, 1);
        }
        // 클라이언트 시계가 빨라서 입력 6개를 더 보낸 상황
        push(&mut q, 6);
        let before = q.len();
        for _ in 0..300 {
            take(&mut q, &p);
            push(&mut q, 1);
        }
        assert!(q.stats.catchup_ticks >= 2, "{:?}", q.stats);
        assert!(q.len() < before, "대기열 {} → {}", before, q.len());
        assert_eq!(q.stats.starved_ticks, 0);
    }
}
