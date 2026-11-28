//! 클라이언트 로직: 서버 메시지 해석, 자기 예측과 보정, 타인 보간, 서버 시각 추정, 화면에 그릴 상태.
//! 입출력이 없어서 브라우저(WebAssembly)와 화면 없는 테스트 클라이언트(네이티브)가 같은 코드를 돌립니다.
//! 시간은 호출하는 쪽이 `now_ms`(단조 증가하는 밀리초)로 넘깁니다.

pub mod clock;
pub mod interp;
pub mod predict;
pub mod render;

use clock::{Playback, PlaybackParams, RateEstimator, RateParams, ServerClock};
use interp::Interpolator;
use predict::{PredictionStats, Predictor};
use render::{Camera, DebugInfo, MapRender, RenderCharacter, RenderState, Status};
use serde::Serialize;
use shared::math::Vec2;
use shared::protocol::{self, ClientMsg, PROTOCOL_VERSION, RejectReason, ServerMsg};
use shared::sim::{CharacterInput, PlayerId, World};
use shared::{BalanceConfig, Map, SimParams};
use std::collections::{BTreeMap, VecDeque};

const RECENT_ERROR_WINDOW_MS: f64 = 5000.0;
/// 그릴 상태를 JSON으로 만들지 못했을 때의 마지막 대체값. `RenderState`의 모든 필드를 갖춥니다.
const FALLBACK_RENDER_JSON: &str = r#"{"status":"error","reject":null,"error":"render_json 실패","map_rev":0,"room":null,"camera":null,"characters":[],"debug":{"rtt_ms":null,"server_tick":null,"render_delay_ms":null,"pending_inputs":0,"seq":0,"ack":0,"last_error_cm":0.0,"recent_max_error_cm":0.0,"visual_offset_cm":0.0,"snaps":0,"interp_starved_pct":0.0,"recent_starved_pct":0.0,"arrival_spread_ms":0.0,"interp_margin_ms":null,"server_tick_rate":0.0,"fps":0.0}}"#;
/// 최근 보간 멈춤 비율을 내는 이동 평균의 시간 상수
const RECENT_STARVED_MS: f64 = 5000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinError {
    BadRoomCode,
    BadName,
}

#[derive(Debug, Default, Clone, Copy)]
struct Controls {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
    pointer_px: Option<(f64, f64)>,
}

struct Session {
    player_id: PlayerId,
    room: String,
    cfg: BalanceConfig,
    params: SimParams,
    map: Map,
    tick_ms: f64,
    predictor: Predictor,
    remote: Interpolator,
    playback: Playback,
    clock: ServerClock,
    roster: BTreeMap<PlayerId, String>,
    last_ping_ms: Option<f64>,
    /// 다음 로컬 틱까지 쌓인 시간 (틱 단위, 0~1)
    accum_ticks: f64,
    rate: RateEstimator,
    last_state_tick: Option<u64>,
    last_ack: u32,
    last_input: CharacterInput,
    recent_errors: VecDeque<(f64, f64)>,
    last_server_pos: Option<Vec2>,
    server_distance_m: f64,
    last_render_ms: Option<f64>,
    recent_starved: f64,
    states_received: u64,
}

/// 도구가 예측 품질을 숫자로 확인할 때 쓰는 요약
#[derive(Debug, Clone, Serialize, Default)]
pub struct ClientReport {
    pub prediction: PredictionStats,
    pub interp_frames: u64,
    pub interp_starved_frames: u64,
    pub states_received: u64,
    pub decode_errors: u64,
    pub rtt_ms: Option<f64>,
    /// 서버가 확정한 자기 캐릭터 위치를 이어 붙인 이동 거리
    pub server_distance_m: f64,
}

