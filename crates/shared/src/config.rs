//! `config/balance.toml`의 파싱과 범위 검증. 파일을 읽는 것은 호출하는 쪽의 몫입니다.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BalanceConfig {
    pub sim: SimSection,
    pub round: RoundSection,
    pub survivor: SurvivorSection,
    pub common: CommonSection,
    pub objective: PointCountSection,
    pub restraint: PointCountSection,
    pub respawn: PointCountSection,
    pub camera: CameraSection,
    pub net: NetSection,
    pub client: ClientSection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimSection {
    pub tick_rate: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundSection {
    pub survivor_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurvivorSection {
    pub run_speed: f64,
    pub radius: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommonSection {
    pub acceleration: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointCountSection {
    pub point_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraSection {
    pub view_height: f64,
    pub max_aspect_w: f64,
    pub max_aspect_h: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetSection {
    pub input_buffer_target: u32,
    pub input_buffer_grow_max: u32,
    pub input_buffer_shrink_after: f64,
    pub input_max_per_tick: u32,
    pub input_max_substitute: u32,
    pub input_drift_allowance: f64,
    pub input_burst: f64,
    pub input_queue_max: u32,
    pub max_catchup_ticks: u32,
    pub max_message_bytes: u32,
    pub max_messages_per_second: u32,
    pub join_timeout: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientSection {
    pub interp_delay: f64,
    pub correction_smoothing: f64,
    pub correction_snap: f64,
    pub max_catchup_ticks: u32,
    pub max_pending_inputs: u32,
    pub ping_interval: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// TOML 문법 오류, 빠진 항목, 모르는 항목, 타입 오류
    Parse(String),
    /// 범위를 벗어난 값들 (한 번에 모두 알려 줍니다)
    OutOfRange(Vec<String>),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Parse(msg) => write!(f, "설정 파일을 읽지 못했습니다: {msg}"),
            ConfigError::OutOfRange(items) => {
                write!(
                    f,
                    "설정 값이 허용 범위를 벗어났습니다: {}",
                    items.join("; ")
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

struct RangeCheck(Vec<String>);

impl RangeCheck {
    fn f(&mut self, name: &str, v: f64, min: f64, max: f64) {
        if !(v.is_finite() && v >= min && v <= max) {
            self.0.push(format!("{name} = {v} (허용 {min} ~ {max})"));
        }
    }
    fn u(&mut self, name: &str, v: u32, min: u32, max: u32) {
        if v < min || v > max {
            self.0.push(format!("{name} = {v} (허용 {min} ~ {max})"));
        }
    }
}

impl BalanceConfig {
    pub fn from_toml_str(text: &str) -> Result<Self, ConfigError> {
        let cfg: BalanceConfig =
            toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut c = RangeCheck(Vec::new());
        c.u("sim.tick_rate", self.sim.tick_rate, 10, 120);
        c.u("round.survivor_count", self.round.survivor_count, 1, 8);
        c.f("survivor.run_speed", self.survivor.run_speed, 0.1, 20.0);
        // 반지름이 0.5 이상이면 폭 1칸 통로를 지나갈 수 없습니다.
        c.f("survivor.radius", self.survivor.radius, 0.05, 0.49);
        c.f("common.acceleration", self.common.acceleration, 1.0, 1000.0);
        c.u("objective.point_count", self.objective.point_count, 0, 64);
        c.u("restraint.point_count", self.restraint.point_count, 0, 64);
        c.u("respawn.point_count", self.respawn.point_count, 1, 64);
        c.f("camera.view_height", self.camera.view_height, 4.0, 200.0);
        c.f("camera.max_aspect_w", self.camera.max_aspect_w, 1.0, 100.0);
        c.f("camera.max_aspect_h", self.camera.max_aspect_h, 1.0, 100.0);
        let n = &self.net;
        c.u("net.input_buffer_target", n.input_buffer_target, 0, 30);
        c.u("net.input_buffer_grow_max", n.input_buffer_grow_max, 0, 30);
        c.f(
            "net.input_buffer_shrink_after",
            n.input_buffer_shrink_after,
            0.5,
            600.0,
        );
        c.u("net.input_max_per_tick", n.input_max_per_tick, 1, 8);
        c.u("net.input_max_substitute", n.input_max_substitute, 0, 60);
        c.f(
            "net.input_drift_allowance",
            n.input_drift_allowance,
            0.0,
            0.2,
        );
        c.f("net.input_burst", n.input_burst, 1.0, 30.0);
        c.u("net.input_queue_max", n.input_queue_max, 1, 1000);
        if n.input_queue_max < n.input_buffer_target {
            c.0.push(format!(
                "net.input_queue_max = {}는 net.input_buffer_target = {} 이상이어야 합니다",
                n.input_queue_max, n.input_buffer_target
            ));
        }
        c.u("net.max_catchup_ticks", n.max_catchup_ticks, 1, 60);
        c.u("net.max_message_bytes", n.max_message_bytes, 256, 1 << 20);
        c.u(
            "net.max_messages_per_second",
            n.max_messages_per_second,
            10,
            10_000,
        );
        c.f("net.join_timeout", n.join_timeout, 0.5, 120.0);
        let cl = &self.client;
        c.f("client.interp_delay", cl.interp_delay, 0.0, 2.0);
        c.f(
            "client.correction_smoothing",
            cl.correction_smoothing,
            0.0,
            2.0,
        );
        c.f("client.correction_snap", cl.correction_snap, 0.001, 100.0);
        c.u("client.max_catchup_ticks", cl.max_catchup_ticks, 1, 120);
        c.u(
            "client.max_pending_inputs",
            cl.max_pending_inputs,
            1,
            10_000,
        );
        c.f("client.ping_interval", cl.ping_interval, 0.05, 60.0);
        if c.0.is_empty() {
            Ok(())
        } else {
            Err(ConfigError::OutOfRange(c.0))
        }
    }

    /// 방 정원. 살인마는 언제나 1명입니다.
    pub fn room_capacity(&self) -> usize {
        self.round.survivor_count as usize + 1
    }
}

/// 규칙 코드가 쓰는 틱 단위 수치. 초 단위 설정값을 한 번만 환산해 둡니다.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimParams {
    pub tick_rate: u32,
    /// 한 틱의 속도 상한 (m/틱)
    pub run_speed: f64,
    /// 한 틱에 바뀔 수 있는 속도의 크기 (m/틱²)
    pub acceleration: f64,
    pub radius: f64,
}

impl SimParams {
    pub fn from_config(cfg: &BalanceConfig) -> Self {
        let tr = f64::from(cfg.sim.tick_rate);
        SimParams {
            tick_rate: cfg.sim.tick_rate,
            run_speed: cfg.survivor.run_speed / tr,
            acceleration: cfg.common.acceleration / (tr * tr),
            radius: cfg.survivor.radius,
        }
    }

    pub fn tick_ms(&self) -> f64 {
        1000.0 / f64::from(self.tick_rate)
    }

    pub fn seconds_to_ticks(&self, seconds: f64) -> f64 {
        seconds * f64::from(self.tick_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = include_str!("../../../config/balance.toml");

    #[test]
    fn repository_config_is_valid() {
        let cfg = BalanceConfig::from_toml_str(REAL).unwrap();
        assert_eq!(cfg.sim.tick_rate, 30);
        assert_eq!(cfg.room_capacity(), 4);
    }

    #[test]
    fn out_of_range_values_are_all_reported() {
        let text = REAL
            .replace("tick_rate = 30", "tick_rate = 5")
            .replace("radius = 0.35", "radius = 0.8");
        match BalanceConfig::from_toml_str(&text) {
            Err(ConfigError::OutOfRange(items)) => {
                assert_eq!(items.len(), 2, "{items:?}");
                assert!(items[0].contains("sim.tick_rate"));
                assert!(items[1].contains("survivor.radius"));
            }
            other => panic!("범위 오류가 나야 합니다: {other:?}"),
        }
    }

    #[test]
    fn unknown_and_missing_fields_are_rejected() {
        let typo = REAL.replace("run_speed", "run_sped");
        assert!(matches!(
            BalanceConfig::from_toml_str(&typo),
            Err(ConfigError::Parse(_))
        ));
        let missing = REAL.replace("acceleration = 40.0", "");
        assert!(matches!(
            BalanceConfig::from_toml_str(&missing),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn seconds_are_converted_to_ticks_once() {
        let cfg = BalanceConfig::from_toml_str(REAL).unwrap();
        let p = SimParams::from_config(&cfg);
        assert!((p.run_speed - 4.0 / 30.0).abs() < 1e-12);
        assert!((p.acceleration - 40.0 / 900.0).abs() < 1e-12);
    }
}
