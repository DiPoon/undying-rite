// 키보드와 포인터의 상태만 모읍니다. 이동 방향이나 조준 각도는 Rust(client-core)가 계산합니다.

import type { WasmClient } from './wasm/client_wasm.js';

const UP = new Set(['KeyW', 'ArrowUp']);
const DOWN = new Set(['KeyS', 'ArrowDown']);
const LEFT = new Set(['KeyA', 'ArrowLeft']);
const RIGHT = new Set(['KeyD', 'ArrowRight']);

export class Input {
  private held = new Set<string>();
  private pointer: { x: number; y: number } | null = null;

  constructor(target: HTMLElement) {
    window.addEventListener('keydown', (e) => {
      if (isGameKey(e.code)) {
        this.held.add(e.code);
        e.preventDefault();
      }
    });
    window.addEventListener('keyup', (e) => {
      this.held.delete(e.code);
    });
    // 창이 포커스를 잃으면 keyup을 받지 못하므로 모두 뗀 것으로 칩니다.
    window.addEventListener('blur', () => this.held.clear());
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) this.held.clear();
    });
    target.addEventListener('pointermove', (e) => {
      this.pointer = { x: e.offsetX, y: e.offsetY };
    });
  }

  apply(client: WasmClient): void {
    client.set_keys(this.any(UP), this.any(DOWN), this.any(LEFT), this.any(RIGHT));
    if (this.pointer) client.set_pointer(this.pointer.x, this.pointer.y);
  }

  private any(keys: Set<string>): boolean {
    for (const k of keys) if (this.held.has(k)) return true;
    return false;
  }
}

function isGameKey(code: string): boolean {
  return UP.has(code) || DOWN.has(code) || LEFT.has(code) || RIGHT.has(code);
}