pub struct Client {
    status: Status,
    reject: Option<RejectReason>,
    error: Option<String>,
    session: Option<Session>,
    controls: Controls,
    viewport: (f64, f64),
    outbox: Vec<String>,
    last_now: Option<f64>,
    frame_ms: Option<f64>,
    map_rev: u32,
    decode_errors: u64,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        Client {
            status: Status::Connecting,
            reject: None,
            error: None,
            session: None,
            controls: Controls::default(),
            viewport: (0.0, 0.0),
            outbox: Vec::new(),
            last_now: None,
            frame_ms: None,
            map_rev: 0,
            decode_errors: 0,
        }
    }

    /// 입장 메시지를 만듭니다. 방 코드와 이름이 규칙에 맞지 않으면 오류를 돌려줍니다.
    pub fn join_message(&mut self, room: &str, name: &str) -> Result<String, JoinError> {
        let room = protocol::normalize_room_code(room).ok_or(JoinError::BadRoomCode)?;
        let name = protocol::normalize_name(name).ok_or(JoinError::BadName)?;
        protocol::encode(&ClientMsg::Join {
            protocol: PROTOCOL_VERSION,
            room,
            name,
        })
        .map_err(|_| JoinError::BadName)
    }

    pub fn set_viewport(&mut self, width_px: f64, height_px: f64) {
        self.viewport = (width_px, height_px);
    }

    pub fn set_keys(&mut self, up: bool, down: bool, left: bool, right: bool) {
        self.controls.up = up;
        self.controls.down = down;
        self.controls.left = left;
        self.controls.right = right;
    }

    pub fn set_pointer(&mut self, x_px: f64, y_px: f64) {
        if x_px.is_finite() && y_px.is_finite() {
            self.controls.pointer_px = Some((x_px, y_px));
        }
    }

    /// 서버가 보낸 메시지 하나를 처리합니다. 해석하지 못한 메시지는 세고 버립니다.
    pub fn on_message(&mut self, text: &str, now_ms: f64) -> Result<(), String> {
        // 유한하지 않은 시각이 세션에 들어가면 도착 기록과 재생 시각이 NaN에 머물러 되돌아오지 않습니다.
        if !now_ms.is_finite() {
            return Err(format!("시각 값이 유한하지 않습니다: {now_ms}"));
        }
        let msg = match protocol::decode_server(text) {
            Ok(m) => m,
            Err(e) => {
                self.decode_errors += 1;
                return Err(e.to_string());
            }
        };
        match msg {
            ServerMsg::Welcome {
                protocol: version,
                player_id,
                room,
                balance,
                map,
            } => self.on_welcome(version, player_id, room, &balance, &map),
            ServerMsg::Rejected { reason } => {
                self.status = Status::Rejected;
                self.reject = Some(reason);
                self.session = None;
                Ok(())
            }
            ServerMsg::Roster { players } => {
                if let Some(s) = self.session.as_mut() {
                    s.roster = players.into_iter().map(|p| (p.id, p.name)).collect();
                }
                Ok(())
            }
            ServerMsg::State {
                tick,
                ack,
                me,
                others,
            } => {
                if let Some(s) = self.session.as_mut() {
                    s.on_state(tick, ack, me.as_ref(), others, now_ms);
                }
                Ok(())
            }
            ServerMsg::Pong {
                client_time,
                server_tick,
            } => {
                if let Some(s) = self.session.as_mut() {
                    let rate = s.ticks_per_ms();
                    s.clock.on_pong(client_time, server_tick, now_ms, rate);
                }
                Ok(())
            }
        }
    }

    fn on_welcome(
        &mut self,
        version: u32,
        player_id: PlayerId,
        room: String,
        balance: &str,
        map_text: &str,
    ) -> Result<(), String> {
        if version != PROTOCOL_VERSION {
            self.status = Status::Rejected;
            self.reject = Some(RejectReason::ProtocolMismatch { server: version });
            return Err(format!(
                "프로토콜 버전이 다릅니다 (서버 {version}, 클라이언트 {PROTOCOL_VERSION})"
            ));
        }
        let setup = BalanceConfig::from_toml_str(balance)
            .map_err(|e| e.to_string())
            .and_then(|cfg| {
                Map::parse(map_text, &cfg)
                    .map(|m| (cfg, m))
                    .map_err(|e| e.to_string())
            });
        let (cfg, map) = match setup {
            Ok(v) => v,
            Err(e) => {
                self.status = Status::Error;
                self.error = Some(e.clone());
                return Err(e);
            }
        };
        let params = SimParams::from_config(&cfg);
        let cl = &cfg.client;
        let playback = Playback::new(PlaybackParams {
            arrival_window_ms: cl.interp_arrival_window * 1000.0,
            rate_adjust: cl.interp_rate_adjust,
            starved_slowdown: cl.interp_starved_slowdown,
            snap_ticks: params.seconds_to_ticks(cl.interp_snap),
        });
        let rate = RateEstimator::new(RateParams {
            window_ms: cl.rate_window * 1000.0,
            min_span_ms: cl.rate_min_span * 1000.0,
            max_deviation: cl.rate_max_deviation,
        });
        self.session = Some(Session {
            player_id,
            room,
            tick_ms: params.tick_ms(),
            cfg,
            params,
            map,
            predictor: Predictor::default(),
            remote: Interpolator::default(),
            playback,
            clock: ServerClock::default(),
            roster: BTreeMap::new(),
            last_ping_ms: None,
            accum_ticks: 0.0,
            rate,
            last_state_tick: None,
            last_ack: 0,
            last_input: CharacterInput::default(),
            recent_errors: VecDeque::new(),
            last_server_pos: None,
            server_distance_m: 0.0,
            last_render_ms: None,
            recent_starved: 0.0,
            states_received: 0,
        });
        self.status = Status::Joined;
        self.map_rev += 1;
        Ok(())
    }

    /// 시간을 진행합니다. 로컬 틱마다 입력을 만들어 예측하고, 보낼 메시지를 돌려줍니다.
    pub fn update(&mut self, now_ms: f64) -> Vec<String> {
        // 유한하지 않은 시각은 이 프레임을 건너뜁니다. 보낼 메시지는 다음 정상 호출 때 나갑니다.
        if !now_ms.is_finite() {
            return Vec::new();
        }
        let dt = match self.last_now {
            Some(last) => (now_ms - last).max(0.0),
            None => 0.0,
        };
        self.last_now = Some(now_ms);
        if dt > 0.0 {
            self.frame_ms = Some(match self.frame_ms {
                None => dt,
                Some(f) => f + (dt - f) * 0.05,
            });
        }
        let controls = self.controls;
        let viewport = self.viewport;
        if let Some(s) = self.session.as_mut() {
            s.update(now_ms, dt, controls, viewport, &mut self.outbox);
        }
        std::mem::take(&mut self.outbox)
    }

    pub fn render_state(&mut self) -> RenderState {
        let viewport = self.viewport;
        let fps = self
            .frame_ms
            .map_or(0.0, |f| if f > 0.0 { 1000.0 / f } else { 0.0 });
        let mut rs = RenderState {
            status: self.status.clone(),
            reject: self.reject.clone(),
            error: self.error.clone(),
            map_rev: self.map_rev,
            room: None,
            camera: None,
            characters: Vec::new(),
            debug: DebugInfo {
                fps,
                ..DebugInfo::default()
            },
        };
        if let Some(s) = self.session.as_mut() {
            s.fill_render(&mut rs, viewport, self.last_now.unwrap_or(0.0));
        }
        rs
    }

    pub fn render_json(&mut self) -> String {
        let rs = self.render_state();
        protocol::encode(&rs).unwrap_or_else(|e| {
            // 그리는 쪽이 모든 필드를 읽으므로, 실패해도 같은 모양의 올바른 JSON을 돌려줍니다.
            let fallback = RenderState {
                status: Status::Error,
                reject: None,
                error: Some(format!("그릴 상태를 만들지 못했습니다: {e}")),
                map_rev: self.map_rev,
                room: None,
                camera: None,
                characters: Vec::new(),
                debug: DebugInfo::default(),
            };
            protocol::encode(&fallback).unwrap_or_else(|_| FALLBACK_RENDER_JSON.to_string())
        })
    }

    pub fn map_render(&self) -> Option<MapRender> {
        self.session.as_ref().map(|s| MapRender {
            width: s.map.width(),
            height: s.map.height(),
            rows: s
                .map
                .tiles()
                .chunks(s.map.width().max(1) as usize)
                .map(<[shared::Tile]>::to_vec)
                .collect(),
        })
    }

    pub fn map_json(&self) -> String {
        self.map_render()
            .and_then(|m| protocol::encode(&m).ok())
            .unwrap_or_else(|| "null".into())
    }

    pub fn report(&self) -> ClientReport {
        match &self.session {
            None => ClientReport {
                decode_errors: self.decode_errors,
                ..ClientReport::default()
            },
            Some(s) => ClientReport {
                prediction: s.predictor.stats.clone(),
                interp_frames: s.remote.frames,
                interp_starved_frames: s.remote.starved_frames,
                states_received: s.states_received,
                decode_errors: self.decode_errors,
                rtt_ms: s.clock.rtt_ms(),
                server_distance_m: s.server_distance_m,
            },
        }
    }

    pub fn player_id(&self) -> Option<PlayerId> {
        self.session.as_ref().map(|s| s.player_id)
    }

    pub fn is_joined(&self) -> bool {
        matches!(self.status, Status::Joined)
    }
}

