//! 서버 틱의 처리 시간과 간격 분포

use serde::Serialize;

/// 보관할 표본 수의 상한 (30Hz에서 약 1시간)
const MAX_SAMPLES: usize = 108_000;

#[derive(Debug, Default)]
pub struct Samples {
    values_us: Vec<u32>,
}

impl Samples {
    pub fn push(&mut self, us: u64) {
        if self.values_us.len() < MAX_SAMPLES {
            self.values_us.push(u32::try_from(us).unwrap_or(u32::MAX));
        }
    }

    pub fn summary(&self) -> Distribution {
        let mut v = self.values_us.clone();
        v.sort_unstable();
        let pick = |q: f64| -> f64 {
            if v.is_empty() {
                return 0.0;
            }
            let i = ((v.len() - 1) as f64 * q).round() as usize;
            f64::from(v[i.min(v.len() - 1)]) / 1000.0
        };
        let mean = if v.is_empty() {
            0.0
        } else {
            v.iter().map(|&x| f64::from(x)).sum::<f64>() / v.len() as f64 / 1000.0
        };
        Distribution {
            count: v.len(),
            min_ms: pick(0.0),
            p50_ms: pick(0.5),
            p90_ms: pick(0.9),
            p99_ms: pick(0.99),
            max_ms: pick(1.0),
            mean_ms: mean,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Distribution {
    pub count: usize,
    pub min_ms: f64,
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub mean_ms: f64,
}

impl Distribution {
    pub fn line(&self) -> String {
        format!(
            "n={} 최소 {:.3} / 중앙 {:.3} / p90 {:.3} / p99 {:.3} / 최대 {:.3} / 평균 {:.3} ms",
            self.count,
            self.min_ms,
            self.p50_ms,
            self.p90_ms,
            self.p99_ms,
            self.max_ms,
            self.mean_ms
        )
    }
}
