//! M0 자동 확인: 지연 속에서 테스트 클라이언트 여럿을 돌려 예측 오차, 연결 안정성, 서버 틱 분포,
//! 입력을 빨리 보내는 클라이언트의 이동 거리를 확인합니다.

use crate::testclient::{Outcome, Pattern, run_client, run_fast_sender};
use client_core::predict::ONE_CM;
use server::lag::LagSpec;
use server::room::RoomReport;
use server::{ServerHandle, ServerOptions};
use std::path::PathBuf;
use std::time::Duration;

pub struct SoakOptions {
    pub clients: usize,
    pub lag: LagSpec,
    pub secs: f64,
    pub config_path: PathBuf,
    pub map_path: PathBuf,
}

const SOAK_ROOM: &str = "SOAK";
const SPEED_ROOM: &str = "SPEED";
const SKEW_ROOM: &str = "SKEW";
/// 서버와 시계 빠르기가 다른 클라이언트. 이 개발 환경에서 WSL과 Windows의 시계가 약 2.5% 달랐습니다.
const SKEWS: [f64; 2] = [1.025, 0.975];
/// 입력 두 배 클라이언트가 정상 클라이언트보다 더 멀리 갈 수 있는 비율의 상한 (milestones.md M0)
const SPEED_RATIO_LIMIT: f64 = 1.05;
/// 받은 상태 수가 이론값(초 × 틱레이트)의 이 비율보다 적으면 무언가 멈춘 것입니다.
const MIN_STATE_RATIO: f64 = 0.9;
const SHUTTLE_HALF_PERIOD_MS: u64 = 1000;
/// 클라이언트 프레임 간격이나 서버 틱 간격이 이보다 길면 CPU를 제때 받지 못한 것으로 보고 경고합니다 (60Hz 프레임의 약 6배).
const CPU_STALL_WARN_MS: f64 = 100.0;

/// 서버를 이 프로세스 안의 별도 런타임(작업 스레드 2개)에서 띄웁니다. 클라이언트의 부하가 서버 틱 측정에 섞이지 않게 합니다.
pub fn spawn_server(
    opts: ServerOptions,
) -> Result<(ServerHandle, tokio::sync::oneshot::Sender<()>), String> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("server")
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = ready_tx.send(Err(e.to_string()));
                return;
            }
        };
        rt.block_on(async move {
            match server::start(opts).await {
                Ok(h) => {
                    let _ = ready_tx.send(Ok(h));
                    let _ = stop_rx.await;
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        });
    });
    let handle = ready_rx.recv().map_err(|e| e.to_string())??;
    Ok((handle, stop_tx))
}

struct Check {
    name: String,
    pass: bool,
    detail: String,
}

