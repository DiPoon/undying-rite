// 브라우저 경로 연기 시험. 떠 있는 서버에 브라우저와 같은 WebAssembly 빌드(web/src/wasm)로 두 클라이언트를 접속시킵니다.
// PixiJS 그리기만 빼고 web/src/main.ts와 같은 순서(set_* → update → render_json)로 부릅니다.
//   node scripts/wasm-smoke.mjs http://127.0.0.1:8080
import { readFileSync } from 'node:fs';

const base = process.argv[2];
if (!base) {
  console.error('사용법: node scripts/wasm-smoke.mjs http://주소:포트');
  process.exit(2);
}
const root = new URL('..', import.meta.url);
const glue = await import(new URL('web/src/wasm/client_wasm.js', root).href);
glue.initSync({ module: readFileSync(new URL('web/src/wasm/client_wasm_bg.wasm', root)) });

const failures = [];
const check = (ok, what) => {
  console.log(`${ok ? '  ok ' : '  실패'} ${what}`);
  if (!ok) failures.push(what);
};

// 1. 서버가 빌드된 브라우저 껍데기를 제공하는가
const index = await fetch(`${base}/`);
const html = await index.text();
check(index.ok && html.includes('id="game"'), `GET / → ${index.status}, index.html 제공`);
const script = html.match(/src="(\/assets\/[^"]+\.js)"/)?.[1];
const asset = script ? await fetch(`${base}${script}`) : null;
check(asset?.ok === true, `GET ${script ?? '(스크립트 없음)'} → ${asset?.status ?? '-'}`);

// 2. 두 클라이언트가 같은 방에 들어가 서로를 보는가
const LAG = { lag_ms: '100', jitter_ms: '20' };
function runClient(name, keys, ms) {
  return new Promise((resolve) => {
    const client = new glue.WasmClient();
    client.set_viewport(1280, 720);
    const errors = [];
    const ws = new WebSocket(`${base.replace(/^http/, 'ws')}/ws?${new URLSearchParams(LAG)}`);
    ws.onopen = () => ws.send(client.join_message('SMOKE', name));
    ws.onmessage = (e) => {
      const err = client.on_message(e.data, performance.now());
      if (err) errors.push(err);
    };
    let last = null;
    let closedEarly = false;
    ws.onclose = () => {
      closedEarly = last === null || !done;
    };
    let done = false;
    const timer = setInterval(() => {
      client.set_keys(...keys);
      for (const m of client.update(performance.now())) if (ws.readyState === WebSocket.OPEN) ws.send(m);
      last = JSON.parse(client.render_json());
    }, 16);
    setTimeout(() => {
      done = true;
      clearInterval(timer);
      ws.close();
      resolve({ name, rs: last, errors, closedEarly });
    }, ms);
  });
}

const [a, b] = await Promise.all([
  runClient('가', [false, false, false, true], 4000),
  runClient('나', [false, true, false, false], 4000),
]);
for (const r of [a, b]) {
  const rs = r.rs;
  const me = rs?.characters.find((c) => c.is_self);
  const other = rs?.characters.find((c) => !c.is_self);
  check(rs?.status === 'joined', `${r.name}: 입장 (상태 ${rs?.status})`);
  check(r.errors.length === 0 && !r.closedEarly, `${r.name}: 메시지 오류 ${r.errors.length}개, 도중 끊김 ${r.closedEarly}`);
  check(other !== undefined && other.name === (r.name === '가' ? '나' : '가'), `${r.name}: 상대(${other?.name ?? '없음'})가 보임`);
  check(me !== undefined && rs.camera !== null, `${r.name}: 자기 캐릭터와 카메라가 있음`);
  const rtt = rs?.debug.rtt_ms ?? 0;
  // 주입은 왕복 100±20ms이고, 처리 지연을 감안해 80~200ms면 주소의 지연이 적용된 것으로 봅니다.
  check(rtt >= 80 && rtt <= 200, `${r.name}: 주소로 준 지연이 적용됨 (왕복 ${rtt.toFixed(0)}ms, 주입 100±20, 판정 80~200)`);
  check((rs?.debug.recent_max_error_cm ?? 1) <= 1, `${r.name}: 최근 최대 예측 오차 ${rs?.debug.recent_max_error_cm.toFixed(4)}cm`);
}
const aMe = a.rs?.characters.find((c) => c.is_self);
const bMe = b.rs?.characters.find((c) => c.is_self);
check(aMe !== undefined && aMe.x > 3, `가: 오른쪽으로 이동함 (x=${aMe?.x.toFixed(2)})`);
check(bMe !== undefined && bMe.y > 3, `나: 아래로 이동함 (y=${bMe?.y.toFixed(2)})`);

if (failures.length > 0) {
  console.log(`실패 ${failures.length}건`);
  process.exit(1);
}
console.log('브라우저 경로 연기 시험 통과');
