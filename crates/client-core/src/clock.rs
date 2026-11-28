//! 서버 시각 추정과 보간에 쓰는 재생 시간축. 시계를 직접 읽지 않고 호출하는 쪽이 넘긴 `now_ms`만 씁니다.

use std::collections::VecDeque;

/// 지연 측정 응답으로 왕복 지연과 서버 틱을 추정합니다.
/// 최근 표본 중 왕복 지연이 가장 짧은 것을 믿습니다. 흔들림으로 늦어진 표본일수록 추정이 틀리기 때문입니다.
#[derive(Debug, Default)]
pub struct ServerClock {
    samples: VecDeque<ClockSample>,
    smoothed_rtt_ms: Option<f64>,
    last_rtt_ms: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct ClockSample {
    rtt_ms: f64,
    /// 응답을 받은 클라이언트 시각과, 그 순간의 서버 틱 추정값
    recv_ms: f64,
    server_tick_at_recv: f64,
}

const CLOCK_SAMPLES: usize = 16;
const RTT_SMOOTHING: f64 = 0.2;

impl ServerClock {
    /// `rate`는 클라이언트 시계 1ms당 서버 틱 수입니다 (`RateEstimator`).
    pub fn on_pong(&mut self, client_time: f64, server_tick: f64, now_ms: f64, rate: f64) {
        let rtt = now_ms - client_time;
        if !rtt.is_finite() || rtt < 0.0 || !server_tick.is_finite() {
            return;
        }
        self.last_rtt_ms = Some(rtt);
        self.smoothed_rtt_ms = Some(match self.smoothed_rtt_ms {
            None => rtt,
            Some(s) => s + (rtt - s) * RTT_SMOOTHING,
        });
        self.samples.push_back(ClockSample {
            rtt_ms: rtt,
            recv_ms: now_ms,
            server_tick_at_recv: server_tick + rtt * 0.5 * rate,
        });
        while self.samples.len() > CLOCK_SAMPLES {
            self.samples.pop_front();
        }
    }

    pub fn rtt_ms(&self) -> Option<f64> {
        self.smoothed_rtt_ms
    }

    pub fn last_rtt_ms(&self) -> Option<f64> {
        self.last_rtt_ms
    }

    /// 지금 서버가 진행 중인 틱의 추정값 (소수 포함). 표본 이후의 시간은 `rate`로 외삽합니다.
    pub fn server_tick(&self, now_ms: f64, rate: f64) -> Option<f64> {
        let best = self
            .samples
            .iter()
            .min_by(|a, b| a.rtt_ms.total_cmp(&b.rtt_ms))?;
        Some(best.server_tick_at_recv + (now_ms - best.recv_ms) * rate)
    }
}

/// 다른 캐릭터를 그릴 시각. 상태 메시지가 도착한 시각을 기준으로, 가장 빨리 도착한 쪽에 맞춘 시간축에서
/// 보간 지연만큼 뒤를 재생합니다. 목표에서 벗어나면 재생 속도를 조금만 바꿔 천천히 따라가서 화면이 튀지 않게 합니다.
#[derive(Debug)]
pub struct Playback {
    params: PlaybackParams,
    arrivals: VecDeque<(f64, f64)>,
    tick: Option<f64>,
}

/// `config/balance.toml`의 `[client]` 값을 ms와 틱으로 한 번 환산해 둔 것
#[derive(Debug, Clone, Copy)]
pub struct PlaybackParams {
    /// 재생 시각을 정할 때 보는 최근 도착 기록의 길이
    pub arrival_window_ms: f64,
    /// 재생 속도를 바꿀 수 있는 폭 (±비율)
    pub rate_adjust: f64,
    /// 이미 가장 새 상태를 지나쳐 멈춰 있을 때 늦출 수 있는 폭 (비율)
    pub starved_slowdown: f64,
    /// 목표와 이만큼 넘게 벌어지면 따라가지 않고 바로 맞춤 (틱)
    pub snap_ticks: f64,
}

impl Playback {
    pub fn new(params: PlaybackParams) -> Self {
        Playback {
            params,
            arrivals: VecDeque::new(),
            tick: None,
        }
    }

