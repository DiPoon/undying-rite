//! 맵 텍스트 형식 (design.md 4절). 글자 하나가 1m × 1m 타일 하나입니다.
//! 좌표는 x가 오른쪽, y가 아래쪽이고 타일 (x, y)는 [x, x+1) × [y, y+1) 영역을 차지합니다.

use crate::config::BalanceConfig;
use serde::Serialize;
use std::collections::VecDeque;
use std::fmt;

/// 살인마는 언제나 1명이라 시작 지점도 하나입니다.
const KILLER_START_COUNT: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tile {
    Floor,
    HighWall,
    LowWall,
    Railing,
    Window,
    BarrierSpot,
    Objective,
    RestraintPoint,
    SurvivorSpawn,
    KillerStart,
}

impl Tile {
    pub fn from_char(c: char) -> Option<Tile> {
        Some(match c {
            '.' => Tile::Floor,
            '#' => Tile::HighWall,
            '=' => Tile::LowWall,
            '-' => Tile::Railing,
            'w' => Tile::Window,
            'P' => Tile::BarrierSpot,
            'O' => Tile::Objective,
            'R' => Tile::RestraintPoint,
            'S' => Tile::SurvivorSpawn,
            'K' => Tile::KillerStart,
            _ => return None,
        })
    }

    /// 걸어서 지나갈 수 있는 타일 (창틀과 판자 자리는 따로 다룹니다)
    pub fn is_walkable(self) -> bool {
        matches!(
            self,
            Tile::Floor
                | Tile::Objective
                | Tile::RestraintPoint
                | Tile::SurvivorSpawn
                | Tile::KillerStart
        )
    }

    /// 창틀·판자 자리의 옆을 막는 벽으로 인정하는 타일
    fn is_side_wall(self) -> bool {
        matches!(self, Tile::HighWall | Tile::LowWall | Tile::Railing)
    }
}

