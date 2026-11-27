// 테스트 전용 파일이라 보조 함수에서도 unwrap으로 바로 실패시킵니다 (clippy의 테스트 예외는 #[test] 함수 안에만 적용됨).
#![allow(clippy::unwrap_used)]

use shared::sim::{Character, CharacterInput, PlayerId, PlayerInputs, World};
use shared::{
    BalanceConfig, Map, SimParams, Vec2, create_round, hash_state, scenario, step, step_character,
};

const BALANCE: &str = include_str!("../../../config/balance.toml");
const TEST_MAP: &str = include_str!("../../../maps/test.txt");

/// 안쪽에 벽 한 칸(6, 4)이 있는 빈 방
const ROOM: &str = "\
############
#S........K#
#..........#
#..........#
#.....#....#
#..........#
#..........#
############
";

fn setup() -> (BalanceConfig, Map, SimParams) {
    let mut cfg = BalanceConfig::from_toml_str(BALANCE).unwrap();
    cfg.objective.point_count = 0;
    cfg.restraint.point_count = 0;
    cfg.respawn.point_count = 1;
    let map = Map::parse(ROOM, &cfg).unwrap();
    let params = SimParams::from_config(&cfg);
    (cfg, map, params)
}

fn at(x: f64, y: f64) -> Character {
    Character {
        id: PlayerId(1),
        pos: Vec2::new(x, y),
        vel: Vec2::ZERO,
        facing: 0.0,
    }
}

fn mv(x: i8, y: i8) -> CharacterInput {
    CharacterInput {
        move_x: x,
        move_y: y,
        aim: 0.0,
    }
}

/// 원이 막힌 타일 어느 것과도 겹치지 않는지 (반올림 오차 1e-9까지 허용)
fn assert_no_overlap(map: &Map, ch: &Character, r: f64) {
    let world = World { map };
    for ty in -1..=map.height() {
        for tx in -1..=map.width() {
            if !world.blocks_movement(tx, ty) {
                continue;
            }
            let cx = ch.pos.x.clamp(f64::from(tx), f64::from(tx) + 1.0);
            let cy = ch.pos.y.clamp(f64::from(ty), f64::from(ty) + 1.0);
            let d = ch.pos.distance(Vec2::new(cx, cy));
            assert!(
                d >= r - 1e-9,
                "타일 ({tx}, {ty})와 겹침: 위치 {:?}, 거리 {d}",
                ch.pos
            );
        }
    }
}

#[test]
fn cannot_pass_through_walls() {
    let (_, map, p) = setup();
    let world = World { map: &map };
    // 안쪽 벽(6, 4)을 향해 오른쪽으로
    let mut ch = at(2.5, 4.5);
    for _ in 0..120 {
        step_character(&mut ch, &mv(1, 0), &world, &p);
        assert_no_overlap(&map, &ch, p.radius);
    }
    assert!(
        (ch.pos.x - (6.0 - p.radius)).abs() < 1e-9,
        "벽 앞에 멈춰야 함: {:?}",
        ch.pos
    );
    assert!(
        (ch.pos.y - 4.5).abs() < 1e-12,
        "정면으로 부딪히면 옆으로 밀리지 않음"
    );
    assert!(ch.vel.x.abs() < 1e-12, "벽으로 향하는 속도는 없어짐");

    // 가장자리 벽을 향해 왼쪽 위 대각선으로
    let mut ch = at(3.5, 3.5);
    for _ in 0..120 {
        step_character(&mut ch, &mv(-1, -1), &world, &p);
        assert_no_overlap(&map, &ch, p.radius);
    }
    assert!(
        (ch.pos.x - (1.0 + p.radius)).abs() < 1e-9 && (ch.pos.y - (1.0 + p.radius)).abs() < 1e-9
    );
}

#[test]
fn fast_movement_does_not_tunnel() {
    let (mut cfg, map, _) = setup();
    cfg.survivor.run_speed = 20.0; // 허용 범위의 최댓값
    cfg.common.acceleration = 1000.0;
    let p = SimParams::from_config(&cfg);
    let world = World { map: &map };
    let mut ch = at(4.5, 4.5);
    for _ in 0..30 {
        step_character(&mut ch, &mv(1, 0), &world, &p);
        assert_no_overlap(&map, &ch, p.radius);
    }
    assert!(ch.pos.x < 6.0);
}