pub fn run(opts: SoakOptions) -> Result<bool, String> {
    let server_opts = ServerOptions {
        bind: ([127, 0, 0, 1], 0).into(),
        static_dir: None,
        config_path: opts.config_path.clone(),
        map_path: opts.map_path.clone(),
        default_lag: LagSpec::default(),
    };
    let tick_rate = server::Setup::load(&opts.config_path, &opts.map_path)?
        .cfg
        .sim
        .tick_rate;
    server::set_quiet(true);
    let (server, stop) = spawn_server(server_opts)?;
    let url = format!(
        "ws://{}/ws?lag_ms={}&jitter_ms={}",
        server.addr, opts.lag.rtt_ms, opts.lag.jitter_ms
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let secs = opts.secs;
    println!(
        "== 화면 없는 테스트 클라이언트 {}개 (방 {SOAK_ROOM}), 왕복 지연 {}ms ± {}ms, {}초",
        opts.clients, opts.lag.rtt_ms, opts.lag.jitter_ms, secs
    );
    println!(
        "   같은 시간에 방 {SPEED_ROOM}에서 정상 클라이언트와 입력을 두 배로 보내는 클라이언트를 비교합니다."
    );

    let result = rt.block_on(async {
        let mut soak = Vec::new();
        for i in 0..opts.clients {
            soak.push(tokio::spawn(run_client(
                url.clone(),
                SOAK_ROOM.into(),
                format!("시험{}", i + 1),
                secs,
                Pattern::RandomWalk { seed: i as u64 + 1 },
                1.0,
            )));
        }
        let mut skewed = Vec::new();
        for (i, &scale) in SKEWS.iter().enumerate() {
            skewed.push(tokio::spawn(run_client(
                url.clone(),
                SKEW_ROOM.into(),
                format!("시계{:+.1}%", (scale - 1.0) * 100.0),
                secs,
                Pattern::RandomWalk {
                    seed: 50 + i as u64,
                },
                scale,
            )));
        }
        let normal = tokio::spawn(run_client(
            url.clone(),
            SPEED_ROOM.into(),
            "정상".into(),
            secs,
            Pattern::Shuttle {
                half_period_ms: SHUTTLE_HALF_PERIOD_MS,
            },
            1.0,
        ));
        let fast = tokio::spawn(run_fast_sender(
            url.clone(),
            SPEED_ROOM.into(),
            "두배".into(),
            secs,
            SHUTTLE_HALF_PERIOD_MS,
        ));
        // 클라이언트가 아직 붙어 있을 때 서버 통계를 받습니다.
        tokio::time::sleep(Duration::from_secs_f64((secs - 1.0).max(0.5))).await;
        let soak_report = server.room_report(SOAK_ROOM).await;
        let speed_report = server.room_report(SPEED_ROOM).await;
        let skew_report = server.room_report(SKEW_ROOM).await;
        let mut outcomes = Vec::new();
        let mut panics = Vec::new();
        for t in soak {
            match t.await {
                Ok(o) => outcomes.push(o),
                Err(e) => panics.push(e.to_string()),
            }
        }
        let mut skew_outcomes = Vec::new();
        for t in skewed {
            match t.await {
                Ok(o) => skew_outcomes.push(o),
                Err(e) => panics.push(e.to_string()),
            }
        }
        let normal = normal.await.map_err(|e| e.to_string());
        let fast = fast.await.map_err(|e| e.to_string());
        (
            outcomes,
            panics,
            normal,
            fast,
            soak_report,
            speed_report,
            skew_outcomes,
            skew_report,
        )
    });
    let _ = stop.send(());
    let (outcomes, panics, normal, fast, soak_report, speed_report, skew_outcomes, skew_report) =
        result;

    print_clients(&outcomes);
    if let Some(r) = &soak_report {
        print_server(r);
    } else {
        println!("서버 통계를 받지 못했습니다 (방 {SOAK_ROOM}이 없음)");
    }

    let mut checks = Vec::new();
    let min_states = (secs * f64::from(tick_rate) * MIN_STATE_RATIO) as u64;
    let broken: Vec<String> = outcomes
        .iter()
        .filter(|o| {
            o.disconnected.is_some()
                || !o.errors.is_empty()
                || !o.joined
                || o.report.decode_errors > 0
                || o.report.states_received < min_states
        })
        .map(|o| {
            format!(
                "{}: 끊김={:?} 오류={:?} 입장={} 상태수={}",
                o.name, o.disconnected, o.errors, o.joined, o.report.states_received
            )
        })
        .chain(panics.iter().map(|p| format!("태스크 패닉: {p}")))
        .collect();
    checks.push(Check {
        name: "연결 끊김과 예외가 없음".into(),
        pass: broken.is_empty() && outcomes.len() == opts.clients,
        detail: if broken.is_empty() {
            format!(
                "{}개 모두 {}초 동안 연결 유지, 상태 {}개 이상 수신",
                outcomes.len(),
                secs,
                min_states
            )
        } else {
            broken.join("; ")
        },
    });
    let max_err = outcomes
        .iter()
        .map(|o| o.report.prediction.max_error_m)
        .fold(0.0, f64::max);
    let samples: u64 = outcomes.iter().map(|o| o.report.prediction.samples).sum();
    checks.push(Check {
        name: "자기 예측 오차 최대 1cm 이하".into(),
        pass: max_err <= ONE_CM && samples > 0,
        detail: format!("최대 {:.4}cm, 표본 {samples}개", max_err * 100.0),
    });

    let speed_detail;
    let speed_pass;
    match (&normal, &fast) {
        (Ok(n), Ok((fast_m, fast_err))) => {
            let normal_m = n.report.server_distance_m;
            let ratio = if normal_m > 0.0 {
                fast_m / normal_m
            } else {
                f64::INFINITY
            };
            speed_pass = n.disconnected.is_none()
                && fast_err.is_none()
                && normal_m > 0.0
                && ratio <= SPEED_RATIO_LIMIT;
            speed_detail = format!(
                "정상 {normal_m:.2}m, 두 배 {fast_m:.2}m, 비율 {ratio:.4} (기준 {SPEED_RATIO_LIMIT} 이하){}{}",
                n.disconnected
                    .as_ref()
                    .map_or(String::new(), |d| format!(", 정상 끊김: {d}")),
                fast_err
                    .as_ref()
                    .map_or(String::new(), |d| format!(", 두 배 끊김: {d}")),
            );
            println!("\n== 입력 두 배 클라이언트 (방 {SPEED_ROOM})");
            println!("   {speed_detail}");
            if let Some(r) = &speed_report {
                for p in &r.players {
                    println!(
                        "   서버 입력 대기열 {}: 버림 {}개, 따라잡기 {}틱, 최대 길이 {}",
                        p.name, p.queue.dropped, p.queue.catchup_ticks, p.queue.max_len
                    );
                }
            }
        }
        _ => {
            speed_pass = false;
            speed_detail = format!("태스크 실패: {normal:?} / {fast:?}");
        }
    }
    checks.push(Check {
        name: "입력을 두 배로 보내도 5% 넘게 멀리 가지 못함".into(),
        pass: speed_pass,
        detail: speed_detail,
    });

    println!("\n== 시계 빠르기가 다른 클라이언트 (방 {SKEW_ROOM})");
    print_clients(&skew_outcomes);
    if let Some(r) = &skew_report {
        print_server(r);
    }
    let skew_err = skew_outcomes
        .iter()
        .map(|o| o.report.prediction.max_error_m)
        .fold(0.0, f64::max);
    let skew_dropped: u64 = skew_report.as_ref().map_or(u64::MAX, |r| {
        r.players.iter().map(|p| p.queue.dropped).sum()
    });
    let skew_broken = skew_outcomes.len() != SKEWS.len()
        || skew_outcomes.iter().any(|o| {
            o.disconnected.is_some() || !o.joined || o.report.states_received < min_states
        });
    checks.push(Check {
        name: "시계가 ±2.5% 다른 클라이언트도 예측 오차 1cm 이하, 넘쳐서 버린 입력 없음".into(),
        pass: !skew_broken && skew_err <= ONE_CM && skew_dropped == 0,
        detail: format!(
            "최대 오차 {:.4}cm, 서버 대기열에서 넘쳐 버린 입력 {}개{}",
            skew_err * 100.0,
            skew_dropped,
            if skew_broken {
                ", 연결 문제 있음"
            } else {
                ""
            }
        ),
    });

    println!("\n== 판정");
    for c in &checks {
        println!(
            "[{}] {} — {}",
            if c.pass { "통과" } else { "실패" },
            c.name,
            c.detail
        );
    }
    // 판정 기준은 바꾸지 않고, 실행 중 CPU를 제때 받지 못했으면 그 사실을 함께 알립니다.
    // 이 기기는 다른 작업이 몰리면 프로세스가 수백 ms씩 멈추고(architecture.md 7절), 그때의 실패는 구조의 결함과 구분해야 합니다.
    let max_gap = outcomes
        .iter()
        .chain(skew_outcomes.iter())
        .map(|o| o.max_frame_gap_ms)
        .fold(0.0, f64::max);
    let max_interval = soak_report.as_ref().map_or(0.0, |r| r.tick_interval.max_ms);
    let skipped = soak_report.as_ref().map_or(0, |r| r.skipped_ticks);
    if max_gap > CPU_STALL_WARN_MS || max_interval > CPU_STALL_WARN_MS || skipped > 0 {
        println!(
            "[경고] 이 실행 동안 프로세스가 CPU를 제때 받지 못했습니다: 클라이언트 최대 프레임 간격 {max_gap:.0}ms, \
             서버 틱 간격 최대 {max_interval:.0}ms, 건너뛴 틱 {skipped}. 다른 무거운 작업이 끝난 뒤 다시 돌려 보세요."
        );
    }
    Ok(checks.iter().all(|c| c.pass))
}

fn print_clients(outcomes: &[Outcome]) {
    println!(
        "\n클라이언트  상태수  예측표본  최대오차cm  평균오차cm  1cm초과  즉시맞춤  최대미확인  보간멈춤%  왕복ms  최대프레임간격ms  오류  끊김"
    );
    let mut hist = [0u64; 7];
    for o in outcomes {
        let p = &o.report.prediction;
        for (h, v) in hist.iter_mut().zip(p.histogram.iter()) {
            *h += v;
        }
        let starved = if o.report.interp_frames == 0 {
            0.0
        } else {
            o.report.interp_starved_frames as f64 * 100.0 / o.report.interp_frames as f64
        };
        println!(
            "{:<10}  {:>6}  {:>8}  {:>10.4}  {:>10.5}  {:>7}  {:>8}  {:>10}  {:>9.2}  {:>6.1}  {:>16.1}  {:>4}  {}",
            o.name,
            o.report.states_received,
            p.samples,
            p.max_error_m * 100.0,
            p.mean_error_m() * 100.0,
            p.over_1cm,
            p.snaps,
            p.max_pending,
            starved,
            o.report.rtt_ms.unwrap_or(f64::NAN),
            o.max_frame_gap_ms,
            o.errors.len() as u64 + o.report.decode_errors,
            o.disconnected.as_deref().unwrap_or("없음"),
        );
    }
    let labels = ["0", "≤0.1mm", "≤1mm", "≤1cm", "≤10cm", "≤1m", ">1m"];
    let parts: Vec<String> = labels
        .iter()
        .zip(hist.iter())
        .map(|(l, n)| format!("{l}: {n}"))
        .collect();
    println!("예측 오차 분포 (전체): {}", parts.join(", "));
}

fn print_server(r: &RoomReport) {
    println!("\n서버 (방 {}, {}틱)", r.code, r.ticks);
    println!("  틱 처리 시간: {}", r.tick_duration.line());
    println!("  틱 간격:      {}", r.tick_interval.line());
    println!(
        "  밀려서 이어 처리한 틱 {}, 건너뛴 틱 {}",
        r.catchup_ticks, r.skipped_ticks
    );
    for p in &r.players {
        let q = &p.queue;
        println!(
            "  입력 대기열 {}: 대신 쓴 틱 {}, 늦어서 버림 {}, 버퍼 늘림 {}/줄임 {} (끝 +{}칸), 다시 채움 {}, 따라잡기 {}틱, 넘쳐서 버림 {}, 최대 길이 {}",
            p.name,
            q.starved_ticks,
            q.late,
            q.grows,
            q.shrinks,
            q.extra_buffer,
            q.rebuffers,
            q.catchup_ticks,
            q.dropped,
            q.max_len
        );
    }
}