    pub fn on_state(&mut self, tick: u64, now_ms: f64) {
        self.arrivals.push_back((now_ms, tick as f64));
        while let Some(&(t, _)) = self.arrivals.front() {
            if now_ms - t > self.params.arrival_window_ms && self.arrivals.len() > 1 {
                self.arrivals.pop_front();
            } else {
                break;
            }
        }
    }

    /// 가장 빨리 도착한 쪽보다 `delay_ticks`만큼 뒤, 그리고 창 안에서 가장 늦게 도착한 상태보다도 한 틱 뒤.
    /// 한 틱을 더 두는 것은 보간하려면 재생 시각 뒤에 상태가 하나 더 있어야 하기 때문입니다.
    /// 도착이 고르면 설정한 지연 그대로이고, 흔들림이 크면 그만큼만 더 늦춰 끊기지 않게 합니다.
    /// `rate`는 클라이언트 시계 1ms당 서버 틱 수입니다 (`RateEstimator`).
    fn target(&self, now_ms: f64, rate: f64, delay_ticks: f64) -> Option<f64> {
        let offsets = self.arrivals.iter().map(|&(t, k)| k - t * rate);
        let earliest = offsets.clone().max_by(|a, b| a.total_cmp(b))?;
        let latest = offsets.min_by(|a, b| a.total_cmp(b))?;
        Some(now_ms * rate + (earliest - delay_ticks).min(latest - 1.0))
    }

    /// `newest_tick`은 지금까지 받은 가장 새 상태의 틱입니다.
    pub fn advance(
        &mut self,
        now_ms: f64,
        dt_ms: f64,
        rate: f64,
        delay_ticks: f64,
        newest_tick: Option<u64>,
    ) {
        let Some(target) = self.target(now_ms, rate, delay_ticks) else {
            return;
        };
        let next = match self.tick {
            None => target,
            Some(t) => {
                let dt_ticks = dt_ms * rate;
                let t = t + dt_ticks;
                let err = target - t;
                if err.abs() > self.params.snap_ticks {
                    target
                } else {
                    let starved = newest_tick.is_some_and(|n| t > n as f64);
                    let down = if starved {
                        self.params.starved_slowdown
                    } else {
                        self.params.rate_adjust
                    };
                    t + err.clamp(-dt_ticks * down, dt_ticks * self.params.rate_adjust)
                }
            }
        };
        self.tick = Some(next);
    }

    pub fn tick(&self) -> Option<f64> {
        self.tick
    }

    /// 창 안에서 가장 빨리 온 상태와 가장 늦게 온 상태의 도착 시각 차이 (틱). 보간 지연보다 크면 끊김이 생깁니다.
    pub fn arrival_spread_ticks(&self, rate: f64) -> f64 {
        let offsets = self.arrivals.iter().map(|&(t, k)| k - t * rate);
        match (
            offsets.clone().max_by(|a, b| a.total_cmp(b)),
            offsets.min_by(|a, b| a.total_cmp(b)),
        ) {
            (Some(hi), Some(lo)) => hi - lo,
            _ => 0.0,
        }
    }
}

/// 서버 틱이 클라이언트 시계로 얼마나 빨리 가는지 (1ms당 틱 수). 두 기계의 시계는 빠르기가 조금씩 다르고,
/// 이 개발 환경에서는 WSL 시계가 Windows 시계보다 약 2.5% 느렸습니다. 로컬 예측 틱과 보간 재생을 명목 틱레이트가 아니라
/// 이 값으로 진행해야 서버가 입력을 쓰는 속도와 클라이언트가 입력을 만드는 속도가 같아집니다.
/// 최근 상태들의 (도착 시각, 틱)에 직선을 맞춘 기울기이고, 표본이 모자라면 명목값을 씁니다.
#[derive(Debug)]
pub struct RateEstimator {
    params: RateParams,
    samples: VecDeque<(f64, f64)>,
}

/// `config/balance.toml`의 `[client]` 값을 ms로 한 번 환산해 둔 것
#[derive(Debug, Clone, Copy)]
pub struct RateParams {
    pub window_ms: f64,
    /// 기록이 이보다 짧으면 명목값을 씀
    pub min_span_ms: f64,
    /// 명목값에서 이보다 많이 벗어난 추정은 측정 오류로 보고 잘라 냄 (비율)
    pub max_deviation: f64,
}

impl RateEstimator {
    pub fn new(params: RateParams) -> Self {
        RateEstimator {
            params,
            samples: VecDeque::new(),
        }
    }

