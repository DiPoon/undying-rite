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

/// 방이 닫히는 중이라 새 방을 만들어 다시 들어가는 시도의 횟수
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

    // 2. 나가는 길: 방 → 지연선(유계) → 소켓
    // 소켓이 막히면 지연선이 차고, 방의 try_send가 실패해서 방이 이 플레이어를 내보냅니다.
    let queue_max = net.send_queue_max as usize;
    let (out_tx, mut out_rx) = delay_line::<String>(lag, conn_id ^ 0x5eed_0001, queue_max);
    let write_timeout = Duration::from_secs_f64(net.write_timeout);
    let mut writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            // 상대가 읽지 않아 송신 버퍼가 차면 send가 돌아오지 않으므로 시간 제한을 둡니다.
            match timeout(write_timeout, sink.send(Message::Text(m.into()))).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) | Err(_) => break,
            }
        }
        let _ = timeout(write_timeout, sink.close()).await;
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
    let (delay_in_in, mut delay_in_rx) =
        delay_line::<RoomMsg>(lag, conn_id ^ 0x5eed_0002, queue_max);
    let room_tx = room.tx.clone();
    let forward = tokio::spawn(async move {
        while let Some(m) = delay_in_rx.recv().await {
            if room_tx.send(m).await.is_err() {
                return;
            }
        }
    });
    // 메시지 수 제한은 토큰 버킷입니다. 망이나 서버가 몇 초 멈췄다 풀리면 쌓여 있던 정상 메시지가 한꺼번에
    // 읽히는데, 멈춘 시간만큼 토큰도 다시 차 있으므로 끊기지 않습니다. 오래 보내는 양은 초당 한도를 넘지 못합니다.
    let refill_per_sec = f64::from(net.max_messages_per_second);
    let burst = f64::from(net.message_burst);
    let mut tokens = burst;
    let mut last_refill = Instant::now();
    let mut writer_done = false;
    let reason = loop {
        let text = tokio::select! {
            t = next_text(&mut stream) => t,
            _ = &mut writer, if !writer_done => {
                writer_done = true;
                break "보내기 실패 또는 쓰기 시간 초과";
            }
        };
        let Some(text) = text else {
            break "연결 끊김";
        };
        let now = Instant::now();
        tokens = (tokens + (now - last_refill).as_secs_f64() * refill_per_sec).min(burst);
        last_refill = now;
        if tokens < 1.0 {
            break "초당 메시지 수 초과";
        }
        tokens -= 1.0;
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
        if delay_in_in.send(room_msg).await.is_err() {
            break "방 닫힘";
        }
    };
    crate::log!("[연결 {conn_id}] {name} 종료: {reason}");
    drop(delay_in_in);
    let _ = forward.await;
    let _ = room.tx.send(RoomMsg::Leave { player }).await;
    if !writer_done {
        let _ = writer.await;
    }
}

async fn join_room(
    app: &AppState,
    code: &str,
    name: &str,
    out: mpsc::Sender<String>,
    conn_id: u64,
) -> Result<(RoomHandle, PlayerId), RejectReason> {
    let rooms = app.rooms();
    for _ in 0..JOIN_ATTEMPTS {
        let existing = {
            let rooms = rooms.lock().unwrap_or_else(|e| e.into_inner());
            rooms.get(code).cloned()
        };
        let handle = match existing {
            Some(h) => h,
            None => {
                // 방을 새로 만들 때마다 설정과 맵을 다시 읽어서, 고친 수치가 다음 방부터 적용되게 합니다.
                // 파일 읽기는 /mnt/e에서 수백 ms가 걸릴 수 있어 락 밖, 블로킹 전용 스레드에서 합니다.
                let loader = app.clone();
                let setup = match tokio::task::spawn_blocking(move || loader.load_setup()).await {
                    Ok(Ok(s)) => Arc::new(s),
                    Ok(Err(e)) => {
                        crate::log!("[연결 {conn_id}] 방을 만들지 못했습니다: {e}");
                        return Err(RejectReason::ServerError);
                    }
                    Err(e) => {
                        crate::log!("[연결 {conn_id}] 설정을 읽는 작업이 실패했습니다: {e}");
                        return Err(RejectReason::ServerError);
                    }
                };
                let mut rooms = rooms.lock().unwrap_or_else(|e| e.into_inner());
                // 읽는 사이에 다른 연결이 같은 방을 만들었으면 그 방에 들어갑니다.
                match rooms.get(code) {
                    Some(h) => h.clone(),
                    None => {
                        let id = app.next_id();
                        let h = spawn_room(code.to_string(), id, setup, id, app.rooms());
                        rooms.insert(code.to_string(), h.clone());
                        h
                    }
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