impl Session {
    fn on_state(
        &mut self,
        tick: u64,
        ack: u32,
        me: Option<&shared::Character>,
        others: Vec<shared::OtherCharacter>,
        now_ms: f64,
    ) {
        if self.last_state_tick.is_some_and(|t| t >= tick) {
            return;
        }
        self.last_state_tick = Some(tick);
        self.last_ack = ack;
        self.states_received += 1;
        if let Some(m) = me {
            if let Some(prev) = self.last_server_pos {
                self.server_distance_m += prev.distance(m.pos);
            }
            self.last_server_pos = Some(m.pos);
        }
        let world = World { map: &self.map };
        if let Some(c) = self.predictor.on_server_state(
            ack,
            me,
            &world,
            &self.params,
            self.cfg.client.correction_snap,
        ) {
            self.recent_errors.push_back((now_ms, c.error_m));
        }
        while self
            .recent_errors
            .front()
            .is_some_and(|&(t, _)| now_ms - t > RECENT_ERROR_WINDOW_MS)
        {
            self.recent_errors.pop_front();
        }
        self.remote.push(tick, others);
        self.playback.on_state(tick, now_ms);
        self.rate.push(now_ms, tick);
    }

    fn update(
        &mut self,
        now_ms: f64,
        dt: f64,
        controls: Controls,
        viewport: (f64, f64),
        outbox: &mut Vec<String>,
    ) {
        let ping_ms = self.cfg.client.ping_interval * 1000.0;
        if self.last_ping_ms.is_none_or(|t| now_ms - t >= ping_ms) {
            self.last_ping_ms = Some(now_ms);
            if let Ok(m) = protocol::encode(&ClientMsg::Ping {
                client_time: now_ms,
            }) {
                outbox.push(m);
            }
        }

        // 프레임 사이가 너무 길면(탭이 숨겨졌다 돌아온 경우 등) 그 사이의 시간은 예측에 쓰지 않고 버립니다.
        if self.predictor.is_active() && dt <= self.cfg.client.max_frame_gap * 1000.0 {
            // 명목 틱레이트가 아니라 서버 틱이 이 시계로 실제로 가는 속도로 진행합니다.
            self.accum_ticks += dt * self.ticks_per_ms();
            let max_ticks = self.cfg.client.max_catchup_ticks;
            let due = libm::floor(self.accum_ticks) as u32;
            let mut ticks = 0;
            while self.accum_ticks >= 1.0 {
                if ticks >= max_ticks {
                    self.accum_ticks = 0.0;
                    break;
                }
                // 프레임이 밀려 여러 틱을 한꺼번에 돌릴 때는 키가 언제 바뀌었는지 모르므로 마지막 틱에 바뀐 것으로 봅니다.
                // 서버도 늦은 칸을 직전 입력으로 채우므로, 이렇게 해야 밀린 동안의 예측이 서버와 같습니다.
                let last_of_batch = ticks + 1 >= due.min(max_ticks);
                let input = if last_of_batch {
                    self.input_from(controls, viewport)
                } else {
                    self.last_input
                };
                self.last_input = input;
                let world = World { map: &self.map };
                let max_pending = self.cfg.client.max_pending_inputs as usize;
                if let Some(seq) =
                    self.predictor
                        .local_tick(input, &world, &self.params, max_pending)
                    && let Ok(m) = protocol::encode(&ClientMsg::Input { seq, input })
                {
                    outbox.push(m);
                }
                self.accum_ticks -= 1.0;
                ticks += 1;
            }
        } else {
            self.accum_ticks = 0.0;
        }

        self.predictor
            .decay(dt, self.cfg.client.correction_smoothing * 1000.0);
        let delay_ticks = self.params.seconds_to_ticks(self.cfg.client.interp_delay);
        let newest = self.remote.newest_tick();
        self.playback
            .advance(now_ms, dt, self.ticks_per_ms(), delay_ticks, newest);
    }

