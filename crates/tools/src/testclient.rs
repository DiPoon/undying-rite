//! 화면 없는 테스트 클라이언트. 브라우저와 같은 `client-core`를 네이티브로 돌려 예측 오차를 잽니다.

use client_core::{Client, ClientReport};
use futures_util::{SinkExt, StreamExt};
use shared::protocol::{self, ClientMsg, ServerMsg};
use shared::rng::Rng;
use shared::sim::CharacterInput;
use std::time::Duration;
use tokio::time::{Instant, MissedTickBehavior, interval};
use tokio_tungstenite::tungstenite::Message;

/// 브라우저의 requestAnimationFrame 간격 (60Hz)
const FRAME: Duration = Duration::from_micros(16_667);

#[derive(Debug, Clone, Copy)]
pub enum Pattern {
    /// 0.3~1.5초마다 무작위 방향(멈춤 포함)으로 바꿉니다.
    RandomWalk { seed: u64 },
    /// 오른쪽과 왼쪽을 `half_period_ms`마다 번갈아 누릅니다. 벽에 닿지 않는 거리 비교에 씁니다.
    Shuttle { half_period_ms: u64 },
}

struct Driver {
    pattern: Pattern,
    rng: Rng,
    next_change_ms: f64,
    keys: (bool, bool, bool, bool),
}

impl Driver {
    fn new(pattern: Pattern) -> Self {
        let seed = match pattern {
            Pattern::RandomWalk { seed } => seed,
            Pattern::Shuttle { .. } => 0,
        };
        Driver {
            pattern,
            rng: Rng::new(seed, 3),
            next_change_ms: 0.0,
            keys: (false, false, false, false),
        }
    }