#[test]
fn slides_along_wall_when_moving_diagonally() {
    let (_, map, p) = setup();
    let world = World { map: &map };
    let mut ch = at(2.5, 2.0);
    let mut prev_x = ch.pos.x;
    let mut touching = 0;
    for _ in 0..30 {
        step_character(&mut ch, &mv(1, -1), &world, &p);
        assert_no_overlap(&map, &ch, p.radius);
        if (ch.pos.y - (1.0 + p.radius)).abs() < 1e-9 {
            touching += 1;
            assert!(
                ch.pos.x > prev_x,
                "벽에 닿은 채로도 x 방향으로 계속 움직여야 함"
            );
        }
        prev_x = ch.pos.x;
    }
    assert!(touching > 10, "위쪽 벽에 닿아 미끄러지는 구간이 있어야 함");
}

#[test]
fn slides_around_a_corner() {
    let (_, map, p) = setup();
    let world = World { map: &map };
    // 벽(6, 4)의 위쪽 모서리를 반지름보다 얕게 걸치며 오른쪽으로 갑니다.
    let mut ch = at(3.5, 4.0 - 0.2);
    for _ in 0..60 {
        step_character(&mut ch, &mv(1, 0), &world, &p);
        assert_no_overlap(&map, &ch, p.radius);
    }
    assert!(ch.pos.x > 8.0, "모서리를 돌아 지나가야 함: {:?}", ch.pos);
    assert!(ch.pos.y < 4.0 - p.radius + 1e-9, "벽 위로 비켜 지나감");
}

#[test]
fn diagonal_is_not_faster() {
    let (_, map, p) = setup();
    let world = World { map: &map };
    for ticks in [1, 3, 10] {
        let mut straight = at(3.5, 3.5);
        let mut diag = at(3.5, 3.5);
        for _ in 0..ticks {
            step_character(&mut straight, &mv(1, 0), &world, &p);
            step_character(&mut diag, &mv(1, 1), &world, &p);
        }
        let ds = straight.pos.distance(Vec2::new(3.5, 3.5));
        let dd = diag.pos.distance(Vec2::new(3.5, 3.5));
        assert!(dd <= ds + 1e-12, "{ticks}틱: 대각선 {dd} > 직선 {ds}");
        assert!(
            (dd - ds).abs() < 1e-9,
            "{ticks}틱: 대각선 {dd}과 직선 {ds}의 속력이 같아야 함"
        );
    }
    // 가속이 끝난 뒤의 속력은 설정값 그대로
    let mut ch = at(2.5, 2.5);
    for _ in 0..20 {
        step_character(&mut ch, &mv(1, 1), &world, &p);
    }
    assert!((ch.vel.length() - p.run_speed).abs() < 1e-12);
}

#[test]
fn same_inputs_give_same_hash() {
    let a = scenario::run(BALANCE, TEST_MAP, 7, 1500).unwrap();
    let b = scenario::run(BALANCE, TEST_MAP, 7, 1500).unwrap();
    assert_eq!(a, b);
    let c = scenario::run(BALANCE, TEST_MAP, 8, 1500).unwrap();
    assert_ne!(
        a.final_hash, c.final_hash,
        "다른 입력열이면 해시가 달라야 함"
    );
}

#[test]
fn one_different_input_changes_hash() {
    let cfg = BalanceConfig::from_toml_str(BALANCE).unwrap();
    let map = Map::parse(TEST_MAP, &cfg).unwrap();
    let p = SimParams::from_config(&cfg);
    let world = World { map: &map };
    let ids = [PlayerId(1), PlayerId(2)];
    let run = |tweak: bool| {
        let mut s = create_round(&cfg, &map, 1, &ids).unwrap();
        for t in 0..30 {
            // 벽에 닿으면 차이가 지워지므로, 아무도 벽에 닿지 않는 30틱 안에서 한 틱만 멈춥니다.
            let x = if tweak && t == 10 { 0 } else { 1 };
            let frame = vec![
                PlayerInputs {
                    player: PlayerId(1),
                    inputs: vec![mv(x, 0)],
                },
                PlayerInputs {
                    player: PlayerId(2),
                    inputs: vec![mv(0, 1)],
                },
            ];
            step(&mut s, &world, &p, &frame);
        }
        hash_state(&s)
    };
    assert_eq!(run(false), run(false));
    assert_ne!(run(false), run(true));
}