    fn ticks_per_ms(&self) -> f64 {
        self.rate.ticks_per_ms(1.0 / self.tick_ms)
    }

    fn camera(&self, viewport: (f64, f64), center: Vec2) -> Option<Camera> {
        let cam = &self.cfg.camera;
        Camera::compute(
            viewport.0,
            viewport.1,
            center,
            cam.view_height,
            cam.max_aspect_w / cam.max_aspect_h,
        )
    }

    fn input_from(&self, c: Controls, viewport: (f64, f64)) -> CharacterInput {
        let move_x = i8::from(c.right) - i8::from(c.left);
        let move_y = i8::from(c.down) - i8::from(c.up);
        let alpha = self.accum_ticks;
        let aim = match (self.predictor.render_pose(alpha), c.pointer_px) {
            (Some((pos, facing)), Some((px, py))) => match self.camera(viewport, pos) {
                Some(cam) => {
                    let d = cam.screen_to_world(px, py) - pos;
                    if d == Vec2::ZERO {
                        facing
                    } else {
                        libm::atan2(d.y, d.x)
                    }
                }
                None => facing,
            },
            (Some((_, facing)), None) => facing,
            (None, _) => 0.0,
        };
        CharacterInput {
            move_x,
            move_y,
            aim,
        }
    }