    /// (위, 아래, 왼쪽, 오른쪽)
    fn keys(&mut self, now_ms: f64) -> (bool, bool, bool, bool) {
        match self.pattern {
            Pattern::RandomWalk { .. } => {
                if now_ms >= self.next_change_ms {
                    let dx = self.rng.below(3) as i32 - 1;
                    let dy = self.rng.below(3) as i32 - 1;
                    self.keys = (dy < 0, dy > 0, dx < 0, dx > 0);
                    self.next_change_ms = now_ms + 300.0 + f64::from(self.rng.below(1200));
                }
                self.keys
            }
            Pattern::Shuttle { half_period_ms } => {
                let right = (now_ms as u64 / half_period_ms.max(1)).is_multiple_of(2);
                (false, false, !right, right)
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub name: String,
    pub report: ClientReport,
    /// 시간이 다 되기 전에 연결이 끊겼으면 그 이유
    pub disconnected: Option<String>,
    pub errors: Vec<String>,
    pub joined: bool,
    /// 이어진 두 프레임 사이의 가장 긴 간격. 60Hz의 16.7ms보다 크게 길면 이 프로세스가 CPU를 제때 받지 못한 것입니다.
    pub max_frame_gap_ms: f64,
}

/// 서버에 접속해 `secs`초 동안 움직이고 결과를 돌려줍니다.
/// `clock_scale`은 이 클라이언트 시계의 빠르기입니다. 1.025이면 서버보다 2.5% 빠른 시계를 흉내 냅니다.
pub async fn run_client(
    url: String,
    room: String,
    name: String,
    secs: f64,
    pattern: Pattern,
    clock_scale: f64,
) -> Outcome {
    let mut out = Outcome {
        name: name.clone(),
        ..Outcome::default()
    };
    let ws = match tokio_tungstenite::connect_async(url.as_str()).await {
        Ok((ws, _)) => ws,
        Err(e) => {
            out.disconnected = Some(format!("접속 실패: {e}"));
            return out;
        }
    };
    let (mut sink, mut stream) = ws.split();
    let mut client = Client::new();
    client.set_viewport(1280.0, 720.0);
    let join = match client.join_message(&room, &name) {
        Ok(j) => j,
        Err(e) => {
            out.disconnected = Some(format!("입장 메시지를 만들지 못함: {e:?}"));
            return out;
        }
    };
    if let Err(e) = sink.send(Message::Text(join.into())).await {
        out.disconnected = Some(format!("보내기 실패: {e}"));
        return out;
    }
    let start = Instant::now();
    let now_ms = || start.elapsed().as_secs_f64() * 1000.0 * clock_scale;
    let mut driver = Driver::new(pattern);
    let mut frames = interval(FRAME);
    frames.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let deadline = start + Duration::from_secs_f64(secs);
    let mut last_frame: Option<Instant> = None;
    loop {
        tokio::select! {
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    if let Err(e) = client.on_message(t.as_str(), now_ms())
                        && out.errors.len() < 10
                    {
                        out.errors.push(e);
                    }
                }
                Some(Ok(Message::Close(_))) | None => {
                    out.disconnected = Some("서버가 연결을 닫음".into());
                    break;
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    out.disconnected = Some(format!("수신 오류: {e}"));
                    break;
                }
            },
            _ = frames.tick() => {
                let frame_at = Instant::now();
                if frame_at >= deadline {
                    break;
                }
                if let Some(prev) = last_frame {
                    out.max_frame_gap_ms = out.max_frame_gap_ms.max((frame_at - prev).as_secs_f64() * 1000.0);
                }
                last_frame = Some(frame_at);
                let t = now_ms();
                let (u, d, l, r) = driver.keys(t);
                client.set_keys(u, d, l, r);
                for m in client.update(t) {
                    if let Err(e) = sink.send(Message::Text(m.into())).await {
                        out.disconnected = Some(format!("보내기 실패: {e}"));
                        break;
                    }
                }
                // 브라우저처럼 매 프레임 그릴 상태를 만들어서 보간 경로까지 돌립니다.
                let _ = client.render_state();
            }
        }
        if out.disconnected.is_some() {
            break;
        }
    }
    out.joined = client.is_joined();
    out.report = client.report();
    let _ = sink.close().await;
    out
}

/// 입력을 정상의 두 배 속도(60Hz)로 보내는 클라이언트. 예측 없이 서버가 확정한 위치의 이동 거리만 잽니다.
pub async fn run_fast_sender(
    url: String,
    room: String,
    name: String,
    secs: f64,
    half_period_ms: u64,
) -> (f64, Option<String>) {
    let ws = match tokio_tungstenite::connect_async(url.as_str()).await {
        Ok((ws, _)) => ws,
        Err(e) => return (0.0, Some(format!("접속 실패: {e}"))),
    };
    let (mut sink, mut stream) = ws.split();
    let join = ClientMsg::Join {
        protocol: protocol::PROTOCOL_VERSION,
        room,
        name,
    };
    let Ok(join) = protocol::encode(&join) else {
        return (0.0, Some("입장 메시지를 만들지 못함".into()));
    };
    if sink.send(Message::Text(join.into())).await.is_err() {
        return (0.0, Some("보내기 실패".into()));
    }
    let start = Instant::now();
    let deadline = start + Duration::from_secs_f64(secs);
    let mut sends = interval(FRAME);
    sends.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut seq: u32 = 0;
    let mut spawned = false;
    let mut last_pos: Option<shared::Vec2> = None;
    let mut distance = 0.0;
    loop {
        tokio::select! {
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    if let Ok(ServerMsg::State { me: Some(me), .. }) = protocol::decode_server(t.as_str()) {
                        spawned = true;
                        if let Some(p) = last_pos {
                            distance += p.distance(me.pos);
                        }
                        last_pos = Some(me.pos);
                    }
                }
                Some(Ok(Message::Close(_))) | None => return (distance, Some("서버가 연결을 닫음".into())),
                Some(Ok(_)) => {}
                Some(Err(e)) => return (distance, Some(format!("수신 오류: {e}"))),
            },
            _ = sends.tick() => {
                if Instant::now() >= deadline {
                    break;
                }
                if !spawned {
                    continue;
                }
                let t = start.elapsed().as_millis() as u64;
                let right = (t / half_period_ms.max(1)).is_multiple_of(2);
                seq += 1;
                let msg = ClientMsg::Input {
                    seq,
                    input: CharacterInput { move_x: if right { 1 } else { -1 }, move_y: 0, aim: 0.0 },
                };
                if let Ok(text) = protocol::encode(&msg)
                    && sink.send(Message::Text(text.into())).await.is_err()
                {
                    return (distance, Some("보내기 실패".into()));
                }
            }
        }
    }
    let _ = sink.close().await;
    (distance, None)
}
