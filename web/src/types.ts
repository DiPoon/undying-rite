// client-core가 render_json / map_json으로 돌려주는 값의 모양 (crates/client-core/src/render.rs).
// 서버와 주고받는 메시지 타입은 여기에 두지 않습니다. 메시지는 해석하지 않고 Rust에 그대로 넘깁니다.

export interface Camera {
  view_x: number;
  view_y: number;
  view_w: number;
  view_h: number;
  scale: number;
  origin_x: number;
  origin_y: number;
}

export interface RenderCharacter {
  id: number;
  name: string;
  is_self: boolean;
  x: number;
  y: number;
  facing: number;
  radius: number;
  label_x: number;
  label_y: number;
}

export interface DebugInfo {
  rtt_ms: number | null;
  server_tick: number | null;
  render_delay_ms: number | null;
  pending_inputs: number;
  seq: number;
  ack: number;
  last_error_cm: number;
  recent_max_error_cm: number;
  visual_offset_cm: number;
  snaps: number;
  interp_starved_pct: number;
  recent_starved_pct: number;
  arrival_spread_ms: number;
  interp_margin_ms: number | null;
  server_tick_rate: number;
  fps: number;
}

export interface RenderState {
  status: 'connecting' | 'joined' | 'rejected' | 'error';
  reject: { code: string } | null;
  error: string | null;
  map_rev: number;
  room: string | null;
  camera: Camera | null;
  characters: RenderCharacter[];
  debug: DebugInfo;
}

export interface MapRender {
  width: number;
  height: number;
  /** rows[y][x]가 타일 (x, y)의 종류 이름 */
  rows: string[][];
}
