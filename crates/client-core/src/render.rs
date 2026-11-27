//! 화면에 그릴 상태. TypeScript가 다시 계산할 것이 없도록 화면 변환과 글자 위치까지 여기서 정합니다.

use serde::Serialize;
use shared::math::Vec2;
use shared::protocol::RejectReason;
use shared::sim::PlayerId;

/// 세계 좌표(m)를 화면 픽셀로 바꾸는 변환. 화면 픽셀 = 세계 좌표 × scale + origin
/// view_*는 실제로 세계를 보여 주는 사각형이고, 그 밖은 가립니다 (최대 가로세로 비를 넘는 부분).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Camera {
    pub view_x: f64,
    pub view_y: f64,
    pub view_w: f64,
    pub view_h: f64,
    pub scale: f64,
    pub origin_x: f64,
    pub origin_y: f64,
}

impl Camera {
    pub fn compute(
        canvas_w: f64,
        canvas_h: f64,
        center: Vec2,
        view_height_m: f64,
        max_aspect: f64,
    ) -> Option<Camera> {
        if !(canvas_w.is_finite() && canvas_h.is_finite() && canvas_w > 0.0 && canvas_h > 0.0) {
            return None;
        }
        let view_w = canvas_w.min(canvas_h * max_aspect);
        let view_h = canvas_h;
        let view_x = (canvas_w - view_w) * 0.5;
        let view_y = 0.0;
        let scale = view_h / view_height_m;
        Some(Camera {
            view_x,
            view_y,
            view_w,
            view_h,
            scale,
            origin_x: view_x + view_w * 0.5 - center.x * scale,
            origin_y: view_y + view_h * 0.5 - center.y * scale,
        })
    }

    pub fn world_to_screen(&self, p: Vec2) -> (f64, f64) {
        (
            p.x * self.scale + self.origin_x,
            p.y * self.scale + self.origin_y,
        )
    }

    pub fn screen_to_world(&self, x: f64, y: f64) -> Vec2 {
        Vec2::new(
            (x - self.origin_x) / self.scale,
            (y - self.origin_y) / self.scale,
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderCharacter {
    pub id: PlayerId,
    pub name: String,
    pub is_self: bool,
    pub x: f64,
    pub y: f64,
    pub facing: f64,
    pub radius: f64,
    /// 이름표를 놓을 화면 픽셀 위치 (캐릭터 위쪽)
    pub label_x: f64,
    pub label_y: f64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DebugInfo {
    pub rtt_ms: Option<f64>,
    pub server_tick: Option<f64>,
    /// 다른 캐릭터를 서버 시각보다 얼마나 늦게 그리는지
    pub render_delay_ms: Option<f64>,
    pub pending_inputs: usize,
    pub seq: u32,
    pub ack: u32,
    /// 가장 최근 서버 상태에서 잰 예측 오차
    pub last_error_cm: f64,
    /// 최근 5초 동안의 가장 큰 예측 오차
    pub recent_max_error_cm: f64,
    /// 지금 화면에 남아 있는 보정 오프셋의 크기
    pub visual_offset_cm: f64,
    pub snaps: u64,
    /// 다른 캐릭터 보간이 새 상태를 기다리며 멈춘 프레임의 비율 (접속 뒤 전체)
    pub interp_starved_pct: f64,
    /// 같은 비율의 최근 값 (약 5초 시간 상수의 이동 평균)
    pub recent_starved_pct: f64,
    /// 최근 2초 동안 서버 상태 도착 시각의 흔들림 폭
    pub arrival_spread_ms: f64,
    /// 가장 새 서버 상태가 재생 시각보다 얼마나 앞서 있는지. 0 아래로 내려가면 보간이 멈춥니다.
    pub interp_margin_ms: Option<f64>,
    /// 서버 틱이 이 클라이언트의 시계로 초당 몇 번 도는지 (명목값은 설정의 tick_rate)
    pub server_tick_rate: f64,
    pub fps: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Connecting,
    Joined,
    Rejected,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderState {
    pub status: Status,
    pub reject: Option<RejectReason>,
    pub error: Option<String>,
    /// 맵이 바뀌면 올라갑니다. TypeScript는 이 값이 바뀔 때만 `map_json`을 다시 받아 그립니다.
    pub map_rev: u32,
    pub room: Option<String>,
    pub camera: Option<Camera>,
    pub characters: Vec<RenderCharacter>,
    pub debug: DebugInfo,
}

#[derive(Debug, Clone, Serialize)]
pub struct MapRender {
    pub width: i32,
    pub height: i32,
    /// rows[y][x]가 타일 (x, y)의 종류 이름
    pub rows: Vec<Vec<shared::Tile>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_canvas_is_letterboxed_to_max_aspect() {
        let c = Camera::compute(2000.0, 900.0, Vec2::new(10.0, 5.0), 24.0, 16.0 / 9.0).unwrap();
        assert!((c.view_w - 1600.0).abs() < 1e-9);
        assert!((c.view_x - 200.0).abs() < 1e-9);
        assert!((c.scale - 900.0 / 24.0).abs() < 1e-12);
        let (sx, sy) = c.world_to_screen(Vec2::new(10.0, 5.0));
        assert!(
            (sx - 1000.0).abs() < 1e-9 && (sy - 450.0).abs() < 1e-9,
            "중심이 화면 가운데"
        );
        let back = c.screen_to_world(sx + 37.5, sy);
        assert!((back.x - 11.0).abs() < 1e-9);
    }

    #[test]
    fn narrow_canvas_uses_full_width() {
        let c = Camera::compute(600.0, 900.0, Vec2::ZERO, 24.0, 16.0 / 9.0).unwrap();
        assert_eq!((c.view_x, c.view_w), (0.0, 600.0));
        assert!(Camera::compute(0.0, 900.0, Vec2::ZERO, 24.0, 1.0).is_none());
    }
}
