use server::lag::LagSpec;
use server::{ServerOptions, start};
use std::net::SocketAddr;
use std::path::PathBuf;

const USAGE: &str = "\
사용법: server [옵션]  (저장소 루트에서 실행)
  --bind 주소:포트       기본 127.0.0.1:8080
  --static 디렉터리      빌드된 브라우저 껍데기. 기본 web/dist (없으면 WebSocket만 엽니다)
  --config 파일          기본 config/balance.toml
  --map 파일             기본 maps/test.txt
  --lag-ms 밀리초        주소에 지정하지 않은 연결에 넣을 왕복 지연. 기본 0
  --jitter-ms 밀리초     왕복 지연의 흔들림 폭(±). 기본 0
연결마다 지연을 바꾸려면 브라우저 주소에 ?lag=150&jitter=30 을 붙입니다. 붙이지 않은 연결에는 --lag-ms, --jitter-ms가 적용됩니다.
설정 파일은 방을 새로 만들 때마다 다시 읽습니다. 단, [net]의 연결 제한값(max_message_bytes, max_messages_per_second,
message_burst, send_queue_max, write_timeout, join_timeout)은 서버를 띄울 때만 읽으므로 바꾸려면 다시 띄워야 합니다.";

fn parse_args() -> Result<ServerOptions, String> {
    let mut opts = ServerOptions {
        bind: SocketAddr::from(([127, 0, 0, 1], 8080)),
        static_dir: Some(PathBuf::from("web/dist")),
        config_path: PathBuf::from("config/balance.toml"),
        map_path: PathBuf::from("maps/test.txt"),
        default_lag: LagSpec::default(),
    };
    let mut args = std::env::args().skip(1);
    let (mut lag, mut jitter) = (0u32, 0u32);
    while let Some(flag) = args.next() {
        if flag == "--help" || flag == "-h" {
            return Err(String::new());
        }
        let value = args
            .next()
            .ok_or_else(|| format!("{flag}에 값이 없습니다"))?;
        let num = |v: &str| {
            v.parse::<u32>()
                .map_err(|_| format!("{flag}의 값 {v}가 숫자가 아닙니다"))
        };
        match flag.as_str() {
            "--bind" => {
                opts.bind = value
                    .parse()
                    .map_err(|_| format!("주소 {value}를 읽지 못했습니다"))?
            }
            "--static" => opts.static_dir = Some(PathBuf::from(value)),
            "--config" => opts.config_path = PathBuf::from(value),
            "--map" => opts.map_path = PathBuf::from(value),
            "--lag-ms" => lag = num(&value)?,
            "--jitter-ms" => jitter = num(&value)?,
            _ => return Err(format!("모르는 옵션 {flag}")),
        }
    }
    opts.default_lag = LagSpec::new(lag, jitter);
    if opts.static_dir.as_ref().is_some_and(|d| !d.is_dir()) {
        eprintln!(
            "참고: {}가 없어서 브라우저 껍데기는 제공하지 않습니다. bash scripts/build-web.sh 로 만들 수 있습니다.",
            opts.static_dir
                .as_ref()
                .map_or_else(String::new, |d| d.display().to_string())
        );
        opts.static_dir = None;
    }
    Ok(opts)
}

#[tokio::main]
async fn main() {
    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("{e}\n");
            }
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    let lag = opts.default_lag;
    match start(opts).await {
        Ok(handle) => {
            println!("서버 시작: http://{}/", handle.addr);
            if !lag.is_zero() {
                println!("기본 지연: 왕복 {}ms ± {}ms", lag.rtt_ms, lag.jitter_ms);
            }
            let _ = tokio::signal::ctrl_c().await;
            println!("종료합니다");
        }
        Err(e) => {
            eprintln!("서버를 띄우지 못했습니다: {e}");
            std::process::exit(1);
        }
    }
}
