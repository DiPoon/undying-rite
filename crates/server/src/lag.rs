//! 연결별 지연 주입. 보내고 받는 메시지를 각각 지정한 만큼 늦춥니다.
//! `rtt_ms`는 왕복 지연이라 편도는 절반이고, 흔들림은 편도마다 ±jitter/2라서 왕복은 최대 ±jitter입니다.
//! WebSocket은 TCP 위라 순서가 바뀌지 않으므로, 앞 메시지보다 먼저 도착하지 않게 도착 시각을 앞 메시지 이후로 미룹니다.

use shared::rng::Rng;
use std::collections::VecDeque;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};

/// 주입할 수 있는 지연의 상한 (ms). 잘못된 값으로 연결이 사실상 멈추는 것을 막습니다.
pub const MAX_LAG_MS: u32 = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LagSpec {
    pub rtt_ms: u32,
    pub jitter_ms: u32,
}

impl LagSpec {
    pub fn new(rtt_ms: u32, jitter_ms: u32) -> Self {
        LagSpec {
            rtt_ms: rtt_ms.min(MAX_LAG_MS),
            jitter_ms: jitter_ms.min(MAX_LAG_MS),
        }
    }

    pub fn is_zero(&self) -> bool {
        self.rtt_ms == 0 && self.jitter_ms == 0
    }

    fn one_way(&self, rng: &mut Rng) -> Duration {
        let base = f64::from(self.rtt_ms) * 0.5;
        let jitter = f64::from(self.jitter_ms) * (rng.next_f64() - 0.5);
        Duration::from_secs_f64((base + jitter).max(0.0) / 1000.0)
    }
}

/// 넣은 순서대로, 각각 편도 지연만큼 늦게 내보내는 채널을 만듭니다. 지연이 0이면 바로 내보냅니다.
pub fn delay_line<T: Send + 'static>(
    lag: LagSpec,
    seed: u64,
) -> (mpsc::UnboundedSender<T>, mpsc::UnboundedReceiver<T>) {
    let (in_tx, mut in_rx) = mpsc::unbounded_channel::<T>();
    let (out_tx, out_rx) = mpsc::unbounded_channel::<T>();
    tokio::spawn(async move {
        let mut rng = Rng::new(seed, 2);
        let mut queue: VecDeque<(Instant, T)> = VecDeque::new();
        let mut last_due = Instant::now();
        let mut input_open = true;
        loop {
            if !input_open && queue.is_empty() {
                return;
            }
            let next_due = queue.front().map(|(d, _)| *d);
            tokio::select! {
                msg = in_rx.recv(), if input_open => match msg {
                    Some(m) if lag.is_zero() => {
                        if out_tx.send(m).is_err() {
                            return;
                        }
                    }
                    Some(m) => {
                        let due = (Instant::now() + lag.one_way(&mut rng)).max(last_due);
                        last_due = due;
                        queue.push_back((due, m));
                    }
                    None => input_open = false,
                },
                () = sleep_until(next_due.unwrap_or_else(Instant::now)), if next_due.is_some() => {
                    let now = Instant::now();
                    while queue.front().is_some_and(|(d, _)| *d <= now) {
                        if let Some((_, m)) = queue.pop_front()
                            && out_tx.send(m).is_err()
                        {
                            return;
                        }
                    }
                }
            }
        }
    });
    (in_tx, out_rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn keeps_order_and_delays_within_bounds() {
        let lag = LagSpec::new(100, 60);
        let (tx, mut rx) = delay_line::<(u32, Instant)>(lag, 9);
        tokio::spawn(async move {
            for i in 0..50u32 {
                tx.send((i, Instant::now())).unwrap();
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        });
        let mut expect = 0;
        while let Some((i, sent)) = rx.recv().await {
            assert_eq!(i, expect, "순서가 유지되어야 함");
            let d = sent.elapsed().as_secs_f64() * 1000.0;
            assert!(d >= 19.0, "편도 최소 50-30=20ms 근처: {d}");
            assert!(d < 150.0, "편도 최대 80ms + 순서 대기: {d}");
            expect += 1;
        }
        assert_eq!(expect, 50);
    }

    #[tokio::test]
    async fn zero_lag_passes_through() {
        let (tx, mut rx) = delay_line::<u8>(LagSpec::default(), 1);
        tx.send(7).unwrap();
        assert_eq!(rx.recv().await, Some(7));
    }
}
