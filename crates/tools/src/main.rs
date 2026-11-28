//! 개발 도구 (저장소 루트에서 실행)
//!   tools soak [--clients 4] [--lag-ms 150] [--jitter-ms 30] [--secs 60]
//!   tools determinism [--seed 20261127] [--ticks 3000]
//!   tools bots --url ws://127.0.0.1:8080/ws --room ABC [--clients 2] [--secs 600]

mod soak;
mod testclient;

use server::lag::LagSpec;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use testclient::{Pattern, run_client};

const USAGE: &str = "\
사용법 (저장소 루트에서):
  tools soak [--clients 4] [--lag-ms 150] [--jitter-ms 30] [--secs 60]
      서버를 이 프로세스 안에 띄우고 테스트 클라이언트로 M0 자동 확인을 합니다. 실패하면 종료 코드 1.
  tools determinism [--seed 20261127] [--ticks 3000]
      정해진 입력열을 돌려 상태 해시를 출력합니다. WebAssembly 빌드의 결과와 비교하는 데 씁니다.
  tools bots --url ws://127.0.0.1:8080/ws --room 방코드 [--clients 2] [--secs 600] [--lag-ms 0] [--jitter-ms 0]
      이미 떠 있는 서버의 방에 무작위로 움직이는 테스트 클라이언트를 넣습니다. 끝나면 예측 오차를 출력합니다.
공통: [--config config/balance.toml] [--map maps/test.txt]";

struct Args(HashMap<String, String>);

impl Args {
    fn parse(rest: &[String]) -> Result<Args, String> {
        let mut map = HashMap::new();
        let mut it = rest.iter();
        while let Some(k) = it.next() {
            let key = k
                .strip_prefix("--")
                .ok_or_else(|| format!("옵션은 --로 시작해야 합니다: {k}"))?;
            let v = it.next().ok_or_else(|| format!("{k}에 값이 없습니다"))?;
            map.insert(key.to_string(), v.clone());
        }
        Ok(Args(map))
    }

    fn str(&self, k: &str, default: &str) -> String {
        self.0
            .get(k)
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }

    fn num<T: std::str::FromStr>(&self, k: &str, default: T) -> Result<T, String> {
        match self.0.get(k) {
            None => Ok(default),
            Some(v) => v
                .parse()
                .map_err(|_| format!("--{k}의 값 {v}를 읽지 못했습니다")),
        }
    }
}

fn run() -> Result<bool, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = argv.first() else {
        return Err(String::new());
    };
    let args = Args::parse(&argv[1..])?;
    let config_path = PathBuf::from(args.str("config", "config/balance.toml"));
    let map_path = PathBuf::from(args.str("map", "maps/test.txt"));
    match cmd.as_str() {
        "soak" => soak::run(soak::SoakOptions {
            clients: args.num("clients", 4)?,
            lag: LagSpec::new(args.num("lag-ms", 150)?, args.num("jitter-ms", 30)?),
            secs: args.num("secs", 60.0)?,
            config_path,
            map_path,
        }),
        "determinism" => {
            let balance = std::fs::read_to_string(&config_path).map_err(|e| e.to_string())?;
            let map = std::fs::read_to_string(&map_path).map_err(|e| e.to_string())?;
            let seed: u32 = args.num("seed", 20_261_127)?;
            let ticks: u32 = args.num("ticks", 3000)?;
            let r = shared::scenario::run(&balance, &map, u64::from(seed), ticks)?;
            println!("{}", r.to_line());
            Ok(true)
        }
        "bots" => {
            let url = args.0.get("url").cloned().ok_or("--url이 필요합니다")?;
            let room = args.0.get("room").cloned().ok_or("--room이 필요합니다")?;
            let clients: usize = args.num("clients", 2)?;
            let secs: f64 = args.num("secs", 600.0)?;
            // 옵션을 준 것만 붙입니다. 붙이지 않으면 서버의 --lag-ms, --jitter-ms 기본값이 적용됩니다.
            let mut query = Vec::new();
            if args.0.contains_key("lag-ms") {
                query.push(format!("lag_ms={}", args.num::<u32>("lag-ms", 0)?));
            }
            if args.0.contains_key("jitter-ms") {
                query.push(format!("jitter_ms={}", args.num::<u32>("jitter-ms", 0)?));
            }
            let url = if query.is_empty() {
                url
            } else {
                let sep = if url.contains('?') { '&' } else { '?' };
                format!("{url}{sep}{}", query.join("&"))
            };
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            let outcomes = rt.block_on(async {
                let tasks: Vec<_> = (0..clients)
                    .map(|i| {
                        tokio::spawn(run_client(
                            url.clone(),
                            room.clone(),
                            format!("봇{}", i + 1),
                            secs,
                            Pattern::RandomWalk {
                                seed: 100 + i as u64,
                            },
                            1.0,
                        ))
                    })
                    .collect();
                let mut out = Vec::new();
                let mut panics = Vec::new();
                for t in tasks {
                    match t.await {
                        Ok(o) => out.push(o),
                        Err(e) => panics.push(e.to_string()),
                    }
                }
                (out, panics)
            });
            let (outcomes, panics) = outcomes;
            for p in &panics {
                println!("봇 태스크 실패: {p}");
            }
            for o in &outcomes {
                let p = &o.report.prediction;
                println!(
                    "{}: 상태 {}개, 최대 예측 오차 {:.4}cm, 즉시 맞춤 {}, 끊김 {}",
                    o.name,
                    o.report.states_received,
                    p.max_error_m * 100.0,
                    p.snaps,
                    o.disconnected.as_deref().unwrap_or("없음")
                );
            }
            Ok(panics.is_empty()
                && outcomes.len() == clients
                && outcomes.iter().all(|o| o.disconnected.is_none()))
        }
        "--help" | "-h" | "help" => Err(String::new()),
        other => Err(format!("모르는 명령 {other}")),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("{e}\n");
            }
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
