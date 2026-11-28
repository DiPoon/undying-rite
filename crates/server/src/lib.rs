//! 게임 서버: 방 코드 입장, 방마다 고정 틱 루프, WebSocket, 정적 파일 제공.
//! 화면 없는 테스트 클라이언트(`tools`)가 같은 서버를 프로세스 안에서 띄울 수 있게 라이브러리로 둡니다.

pub mod conn;
pub mod input_queue;

static QUIET: AtomicBool = AtomicBool::new(false);

/// 방과 연결의 진행 로그를 끕니다. 시험 도구가 결과 표를 읽기 쉽게 하려고 씁니다.
pub fn set_quiet(quiet: bool) {
    QUIET.store(quiet, Ordering::Relaxed);
}

pub fn is_quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

/// 진행 로그 (stderr). `set_quiet(true)`이면 찍지 않습니다.
#[macro_export]
macro_rules! log {
    ($($t:tt)*) => {
        if !$crate::is_quiet() {
            eprintln!($($t)*);
        }
    };
}
pub mod lag;
pub mod room;
pub mod stats;

use axum::Router;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::serve::ListenerExt;
use lag::LagSpec;
use room::{RoomMsg, RoomReport, Rooms};
use shared::config::NetSection;
use shared::{BalanceConfig, Map};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tower_http::services::ServeDir;

/// 방 하나가 쓰는 설정과 맵. 클라이언트에 원문을 그대로 보냅니다.
pub struct Setup {
    pub cfg: BalanceConfig,
    pub balance_text: String,
    pub map: Map,
    pub map_text: String,
}

impl Setup {
    pub fn load(
        config_path: &std::path::Path,
        map_path: &std::path::Path,
    ) -> Result<Setup, String> {
        let balance_text = std::fs::read_to_string(config_path)
            .map_err(|e| format!("{}: {e}", config_path.display()))?;
        let cfg = BalanceConfig::from_toml_str(&balance_text)
            .map_err(|e| format!("{}: {e}", config_path.display()))?;
        let map_text = std::fs::read_to_string(map_path)
            .map_err(|e| format!("{}: {e}", map_path.display()))?;
        let map =
            Map::parse(&map_text, &cfg).map_err(|e| format!("{}: {e}", map_path.display()))?;
        Ok(Setup {
            cfg,
            balance_text,
            map,
            map_text,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub bind: SocketAddr,
    /// 빌드된 브라우저 껍데기(web/dist). 없으면 WebSocket만 엽니다.
    pub static_dir: Option<PathBuf>,
    pub config_path: PathBuf,
    pub map_path: PathBuf,
    /// 주소에 지연을 지정하지 않은 연결에 넣을 지연
    pub default_lag: LagSpec,
}

struct Inner {
    rooms: Rooms,
    opts: ServerOptions,
    /// 연결 제한값은 서버를 띄울 때의 설정을 씁니다.
    net: NetSection,
    next_id: AtomicU64,
}

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

impl AppState {
    pub fn rooms(&self) -> Rooms {
        self.0.rooms.clone()
    }

    pub fn net(&self) -> &NetSection {
        &self.0.net
    }

    /// 방을 만들 때 설정과 맵을 다시 읽습니다. 연결의 메시지 수 한도는 서버를 띄울 때의 값이므로,
    /// 새 설정의 정상 송신량이 그 한도 안에 드는지도 확인합니다.
    pub fn load_setup(&self) -> Result<Setup, String> {
        let setup = Setup::load(&self.0.opts.config_path, &self.0.opts.map_path)?;
        setup
            .cfg
            .check_message_limit(self.0.net.max_messages_per_second)
            .map_err(|e| format!("{e}. 연결 한도는 서버를 다시 띄워야 바뀝니다"))?;
        Ok(setup)
    }

    pub fn next_id(&self) -> u64 {
        self.0.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

pub struct ServerHandle {
    pub addr: SocketAddr,
    state: AppState,
}

impl ServerHandle {
    /// 방의 틱 통계와 플레이어별 입력 대기열 통계. 방이 없으면 `None`.
    pub async fn room_report(&self, code: &str) -> Option<RoomReport> {
        let handle = {
            let rooms = self.state.rooms();
            let rooms = rooms.lock().unwrap_or_else(|e| e.into_inner());
            rooms.get(code)?.clone()
        };
        let (tx, rx) = oneshot::channel();
        handle.tx.send(RoomMsg::Report { reply: tx }).await.ok()?;
        rx.await.ok()
    }

    pub fn room_count(&self) -> usize {
        let rooms = self.state.rooms();
        let rooms = rooms.lock().unwrap_or_else(|e| e.into_inner());
        rooms.len()
    }
}

/// 서버를 띄웁니다. 설정과 맵이 올바르지 않으면 띄우지 않고 오류를 돌려줍니다.
pub async fn start(opts: ServerOptions) -> Result<ServerHandle, String> {
    let setup = Setup::load(&opts.config_path, &opts.map_path)?;
    let listener = tokio::net::TcpListener::bind(opts.bind)
        .await
        .map_err(|e| format!("{}에 열지 못했습니다: {e}", opts.bind))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let state = AppState(Arc::new(Inner {
        rooms: Arc::new(Mutex::new(HashMap::new())),
        net: setup.cfg.net.clone(),
        opts: opts.clone(),
        next_id: AtomicU64::new(1),
    }));
    let mut app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state.clone());
    if let Some(dir) = &opts.static_dir {
        app = app.fallback_service(ServeDir::new(dir));
    }
    // 작은 메시지를 30Hz로 보내므로 Nagle 알고리즘이 메시지를 묶어 늦추지 않게 합니다.
    let listener = listener.tap_io(|tcp| {
        if let Err(e) = tcp.set_nodelay(true) {
            log!("TCP_NODELAY를 켜지 못했습니다: {e}");
        }
    });
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("서버가 멈췄습니다: {e}");
        }
    });
    Ok(ServerHandle { addr, state })
}

/// `/ws?lag_ms=150&jitter_ms=30`처럼 연결마다 지연을 지정할 수 있습니다 (왕복 기준).
async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(q): Query<HashMap<String, String>>,
    State(app): State<AppState>,
) -> Response {
    let parse = |k: &str| q.get(k).and_then(|v| v.parse::<u32>().ok());
    let default = app.0.opts.default_lag;
    let lag = LagSpec::new(
        parse("lag_ms").unwrap_or(default.rtt_ms),
        parse("jitter_ms").unwrap_or(default.jitter_ms),
    );
    let limit = app.net().max_message_bytes as usize;
    let conn_id = app.next_id();
    ws.max_message_size(limit)
        .max_frame_size(limit)
        .on_upgrade(move |socket| conn::handle(socket, app, lag, conn_id))
}
