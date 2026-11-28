//! 방 하나 = 태스크 하나. 라운드 상태를 혼자 소유하고 채널로만 바깥과 통신하므로 락이 없습니다.
//! M0에는 대기실이 없어서, 방이 만들어지면 바로 진행 상태로 틱을 돕니다.

use crate::Setup;
use crate::input_queue::{InputQueue, QueueParams, QueueStats};
use crate::stats::{Distribution, Samples};
use serde::Serialize;
use shared::SimParams;
use shared::protocol::{self, PROTOCOL_VERSION, RosterEntry, ServerMsg};
use shared::sim::{
    CharacterInput, PlayerId, PlayerInputs, RoundState, World, add_character, create_round,
    remove_character, step, step_character, view_for,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until};

pub type Rooms = Arc<Mutex<HashMap<String, RoomHandle>>>;

/// 방이 받는 메시지
pub enum RoomMsg {
    Join {
        name: String,
        out: mpsc::Sender<String>,
        reply: oneshot::Sender<JoinReply>,
    },
    Input {
        player: PlayerId,
        seq: u32,
        input: CharacterInput,
    },
    Ping {
        player: PlayerId,
        client_time: f64,
    },
    Leave {
        player: PlayerId,
    },
    Report {
        reply: oneshot::Sender<RoomReport>,
    },
}

#[derive(Debug)]
pub enum JoinReply {
    Accepted(PlayerId),
    Full,
    /// 방이 닫히는 중이라 새 방을 만들어 다시 들어가야 합니다
    Closing,
}