    fn fill_render(&mut self, rs: &mut RenderState, viewport: (f64, f64), now_ms: f64) {
        let alpha = self.accum_ticks;
        let me = self.predictor.render_pose(alpha);
        let center = me.map_or(
            Vec2::new(
                f64::from(self.map.width()) * 0.5,
                f64::from(self.map.height()) * 0.5,
            ),
            |(p, _)| p,
        );
        let camera = self.camera(viewport, center);
        let radius = self.params.radius;
        let name_of = |id: PlayerId, roster: &BTreeMap<PlayerId, String>| {
            roster.get(&id).cloned().unwrap_or_default()
        };
        let label = |p: Vec2| match camera {
            Some(c) => c.world_to_screen(Vec2::new(p.x, p.y - radius)),
            None => (0.0, 0.0),
        };
        let starved_before = self.remote.starved_frames;
        let others = match self.playback.tick() {
            Some(t) => self.remote.sample(t),
            None => Vec::new(),
        };
        let starved = if self.remote.starved_frames > starved_before {
            1.0
        } else {
            0.0
        };
        let dt = self.last_render_ms.map_or(0.0, |t| (now_ms - t).max(0.0));
        self.last_render_ms = Some(now_ms);
        self.recent_starved +=
            (starved - self.recent_starved) * (1.0 - libm::exp(-dt / RECENT_STARVED_MS));
        for o in others {
            let (lx, ly) = label(o.pos);
            rs.characters.push(RenderCharacter {
                id: o.id,
                name: name_of(o.id, &self.roster),
                is_self: false,
                x: o.pos.x,
                y: o.pos.y,
                facing: o.facing,
                radius,
                label_x: lx,
                label_y: ly,
            });
        }
        if let Some((pos, facing)) = me {
            let (lx, ly) = label(pos);
            rs.characters.push(RenderCharacter {
                id: self.player_id,
                name: name_of(self.player_id, &self.roster),
                is_self: true,
                x: pos.x,
                y: pos.y,
                facing,
                radius,
                label_x: lx,
                label_y: ly,
            });
        }
        rs.room = Some(self.room.clone());
        rs.camera = camera;
        let server_tick = self.clock.server_tick(now_ms, self.ticks_per_ms());
        let stats = &self.predictor.stats;
        rs.debug.rtt_ms = self.clock.rtt_ms();
        rs.debug.server_tick = server_tick;
        rs.debug.render_delay_ms = match (server_tick, self.playback.tick()) {
            (Some(s), Some(p)) => Some((s - p) / self.ticks_per_ms()),
            _ => None,
        };
        rs.debug.pending_inputs = self.predictor.pending_len();
        rs.debug.seq = self.predictor.last_seq();
        rs.debug.ack = self.last_ack;
        rs.debug.last_error_cm = stats.last_error_m * 100.0;
        rs.debug.recent_max_error_cm = self
            .recent_errors
            .iter()
            .map(|&(_, e)| e)
            .fold(0.0, f64::max)
            * 100.0;
        rs.debug.visual_offset_cm = self.predictor.visual_offset().length() * 100.0;
        rs.debug.snaps = stats.snaps;
        rs.debug.recent_starved_pct = self.recent_starved * 100.0;
        rs.debug.interp_margin_ms = match (self.remote.newest_tick(), self.playback.tick()) {
            (Some(n), Some(p)) => Some((n as f64 - p) / self.ticks_per_ms()),
            _ => None,
        };
        rs.debug.arrival_spread_ms =
            self.playback.arrival_spread_ticks(self.ticks_per_ms()) / self.ticks_per_ms();
        rs.debug.server_tick_rate = self.ticks_per_ms() * 1000.0;
        rs.debug.interp_starved_pct = if self.remote.frames == 0 {
            0.0
        } else {
            self.remote.starved_frames as f64 * 100.0 / self.remote.frames as f64
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_time_never_reaches_the_session() {
        let balance = include_str!("../../../config/balance.toml");
        let map = include_str!("../../../maps/test.txt");
        let welcome = protocol::encode(&ServerMsg::Welcome {
            protocol: PROTOCOL_VERSION,
            player_id: PlayerId(1),
            room: "T".into(),
            balance: balance.into(),
            map: map.into(),
        })
        .unwrap();
        let mut c = Client::new();
        c.on_message(&welcome, 0.0).unwrap();
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(c.update(bad).is_empty());
            assert!(c.on_message(&welcome, bad).is_err());
        }
        // 정상 시각으로 돌아오면 핑이 나가고, 그 핑의 시각은 유한합니다.
        let out = c.update(10.0);
        assert!(
            out.iter()
                .any(|m| m.contains(r#""type":"ping","client_time":10.0"#)),
            "{out:?}"
        );
    }

    #[test]
    fn fallback_render_json_has_every_field_of_render_state() {
        let normal: serde_json::Value =
            serde_json::from_str(&protocol::encode(&Client::new().render_state()).unwrap())
                .unwrap();
        let fallback: serde_json::Value = serde_json::from_str(FALLBACK_RENDER_JSON).unwrap();
        let keys = |v: &serde_json::Value| -> Vec<String> {
            v.as_object().unwrap().keys().cloned().collect()
        };
        assert_eq!(keys(&normal), keys(&fallback));
        assert_eq!(keys(&normal["debug"]), keys(&fallback["debug"]));
    }
}