/// 창틀이나 판자 자리를 넘는 방향
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossAxis {
    /// 왼쪽과 오른쪽이 바닥이라 x 방향으로 넘습니다
    X,
    /// 위와 아래가 바닥이라 y 방향으로 넘습니다
    Y,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct TilePos {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Crossing {
    pub pos: TilePos,
    pub axis: CrossAxis,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Map {
    width: i32,
    height: i32,
    tiles: Vec<Tile>,
    /// 아래 목록은 모두 행 우선(위에서 아래, 왼쪽에서 오른쪽) 순서입니다.
    pub objectives: Vec<TilePos>,
    pub restraint_points: Vec<TilePos>,
    pub survivor_spawns: Vec<TilePos>,
    pub killer_start: TilePos,
    pub windows: Vec<Crossing>,
    pub barrier_spots: Vec<Crossing>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MapError {
    pub problems: Vec<String>,
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "맵이 올바르지 않습니다:")?;
        for p in &self.problems {
            write!(f, "\n  - {p}")?;
        }
        Ok(())
    }
}

impl std::error::Error for MapError {}

/// 편집기에서 찾기 쉽게 1부터 세는 줄·칸 번호로 적습니다.
fn at(x: i32, y: i32) -> String {
    format!("줄 {} 칸 {}", y + 1, x + 1)
}

impl Map {
    pub fn parse(text: &str, cfg: &BalanceConfig) -> Result<Map, MapError> {
        let mut problems = Vec::new();
        let mut rows: Vec<Vec<Tile>> = Vec::new();
        let lines: Vec<&str> = text.lines().map(|l| l.trim_end_matches('\r')).collect();
        let used = lines
            .iter()
            .rposition(|l| !l.is_empty())
            .map_or(0, |i| i + 1);
        for (y, line) in lines[..used].iter().enumerate() {
            let mut row = Vec::with_capacity(line.len());
            for (x, c) in line.chars().enumerate() {
                match Tile::from_char(c) {
                    Some(t) => row.push(t),
                    None => {
                        problems.push(format!("{}: 모르는 글자 {c:?}", at(x as i32, y as i32)));
                        row.push(Tile::HighWall);
                    }
                }
            }
            rows.push(row);
        }
        if rows.is_empty() {
            return Err(MapError {
                problems: vec!["맵이 비어 있습니다".into()],
            });
        }
        let width = rows[0].len();
        for (y, row) in rows.iter().enumerate() {
            if row.len() != width {
                problems.push(format!(
                    "줄 {}: 폭이 {}칸입니다. 첫 줄과 같은 {}칸이어야 합니다",
                    y + 1,
                    row.len(),
                    width
                ));
            }
        }
        if width < 3 || rows.len() < 3 {
            problems.push("맵은 가로세로 3칸 이상이어야 합니다".into());
        }
        if !problems.is_empty() {
            return Err(MapError { problems });
        }
        if width > 1024 || rows.len() > 1024 {
            return Err(MapError {
                problems: vec!["맵은 가로세로 1024칸 이하여야 합니다".into()],
            });
        }

        let (w, h) = (width as i32, rows.len() as i32);
        let tiles: Vec<Tile> = rows.into_iter().flatten().collect();
        let tile = |x: i32, y: i32| -> Tile {
            if x < 0 || y < 0 || x >= w || y >= h {
                Tile::HighWall
            } else {
                tiles[(y * w + x) as usize]
            }
        };

        // 가장자리
        for y in 0..h {
            for x in 0..w {
                let edge = x == 0 || y == 0 || x == w - 1 || y == h - 1;
                if edge && tile(x, y) != Tile::HighWall {
                    problems.push(format!(
                        "{}: 맵 가장자리는 높은 벽(#)이어야 합니다",
                        at(x, y)
                    ));
                }
            }
        }

        let mut objectives = Vec::new();
        let mut restraint_points = Vec::new();
        let mut survivor_spawns = Vec::new();
        let mut killer_starts = Vec::new();
        let mut windows = Vec::new();
        let mut barrier_spots = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let p = TilePos { x, y };
                match tile(x, y) {
                    Tile::Objective => objectives.push(p),
                    Tile::RestraintPoint => restraint_points.push(p),
                    Tile::SurvivorSpawn => survivor_spawns.push(p),
                    Tile::KillerStart => killer_starts.push(p),
                    t @ (Tile::Window | Tile::BarrierSpot) => {
                        let (l, r) = (tile(x - 1, y), tile(x + 1, y));
                        let (u, d) = (tile(x, y - 1), tile(x, y + 1));
                        let axis = if l.is_walkable()
                            && r.is_walkable()
                            && u.is_side_wall()
                            && d.is_side_wall()
                        {
                            Some(CrossAxis::X)
                        } else if u.is_walkable()
                            && d.is_walkable()
                            && l.is_side_wall()
                            && r.is_side_wall()
                        {
                            Some(CrossAxis::Y)
                        } else {
                            None
                        };
                        let name = if t == Tile::Window {
                            "창틀(w)"
                        } else {
                            "판자 자리(P)"
                        };
                        match axis {
                            Some(axis) => {
                                let c = Crossing { pos: p, axis };
                                if t == Tile::Window {
                                    windows.push(c);
                                } else {
                                    barrier_spots.push(c);
                                }
                            }
                            None => problems.push(format!(
                                "{}: {name}은 마주 보는 두 이웃이 바닥이고 나머지 두 이웃이 벽(#, =, -)이어야 합니다",
                                at(x, y)
                            )),
                        }
                    }
                    _ => {}
                }
            }
        }

        let mut count = |what: &str, ch: char, found: usize, want: usize| {
            if found != want {
                problems.push(format!(
                    "{what}({ch})가 {found}개입니다. {want}개여야 합니다"
                ));
            }
        };
        count(
            "목표 지점",
            'O',
            objectives.len(),
            cfg.objective.point_count as usize,
        );
        count(
            "구속 지점",
            'R',
            restraint_points.len(),
            cfg.restraint.point_count as usize,
        );
        count(
            "생존자 시작·리스폰 지점",
            'S',
            survivor_spawns.len(),
            cfg.respawn.point_count as usize,
        );
        count(
            "살인마 시작 지점",
            'K',
            killer_starts.len(),
            KILLER_START_COUNT,
        );

        // 연결성: 바닥에서 출발해 창틀과 판자 자리를 지나는 경로까지 포함해 모든 바닥에 닿는가
        let passable = |t: Tile| t.is_walkable() || matches!(t, Tile::Window | Tile::BarrierSpot);
        if let Some(start) = (0..w * h).find(|&i| tiles[i as usize].is_walkable()) {
            let mut seen = vec![false; tiles.len()];
            let mut queue = VecDeque::from([start]);
            seen[start as usize] = true;
            while let Some(i) = queue.pop_front() {
                let (x, y) = (i % w, i / w);
                for (nx, ny) in [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)] {
                    if passable(tile(nx, ny)) {
                        let j = (ny * w + nx) as usize;
                        if !seen[j] {
                            seen[j] = true;
                            queue.push_back(j as i32);
                        }
                    }
                }
            }
            let cut: Vec<i32> = (0..w * h)
                .filter(|&i| tiles[i as usize].is_walkable() && !seen[i as usize])
                .collect();
            if let Some(&first) = cut.first() {
                problems.push(format!(
                    "{}: 다른 바닥과 이어져 있지 않습니다 (이어지지 않은 바닥 {}칸)",
                    at(first % w, first / w),
                    cut.len()
                ));
            }
        } else {
            problems.push("바닥이 한 칸도 없습니다".into());
        }

        if !problems.is_empty() {
            return Err(MapError { problems });
        }
        let killer_start = killer_starts[0];
        Ok(Map {
            width: w,
            height: h,
            tiles,
            objectives,
            restraint_points,
            survivor_spawns,
            killer_start,
            windows,
            barrier_spots,
        })
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    /// 맵 밖은 높은 벽으로 봅니다.
    pub fn tile(&self, x: i32, y: i32) -> Tile {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            Tile::HighWall
        } else {
            self.tiles[(y * self.width + x) as usize]
        }
    }

    /// 행 우선 순서의 타일 전체 (그리기용)
    pub fn tiles(&self) -> &[Tile] {
        &self.tiles
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_counts(o: u32, r: u32, s: u32) -> BalanceConfig {
        let mut cfg =
            BalanceConfig::from_toml_str(include_str!("../../../config/balance.toml")).unwrap();
        cfg.objective.point_count = o;
        cfg.restraint.point_count = r;
        cfg.respawn.point_count = s;
        cfg
    }

    fn problems(text: &str, cfg: &BalanceConfig) -> Vec<String> {
        Map::parse(text, cfg).unwrap_err().problems
    }

    const ALL_TILES: &str = "\
#########
#S.O.R.K#
#.......#
###w#####
#.......#
#-.=P=.-#
#..#.#..#
#.......#
#########
";

    #[test]
    fn every_tile_letter_is_recognised() {
        let cfg = cfg_with_counts(1, 1, 1);
        let map = Map::parse(ALL_TILES, &cfg).unwrap();
        assert_eq!((map.width(), map.height()), (9, 9));
        assert_eq!(map.tile(1, 1), Tile::SurvivorSpawn);
        assert_eq!(map.tile(3, 1), Tile::Objective);
        assert_eq!(map.tile(5, 1), Tile::RestraintPoint);
        assert_eq!(map.tile(7, 1), Tile::KillerStart);
        assert_eq!(map.tile(2, 1), Tile::Floor);
        assert_eq!(map.tile(0, 0), Tile::HighWall);
        assert_eq!(map.tile(1, 5), Tile::Railing);
        assert_eq!(map.tile(3, 5), Tile::LowWall);
        assert_eq!(
            map.windows,
            vec![Crossing {
                pos: TilePos { x: 3, y: 3 },
                axis: CrossAxis::Y
            }]
        );
        assert_eq!(
            map.barrier_spots,
            vec![Crossing {
                pos: TilePos { x: 4, y: 5 },
                axis: CrossAxis::Y
            }]
        );
        assert_eq!(map.killer_start, TilePos { x: 7, y: 1 });
        assert_eq!(map.tile(-1, 3), Tile::HighWall, "맵 밖은 벽");
    }

    #[test]
    fn crlf_and_trailing_blank_lines_are_accepted() {
        let cfg = cfg_with_counts(1, 1, 1);
        let text = ALL_TILES.replace('\n', "\r\n") + "\r\n\r\n";
        assert!(Map::parse(&text, &cfg).is_ok());
    }

    #[test]
    fn border_must_be_high_wall() {
        let cfg = cfg_with_counts(1, 1, 1);
        let text = ALL_TILES.replacen("#########\n#S", "####.####\n#S", 1);
        let p = problems(&text, &cfg);
        assert!(
            p.iter()
                .any(|m| m.starts_with("줄 1 칸 5") && m.contains("가장자리")),
            "{p:?}"
        );
    }

    #[test]
    fn unknown_letter_and_ragged_rows_are_reported() {
        let cfg = cfg_with_counts(1, 1, 1);
        let p = problems(&ALL_TILES.replace("#..#.#..#", "#..#.#.x#"), &cfg);
        assert!(
            p.iter()
                .any(|m| m.starts_with("줄 7 칸 8") && m.contains("'x'")),
            "{p:?}"
        );
        let p = problems(&ALL_TILES.replace("#.......#\n###", "#......#\n###"), &cfg);
        assert!(
            p.iter().any(|m| m.starts_with("줄 3") && m.contains("폭")),
            "{p:?}"
        );
    }

    #[test]
    fn window_and_barrier_need_one_crossing_direction() {
        let cfg = cfg_with_counts(1, 1, 1);
        // 창틀 아래가 바닥이 아니라 벽이 되면 넘는 방향이 정해지지 않습니다.
        let text = ALL_TILES.replace("#.......#\n#-.=P", "#..#....#\n#-.=P");
        let p = problems(&text, &cfg);
        assert!(
            p.iter()
                .any(|m| m.starts_with("줄 4 칸 4") && m.contains("창틀")),
            "{p:?}"
        );
        // 판자 자리 옆에 바닥이 세 개
        let text = ALL_TILES.replace("#-.=P=.-#", "#-..P=.-#");
        let p = problems(&text, &cfg);
        assert!(
            p.iter()
                .any(|m| m.starts_with("줄 6 칸 5") && m.contains("판자")),
            "{p:?}"
        );
    }

    #[test]
    fn point_counts_must_match_config() {
        let cfg = cfg_with_counts(2, 1, 1);
        let p = problems(ALL_TILES, &cfg);
        assert!(
            p.iter()
                .any(|m| m.contains("목표 지점(O)가 1개") && m.contains("2개")),
            "{p:?}"
        );
        let text = ALL_TILES.replace("R.K", "R.."); // 살인마 시작 지점이 없음
        let p = problems(&text, &cfg_with_counts(1, 1, 1));
        assert!(
            p.iter().any(|m| m.contains("살인마 시작 지점(K)가 0개")),
            "{p:?}"
        );
    }

    #[test]
    fn all_floor_must_be_connected() {
        let cfg = cfg_with_counts(1, 1, 1);
        // 아래쪽 방에서 창틀을 막아 위쪽 방을 떼어 냅니다. 판자 자리를 지나는 경로는 연결로 칩니다.
        let text = ALL_TILES.replace("###w#####", "#########");
        let p = problems(&text, &cfg);
        assert!(
            p.iter().any(|m| m.contains("이어져 있지 않습니다")),
            "{p:?}"
        );
    }

    #[test]
    fn repository_test_map_is_valid() {
        let cfg =
            BalanceConfig::from_toml_str(include_str!("../../../config/balance.toml")).unwrap();
        let map = Map::parse(include_str!("../../../maps/test.txt"), &cfg);
        assert!(map.is_ok(), "{}", map.unwrap_err());
    }
}
