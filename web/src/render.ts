// PixiJS로 그리기만 합니다. 위치, 방향, 크기, 화면 변환은 모두 Rust가 계산한 값을 그대로 씁니다.

import { Application, Container, Graphics, GraphicsContext } from 'pixi.js';
import type { MapRender, RenderState } from './types';

const TILE_COLORS: Record<string, number> = {
  floor: 0x2b2f36,
  high_wall: 0x8a8f98,
  low_wall: 0x5f7a9a,
  railing: 0x7a6a50,
  window: 0x4fa3c7,
  barrier_spot: 0xb08040,
  objective: 0x3c8f5a,
  restraint_point: 0x8f3c3c,
  survivor_spawn: 0x3c5f8f,
  killer_start: 0x6f3c8f,
};
const UNKNOWN_TILE = 0xff00ff;
const BACKGROUND = 0x000000;

// 반지름 1인 원과 앞쪽 표시. 실제 크기와 방향은 Rust가 준 radius, facing으로 정해집니다.
function body(color: number): GraphicsContext {
  return new GraphicsContext()
    .circle(0, 0, 1)
    .fill(color)
    .poly([0.25, -0.5, 1.3, 0, 0.25, 0.5])
    .fill(0x111111);
}

export class Renderer {
  private readonly world = new Container();
  private readonly mapLayer = new Graphics();
  private readonly charLayer = new Container();
  private readonly viewMask = new Graphics();
  private readonly selfBody = body(0xf2c14e);
  private readonly otherBody = body(0xe8e8e8);
  private readonly sprites = new Map<number, Graphics>();
  private readonly labels = new Map<number, HTMLDivElement>();

  private constructor(
    private readonly app: Application,
    private readonly labelRoot: HTMLElement,
  ) {
    this.world.addChild(this.mapLayer, this.charLayer);
    this.app.stage.addChild(this.world, this.viewMask);
    this.world.mask = this.viewMask;
  }

  static async create(parent: HTMLElement, labelRoot: HTMLElement): Promise<Renderer> {
    const app = new Application();
    await app.init({
      resizeTo: parent,
      background: BACKGROUND,
      antialias: true,
      autoDensity: true,
      resolution: window.devicePixelRatio || 1,
    });
    parent.appendChild(app.canvas);
    return new Renderer(app, labelRoot);
  }

  get canvas(): HTMLCanvasElement {
    return this.app.canvas;
  }

  get width(): number {
    return this.app.screen.width;
  }

  get height(): number {
    return this.app.screen.height;
  }

  setMap(map: MapRender): void {
    this.mapLayer.clear();
    map.rows.forEach((row, y) => {
      row.forEach((kind, x) => {
        this.mapLayer.rect(x, y, 1, 1).fill(TILE_COLORS[kind] ?? UNKNOWN_TILE);
      });
    });
  }

  draw(rs: RenderState): void {
    const cam = rs.camera;
    this.world.visible = cam !== null;
    if (cam) {
      this.world.position.set(cam.origin_x, cam.origin_y);
      this.world.scale.set(cam.scale);
      this.viewMask.clear().rect(cam.view_x, cam.view_y, cam.view_w, cam.view_h).fill(0xffffff);
    }

    const seen = new Set<number>();
    for (const c of rs.characters) {
      seen.add(c.id);
      let g = this.sprites.get(c.id);
      if (!g) {
        g = new Graphics(c.is_self ? this.selfBody : this.otherBody);
        this.sprites.set(c.id, g);
        this.charLayer.addChild(g);
      }
      g.position.set(c.x, c.y);
      g.rotation = c.facing;
      g.scale.set(c.radius);
      // 자기 캐릭터를 맨 위에 그립니다.
      g.zIndex = c.is_self ? 1 : 0;

      let label = this.labels.get(c.id);
      if (!label) {
        label = document.createElement('div');
        label.className = c.is_self ? 'label self' : 'label';
        this.labelRoot.appendChild(label);
        this.labels.set(c.id, label);
      }
      if (label.textContent !== c.name) label.textContent = c.name;
      label.style.transform = `translate(${c.label_x}px, ${c.label_y}px) translate(-50%, -100%)`;
    }
    this.charLayer.sortableChildren = true;
    for (const [id, g] of this.sprites) {
      if (!seen.has(id)) {
        g.destroy();
        this.sprites.delete(id);
        this.labels.get(id)?.remove();
        this.labels.delete(id);
      }
    }
  }
}
