//! WebSocket 연결 하나. 받은 메시지를 검증해 방의 채널로 넘기고, 방이 보낸 메시지를 내보냅니다.
//! 지연 주입은 이 층에서 합니다: 받은 메시지는 방에 넘기기 전에, 보낼 메시지는 소켓에 쓰기 전에 늦춥니다.

use crate::AppState;
use crate::lag::{LagSpec, delay_line};
use crate::room::{JoinReply, RoomHandle, RoomMsg, spawn_room};
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use shared::protocol::{self, ClientMsg, PROTOCOL_VERSION, RejectReason, ServerMsg};
use shared::sim::PlayerId;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, timeout};

/// 방이 이 연결로 보내는 메시지를 쌓아 둘 수 있는 개수. 차면 방이 이 플레이어를 내보냅니다.
const OUT_CHANNEL: usize = 256;
/// 지연선을 지나 소켓에 쓰기를 기다리는 메시지가 이만큼 쌓이면 소켓이 못 따라오는 것으로 보고 끊습니다.
const MAX_WRITE_BACKLOG: usize = 1024;
const JOIN_ATTEMPTS: usize = 3;

pub async fn handle(socket: WebSocket, app: AppState, lag: LagSpec, conn_id: u64) {
    let (mut sink, mut stream) = socket.split();
    let net = app.net().clone();
    let max_bytes = net.max_message_bytes as usize;

    // 1. 입장 메시지
    let join_wait = Duration::from_secs_f64(net.join_timeout);
    let first = match timeout(join_wait, next_text(&mut stream)).await {
        Ok(Some(t)) => t,
        _ => return,
    };
    let (room_code, name) = match protocol::decode_client(&first, max_bytes) {
        Ok(ClientMsg::Join {
            protocol: version,
            room,
            name,
        }) => {
            if version != PROTOCOL_VERSION {
                reject(
                    &mut sink,
                    RejectReason::ProtocolMismatch {
                        server: PROTOCOL_VERSION,
                    },
                )
                .await;
                return;
            }
            let Some(room) = protocol::normalize_room_code(&room) else {
                reject(&mut sink, RejectReason::BadRoomCode).await;
                return;
            };
            let Some(name) = protocol::normalize_name(&name) else {
                reject(&mut sink, RejectReason::BadName).await;
                return;
            };
            (room, name)
        }
        _ => {
            reject(&mut sink, RejectReason::NotJoined).await;
            return;
        }
    };

    // 2. 나가는 길: 방 → out_tx → 지연선 → 소켓
    let (out_tx, mut out_rx) = mpsc::channel::<String>(OUT_CHANNEL);
    let (delay_out_in, mut delay_out_rx) = delay_line::<String>(lag, conn_id ^ 0x5eed_0001);
    tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if delay_out_in.send(m).is_err() {
                return;
            }
        }
    });
    let writer = tokio::spawn(async move {
        while let Some(m) = delay_out_rx.recv().await {
            if delay_out_rx.len() > MAX_WRITE_BACKLOG {
                break;
            }
            if sink.send(Message::Text(m.into())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // 3. 방 찾기 또는 만들기
    let (room, player) = match join_room(&app, &room_code, &name, out_tx.clone(), conn_id).await {
        Ok(v) => v,
        Err(reason) => {
            // 소켓은 writer가 가지고 있으므로 거절 메시지도 같은 길로 보냅니다.
            if let Ok(text) = protocol::encode(&ServerMsg::Rejected { reason }) {
                let _ = out_tx.send(text).await;
            }
            drop(out_tx);
            let _ = writer.await;
            return;
        }
    };
    drop(out_tx);
    crate::log!(
        "[연결 {conn_id}] {name} → 방 {room_code} (지연 {}±{}ms)",
        lag.rtt_ms,
        lag.jitter_ms
    );

    // 4. 들어오는 길: 소켓 → 검증 → 지연선 → 방
    let (delay_in_in, mut delay_in_rx) = delay_line::<RoomMsg>(lag, conn_id ^ 0x5eed_0002);
    let room_tx = room.tx.clone();
    let forward = tokio::spawn(async move {
        while let Some(m) = delay_in_rx.recv().await {
            if room_tx.send(m).await.is_err() {
                return;
            }
        }
    });
    let mut window_start = Instant::now();
    let mut window_count: u32 = 0;
    let reason = loop {
        let Some(text) = next_text(&mut stream).await else {
            break "연결 끊김";
        };
        if window_start.elapsed() >= Duration::from_secs(1) {
            window_start = Instant::now();
            window_count = 0;
        }
        window_count += 1;
        if window_count > net.max_messages_per_second {
            break "초당 메시지 수 초과";
        }
        let msg = match protocol::decode_client(&text, max_bytes) {
            Ok(m) => m,
            Err(_) => break "잘못된 메시지",
        };
        let room_msg = match msg {
            ClientMsg::Input { seq, input } => RoomMsg::Input { player, seq, input },
            ClientMsg::Ping { client_time } => RoomMsg::Ping {
                player,
                client_time,
            },
            ClientMsg::Join { .. } => continue,
        };
        if delay_in_in.send(room_msg).is_err() {
            break "방 닫힘";
        }
    };
    crate::log!("[연결 {conn_id}] {name} 종료: {reason}");
    drop(delay_in_in);
    let _ = forward.await;
    let _ = room.tx.send(RoomMsg::Leave { player }).await;
    let _ = writer.await;
}

async fn join_room(
    app: &AppState,
    code: &str,
    name: &str,
    out: mpsc::Sender<String>,
    conn_id: u64,
) -> Result<(RoomHandle, PlayerId), RejectReason> {
    for _ in 0..JOIN_ATTEMPTS {
        let handle = {
            let rooms = app.rooms();
            let mut rooms = rooms.lock().unwrap_or_else(|e| e.into_inner());
            match rooms.get(code) {
                Some(h) => h.clone(),
                None => {
                    // 방을 새로 만들 때마다 설정과 맵을 다시 읽어서, 고친 수치가 다음 방부터 적용되게 합니다.
                    let setup = match app.load_setup() {
                        Ok(s) => Arc::new(s),
                        Err(e) => {
                            crate::log!("[연결 {conn_id}] 방을 만들지 못했습니다: {e}");
                            return Err(RejectReason::ServerError);
                        }
                    };
                    let id = app.next_id();
                    let h = spawn_room(code.to_string(), id, setup, id, app.rooms());
                    rooms.insert(code.to_string(), h.clone());
                    h
                }
            }
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        let sent = handle
            .tx
            .send(RoomMsg::Join {
                name: name.to_string(),
                out: out.clone(),
                reply: reply_tx,
            })
            .await;
        if sent.is_err() {
            continue;
        }
        match reply_rx.await {
            Ok(JoinReply::Accepted(id)) => return Ok((handle, id)),
            Ok(JoinReply::Full) => return Err(RejectReason::RoomFull),
            Ok(JoinReply::Closing) | Err(_) => continue,
        }
    }
    Err(RejectReason::ServerError)
}

async fn reject(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    reason: RejectReason,
) {
    if let Ok(text) = protocol::encode(&ServerMsg::Rejected { reason }) {
        let _ = sink.send(Message::Text(text.into())).await;
    }
    let _ = sink.close().await;
}

/// 다음 글자 메시지. 연결이 끝나면 `None`. 이진 메시지는 받지 않습니다.
async fn next_text(stream: &mut futures_util::stream::SplitStream<WebSocket>) -> Option<String> {
    loop {
        match stream.next().await? {
            Ok(Message::Text(t)) => return Some(t.as_str().to_owned()),
            Ok(Message::Ping(_) | Message::Pong(_)) => continue,
            Ok(Message::Binary(_) | Message::Close(_)) | Err(_) => return None,
        }
    }
}