    pub fn push(&mut self, now_ms: f64, tick: u64) {
        self.samples.push_back((now_ms, tick as f64));
        while self.samples.len() > 2
            && self
                .samples
                .front()
                .is_some_and(|&(t, _)| now_ms - t > self.params.window_ms)
        {
            self.samples.pop_front();
        }
    }

    pub fn ticks_per_ms(&self, nominal: f64) -> f64 {
        let (Some(&(first, _)), Some(&(last, _))) = (self.samples.front(), self.samples.back())
        else {
            return nominal;
        };
        if last - first < self.params.min_span_ms {
            return nominal;
        }
        let n = self.samples.len() as f64;
        let mean_t = self.samples.iter().map(|&(t, _)| t).sum::<f64>() / n;
        let mean_k = self.samples.iter().map(|&(_, k)| k).sum::<f64>() / n;
        let (mut num, mut den) = (0.0, 0.0);
        for &(t, k) in &self.samples {
            num += (t - mean_t) * (k - mean_k);
            den += (t - mean_t) * (t - mean_t);
        }
        let slope = num / den;
        if !slope.is_finite() {
            return nominal;
        }
        slope.clamp(
            nominal * (1.0 - self.params.max_deviation),
            nominal * (1.0 + self.params.max_deviation),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playback() -> Playback {
        Playback::new(PlaybackParams {
            arrival_window_ms: 2000.0,
            rate_adjust: 0.1,
            starved_slowdown: 0.5,
            snap_ticks: 10.0,
        })
    }

    fn rate() -> RateEstimator {
        RateEstimator::new(RateParams {
            window_ms: 10_000.0,
            min_span_ms: 2_000.0,
            max_deviation: 0.05,
        })
    }

    #[test]
    fn clock_prefers_lowest_rtt_sample() {
        let mut c = ServerClock::default();
        let tick_ms = 1000.0 / 30.0;
        // 서버 틱 = now_ms / tick_ms + 100 이 참값. 편도 지연이 대칭인 표본과 응답만 늦은 표본
        let rate = 1.0 / tick_ms;
        c.on_pong(0.0, (50.0) / tick_ms + 100.0, 100.0, rate);
        c.on_pong(1000.0, (1050.0) / tick_ms + 100.0, 1300.0, rate);
        let est = c.server_tick(2000.0, rate).unwrap();
        assert!((est - (2000.0 / tick_ms + 100.0)).abs() < 1e-9, "{est}");
        assert!(c.rtt_ms().unwrap() > 100.0);
    }

    #[test]
    fn rate_follows_a_server_clock_that_runs_slow() {
        let mut r = rate();
        let nominal = 30.0 / 1000.0;
        assert_eq!(r.ticks_per_ms(nominal), nominal, "표본이 없으면 명목값");
        // 서버가 클라이언트 시계로 초당 29.25틱을 돌고, 도착 시각이 ±15ms 흔들림
        let mut rng = shared::rng::Rng::new(1, 1);
        for tick in 0..600u64 {
            let now = tick as f64 * (1000.0 / 29.25) + 75.0 + (rng.next_f64() - 0.5) * 30.0;
            r.push(now, tick);
        }
        let est = r.ticks_per_ms(nominal) * 1000.0;
        assert!((est - 29.25).abs() < 0.05, "{est}");
    }

    #[test]
    fn playback_runs_behind_newest_and_glides() {
        let mut p = playback();
        let tick_ms = 1000.0 / 30.0;
        let delay = 3.0;
        let mut now = 0.0;
        for tick in 0..300u64 {
            now = tick as f64 * tick_ms + 75.0;
            p.on_state(tick, now);
            p.advance(now, tick_ms, 1.0 / tick_ms, delay, Some(tick));
            let t = p.tick().unwrap();
            assert!(
                t <= tick as f64 - delay + 1e-6,
                "최신 상태보다 지연만큼 뒤에 있어야 함"
            );
        }
        // 도착이 갑자기 빨라져도 한 프레임에 크게 뛰지 않음
        let before = p.tick().unwrap();
        p.on_state(300, now + 10.0);
        p.advance(now + 10.0, 10.0, 1.0 / tick_ms, delay, Some(300));
        let after = p.tick().unwrap();
        assert!(after - before <= 10.0 / tick_ms * 1.1 + 1e-9);
    }
}