#[derive(Clone)]
pub struct RoomHandle {
    pub id: u64,
    pub tx: mpsc::Sender<RoomMsg>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlayerReport {
    pub id: PlayerId,
    pub name: String,
    pub queue: QueueStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoomReport {
    pub code: String,
    pub ticks: u64,
    /// 한 틱의 처리 시간 (입력 수집부터 전송까지)
    pub tick_duration: Distribution,
    /// 이어진 두 틱의 시작 시각 사이 간격
    pub tick_interval: Distribution,
    /// 밀려서 바로 이어서 처리한 틱 수
    pub catchup_ticks: u64,
    /// 너무 밀려서 건너뛴 틱 수
    pub skipped_ticks: u64,
    pub players: Vec<PlayerReport>,
}

struct Player {
    name: String,
    out: mpsc::Sender<String>,
    queue: InputQueue,
}

struct Room {
    code: String,
    id: u64,
    setup: Arc<Setup>,
    params: SimParams,
    queue_params: QueueParams,
    state: RoundState,
    players: BTreeMap<PlayerId, Player>,
    next_player: u32,
    had_players: bool,
    rooms: Rooms,
    last_tick_start: Option<Instant>,
    durations: Samples,
    intervals: Samples,
    catchup_ticks: u64,
    skipped_ticks: u64,
}

/// 방 채널의 크기. 연결 태스크가 넣는 쪽이고, 차면 그 연결이 기다립니다.
const ROOM_CHANNEL: usize = 1024;

pub fn spawn_room(code: String, id: u64, setup: Arc<Setup>, seed: u64, rooms: Rooms) -> RoomHandle {
    let (tx, rx) = mpsc::channel(ROOM_CHANNEL);
    let params = SimParams::from_config(&setup.cfg);
    let queue_params = QueueParams::from_config(&setup.cfg);
    let state = match create_round(&setup.cfg, &setup.map, seed, &[]) {
        Ok(s) => s,
        // 플레이어 없이 만드는 라운드는 실패할 수 없습니다.
        Err(_) => RoundState {
            tick: 0,
            rng: shared::rng::Rng::new(seed, 0),
            characters: Vec::new(),
        },
    };
    let room = Room {
        code,
        id,
        setup,
        params,
        queue_params,
        state,
        players: BTreeMap::new(),
        next_player: 1,
        had_players: false,
        rooms,
        last_tick_start: None,
        durations: Samples::default(),
        intervals: Samples::default(),
        catchup_ticks: 0,
        skipped_ticks: 0,
    };
    tokio::spawn(room.run(rx));
    RoomHandle { id, tx }
}

impl Room {
    async fn run(mut self, mut rx: mpsc::Receiver<RoomMsg>) {
        let tick_rate = f64::from(self.params.tick_rate);
        let net = self.setup.cfg.net.clone();
        let offset = |n: u64| Duration::from_secs_f64(n as f64 / tick_rate);
        let created = Instant::now();
        let mut base = created;
        let mut scheduled: u64 = 0;
        let idle_limit = Duration::from_secs_f64(net.join_timeout * 2.0);
        crate::log!("[방 {}] 열림", self.code);
        loop {
            let due = base + offset(scheduled + 1);
            tokio::select! {
                () = sleep_until(due) => {
                    let mut ran = 0;
                    loop {
                        let now = Instant::now();
                        if now < base + offset(scheduled + 1) {
                            break;
                        }
                        if ran >= net.max_catchup_ticks {
                            let behind = ((now - base).as_secs_f64() * tick_rate) as u64;
                            self.skipped_ticks += behind.saturating_sub(scheduled);
                            base = now;
                            scheduled = 0;
                            break;
                        }
                        self.tick();
                        scheduled += 1;
                        ran += 1;
                        if ran > 1 {
                            self.catchup_ticks += 1;
                        }
                    }
                }
                msg = rx.recv() => match msg {
                    Some(m) => self.handle(m),
                    None => return,
                },
            }
            let idle =
                self.players.is_empty() && (self.had_players || created.elapsed() > idle_limit);
            if idle {
                self.close(&mut rx);
                return;
            }
        }
    }

    fn handle(&mut self, msg: RoomMsg) {
        match msg {
            RoomMsg::Join { name, out, reply } => {
                let result = self.join(name, out);
                let _ = reply.send(result);
            }
            RoomMsg::Input { player, seq, input } => {
                if let Some(p) = self.players.get_mut(&player) {
                    p.queue.push(seq, input, &self.queue_params);
                }
            }
            RoomMsg::Ping {
                player,
                client_time,
            } => {
                let since = self.last_tick_start.map_or(0.0, |t| {
                    t.elapsed().as_secs_f64() * f64::from(self.params.tick_rate)
                });
                let msg = ServerMsg::Pong {
                    client_time,
                    server_tick: self.state.tick as f64 + since.min(1.0),
                };
                self.send_to(player, &msg);
            }
            RoomMsg::Leave { player } => self.leave(player, "연결 종료"),
            RoomMsg::Report { reply } => {
                let _ = reply.send(self.report());
            }
        }
    }

    fn join(&mut self, name: String, out: mpsc::Sender<String>) -> JoinReply {
        if self.players.len() >= self.setup.cfg.room_capacity() {
            return JoinReply::Full;
        }
        let id = PlayerId(self.next_player);
        if add_character(&mut self.state, &self.setup.cfg, &self.setup.map, id).is_err() {
            return JoinReply::Full;
        }
        self.next_player += 1;
        let welcome = ServerMsg::Welcome {
            protocol: PROTOCOL_VERSION,
            player_id: id,
            room: self.code.clone(),
            balance: self.setup.balance_text.clone(),
            map: self.setup.map_text.clone(),
        };
        self.players.insert(
            id,
            Player {
                name: name.clone(),
                out,
                queue: InputQueue::default(),
            },
        );
        self.had_players = true;
        self.send_to(id, &welcome);
        crate::log!(
            "[방 {}] {name}({}) 입장, {}명",
            self.code,
            id.0,
            self.players.len()
        );
        self.broadcast_roster();
        JoinReply::Accepted(id)
    }

    fn leave(&mut self, id: PlayerId, why: &str) {
        if let Some(p) = self.players.remove(&id) {
            remove_character(&mut self.state, id);
            crate::log!(
                "[방 {}] {}({}) 퇴장: {why}, {}명",
                self.code,
                p.name,
                id.0,
                self.players.len()
            );
            self.broadcast_roster();
        }
    }

    fn broadcast_roster(&mut self) {
        let msg = ServerMsg::Roster {
            players: self
                .players
                .iter()
                .map(|(&id, p)| RosterEntry {
                    id,
                    name: p.name.clone(),
                })
                .collect(),
        };
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        for id in ids {
            self.send_to(id, &msg);
        }
    }

    /// 보내지 못하면(연결이 닫혔거나 너무 밀림) 그 플레이어를 내보냅니다.
    fn send_to(&mut self, id: PlayerId, msg: &ServerMsg) {
        let Some(p) = self.players.get(&id) else {
            return;
        };
        let ok = match protocol::encode(msg) {
            Ok(text) => p.out.try_send(text).is_ok(),
            Err(e) => {
                crate::log!("[방 {}] 메시지 인코딩 실패: {e}", self.code);
                true
            }
        };
        if !ok {
            self.leave(id, "보내기 실패 (연결 닫힘 또는 송신 대기열 가득 참)");
        }
    }

    fn tick(&mut self) {
        let start = Instant::now();
        if let Some(prev) = self.last_tick_start {
            self.intervals.push((start - prev).as_micros() as u64);
        }
        self.last_tick_start = Some(start);

        let world = World {
            map: &self.setup.map,
        };
        let (state, params, qp) = (&self.state, &self.params, &self.queue_params);
        let frame: Vec<PlayerInputs> = self
            .players
            .iter_mut()
            .map(|(&id, p)| {
                let ch = state.characters.iter().find(|c| c.id == id);
                // 이 입력을 한 번 더 적용해도 캐릭터가 그대로인지. 버퍼를 늘리는 틱을 예측과 어긋나지 않게 넣는 데 씁니다.
                let is_noop = |input: &CharacterInput| {
                    ch.is_some_and(|c| {
                        let mut probe = c.clone();
                        step_character(&mut probe, input, &world, params);
                        probe == *c
                    })
                };
                PlayerInputs {
                    player: id,
                    inputs: p.queue.take_for_tick(qp, is_noop),
                }
            })
            .collect();
        let _events = step(&mut self.state, &world, &self.params, &frame);

        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        for id in ids {
            let view = view_for(&self.state, id);
            let ack = self.players.get(&id).map_or(0, |p| p.queue.ack());
            let msg = ServerMsg::State {
                tick: view.tick,
                ack,
                me: view.me,
                others: view.others,
            };
            self.send_to(id, &msg);
        }
        self.durations.push(start.elapsed().as_micros() as u64);
    }

    fn report(&self) -> RoomReport {
        RoomReport {
            code: self.code.clone(),
            ticks: self.state.tick,
            tick_duration: self.durations.summary(),
            tick_interval: self.intervals.summary(),
            catchup_ticks: self.catchup_ticks,
            skipped_ticks: self.skipped_ticks,
            players: self
                .players
                .iter()
                .map(|(&id, p)| PlayerReport {
                    id,
                    name: p.name.clone(),
                    queue: p.queue.stats.clone(),
                })
                .collect(),
        }
    }

    fn close(&mut self, rx: &mut mpsc::Receiver<RoomMsg>) {
        {
            let mut rooms = self.rooms.lock().unwrap_or_else(|e| e.into_inner());
            if rooms.get(&self.code).is_some_and(|h| h.id == self.id) {
                rooms.remove(&self.code);
            }
        }
        rx.close();
        while let Ok(msg) = rx.try_recv() {
            match msg {
                RoomMsg::Join { reply, .. } => {
                    let _ = reply.send(JoinReply::Closing);
                }
                RoomMsg::Report { reply } => {
                    let _ = reply.send(self.report());
                }
                _ => {}
            }
        }
        crate::log!("[방 {}] 닫힘 ({}틱)", self.code, self.state.tick);
    }
}
