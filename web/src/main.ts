// 브라우저 껍데기: WebSocket 연결, 입력 장치 상태 전달, Rust가 준 상태 그리기.
// 게임 규칙과 계산은 하지 않습니다. 받은 메시지는 해석하지 않고 그대로 Rust에 넘깁니다.

import init, { WasmClient } from './wasm/client_wasm.js';
import { Input } from './input';
import { Renderer } from './render';
import { S } from './strings';
import type { DebugInfo, MapRender, RenderState } from './types';

function el<T extends HTMLElement>(id: string): T {
  const e = document.getElementById(id);
  if (!e) throw new Error(`#${id} 요소가 없습니다`);
  return e as T;
}

const params = new URLSearchParams(location.search);

function applyStrings(): void {
  document.title = S.pageTitle;
  el('title').textContent = S.title;
  el('room-label').textContent = S.roomLabel;
  el('name-label').textContent = S.nameLabel;
  el('join-button').textContent = S.joinButton;
  el('help').textContent = S.help;
  el<HTMLInputElement>('room').value = params.get('room') ?? '';
  el<HTMLInputElement>('name').value = params.get('name') ?? '';
}

function showBanner(text: string | null): void {
  const b = el('banner');
  b.textContent = text ?? '';
  b.hidden = text === null;
}

function fmt(v: number | null, digits: number, unit: string): string {
  return v === null ? S.debug.none : `${v.toFixed(digits)} ${unit}`;
}

function debugText(room: string | null, d: DebugInfo, lagNote: string | null): string {
  const D = S.debug;
  const lines = [
    `${D.room}: ${room ?? D.none}`,
    `${D.rtt}: ${fmt(d.rtt_ms, 0, D.ms)}`,
    `${D.renderDelay}: ${fmt(d.render_delay_ms, 0, D.ms)}`,
    `${D.pending}: ${d.pending_inputs} (seq ${d.seq} / ack ${d.ack})`,
    `${D.lastError}: ${fmt(d.last_error_cm, 2, D.cm)}`,
    `${D.recentMaxError}: ${fmt(d.recent_max_error_cm, 2, D.cm)}`,
    `${D.visualOffset}: ${fmt(d.visual_offset_cm, 2, D.cm)}`,
    `${D.snaps}: ${d.snaps}`,
    `${D.starved}: ${d.interp_starved_pct.toFixed(1)}% / ${d.recent_starved_pct.toFixed(1)}%`,
    `${D.spread}: ${fmt(d.arrival_spread_ms, 0, D.ms)}`,
    `${D.margin}: ${fmt(d.interp_margin_ms, 0, D.ms)}`,
    `${D.tickRate}: ${d.server_tick_rate.toFixed(2)}${D.perSecond}`,
    `${D.fps}: ${d.fps.toFixed(0)}`,
  ];
  if (lagNote) lines.unshift(lagNote);
  return lines.join('\n');
}

async function play(room: string, name: string): Promise<void> {
  const client = new WasmClient();
  let joinMsg: string;
  try {
    joinMsg = client.join_message(room, name);
  } catch (e) {
    const code = e instanceof Error ? e.message : String(e);
    el('join-error').textContent = S.joinError[code] ?? code;
    return;
  }
  el('join').hidden = true;
  el('game').hidden = false;
  showBanner(S.connecting);

  const renderer = await Renderer.create(el('stage'), el('labels'));
  const input = new Input(renderer.canvas);
  const lag = params.get('lag') ?? '0';
  const jitter = params.get('jitter') ?? '0';
  const lagNote = lag !== '0' || jitter !== '0' ? S.lagNotice(lag, jitter) : null;
  const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
  const query = new URLSearchParams({ lag_ms: lag, jitter_ms: jitter });
  const ws = new WebSocket(`${scheme}://${location.host}/ws?${query}`);
  let closed = false;
  ws.onopen = () => ws.send(joinMsg);
  ws.onmessage = (e) => {
    if (typeof e.data !== 'string') return;
    const err = client.on_message(e.data, performance.now());
    if (err) console.warn(err);
  };
  ws.onclose = () => {
    closed = true;
  };

  const debugEl = el('debug');
  let mapRev = 0;
  let frameNo = 0;
  const frame = () => {
    const now = performance.now();
    client.set_viewport(renderer.width, renderer.height);
    input.apply(client);
    for (const m of client.update(now)) {
      if (ws.readyState === WebSocket.OPEN) ws.send(m);
    }
    const rs = JSON.parse(client.render_json()) as RenderState;
    if (rs.map_rev !== mapRev) {
      mapRev = rs.map_rev;
      const map = JSON.parse(client.map_json()) as MapRender | null;
      if (map) renderer.setMap(map);
    }
    renderer.draw(rs);
    if (frameNo++ % 6 === 0) debugEl.textContent = debugText(rs.room, rs.debug, lagNote);

    if (rs.status === 'rejected') {
      const code = rs.reject?.code ?? '';
      showBanner(S.reject[code] ?? S.rejectUnknown);
    } else if (rs.status === 'error') {
      showBanner(rs.error);
    } else if (closed) {
      showBanner(S.disconnected);
    } else {
      showBanner(rs.status === 'joined' ? null : S.connecting);
    }
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);
}

async function main(): Promise<void> {
  applyStrings();
  try {
    await init();
  } catch (e) {
    el('join-error').textContent = `${S.loadFailed} ${String(e)}`;
    return;
  }
  el<HTMLFormElement>('join-form').addEventListener('submit', (e) => {
    e.preventDefault();
    void play(el<HTMLInputElement>('room').value, el<HTMLInputElement>('name').value);
  });
  // ?room=ABC&name=가&join=1 이면 입장 화면을 건너뜁니다.
  if (params.get('join') === '1') el<HTMLFormElement>('join-form').requestSubmit();
}

void main();
