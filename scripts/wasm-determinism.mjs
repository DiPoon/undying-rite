// WebAssembly 빌드(web/src/wasm)로 정해진 입력열을 돌려 결과 한 줄을 출력합니다.
// 네이티브 `tools determinism`과 같은 줄이 나와야 합니다.
//   node scripts/wasm-determinism.mjs <seed> <ticks>
import { readFileSync } from 'node:fs';

const root = new URL('..', import.meta.url);
const glue = await import(new URL('web/src/wasm/client_wasm.js', root).href);
glue.initSync({ module: readFileSync(new URL('web/src/wasm/client_wasm_bg.wasm', root)) });

const [seed, ticks] = process.argv.slice(2).map(Number);
const balance = readFileSync(new URL('config/balance.toml', root), 'utf8');
const map = readFileSync(new URL('maps/test.txt', root), 'utf8');
console.log(glue.determinism_line(balance, map, seed, ticks));
