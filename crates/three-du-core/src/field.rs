//! Parses the text records of the field configuration section (`GRID`, `BORD`, `VTMJ`…).

use serde::{Deserialize, Serialize};

/// A reference line of the field, in source units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldLine {
    pub position: f64,
    /// True for major lines (`VTMJ`/`HZMJ`), false for minor grid lines (`VTMN`/`HZMN`).
    pub major: bool,
}

/// Steps per distance along each axis (`GRID 8 5 8 5 …` = 8 steps per 5 units).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Grid {
    pub steps_x: f64,
    pub units_x: f64,
    pub steps_y: f64,
    pub units_y: f64,
}

impl Grid {
    pub fn steps_per_unit_x(&self) -> f64 {
        self.steps_x / self.units_x
    }
    pub fn steps_per_unit_y(&self) -> f64 {
        self.steps_y / self.units_y
    }
}

/// Rectangle (x1, y1, x2, y2) in source units, normalized so x1 ≤ x2 and y1 ≤ y2.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SourceField {
    /// Raw `UNIT` arguments. `0 0` was observed with yard-based fields.
    pub unit_code: Option<String>,
    pub grid: Option<Grid>,
    /// Playing-surface border (`BORD`): goal line to goal line, sideline to sideline.
    pub border: Option<Bounds>,
    /// Lines of constant x (yard lines), sorted.
    pub x_lines: Vec<FieldLine>,
    /// Lines of constant y (sidelines, grid lines), sorted.
    pub y_lines: Vec<FieldLine>,
    /// Hash marks (`HZHS`), lines of constant y.
    pub hashes: Vec<f64>,
    /// Caption records such as `LEFT Side_1`, `TOP Back`.
    pub captions: Vec<(String, String)>,
    /// Records this parser does not interpret, preserved verbatim.
    pub other_records: Vec<String>,
}

fn num(s: Option<&str>) -> Option<f64> {
    s.and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite())
}

const CAPTION_KEYS: &[&str] = &[
    "LEFT", "RIGHT", "TOP", "BOTTOM", "INSIDE", "OUTSIDE", "INFRONT", "BEHIND", "STEPS", "HYDLN", "VTYDLN", "SDLINE", "HASH",
];

impl SourceField {
    pub fn parse(records: &[String]) -> SourceField {
        let mut f = SourceField::default();
        for rec in records {
            let mut parts = rec.split_whitespace();
            let key = parts.next().unwrap_or("");
            let args: Vec<&str> = parts.collect();
            let a = |i: usize| num(args.get(i).copied());
            let handled = match key {
                "UNIT" => {
                    f.unit_code = Some(args.join(" "));
                    true
                }
                "GRID" => match (a(0), a(1), a(2), a(3)) {
                    (Some(sx), Some(ux), Some(sy), Some(uy)) if sx > 0.0 && ux > 0.0 && sy > 0.0 && uy > 0.0 => {
                        f.grid = Some(Grid { steps_x: sx, units_x: ux, steps_y: sy, units_y: uy });
                        true
                    }
                    _ => false,
                },
                "BORD" => match (a(0), a(1), a(2), a(3)) {
                    (Some(x1), Some(y1), Some(x2), Some(y2)) => {
                        f.border = Some(Bounds { x1: x1.min(x2), y1: y1.min(y2), x2: x1.max(x2), y2: y1.max(y2) });
                        true
                    }
                    _ => false,
                },
                "VTMJ" | "VTMN" => match a(0) {
                    Some(p) => {
                        f.x_lines.push(FieldLine { position: p, major: key == "VTMJ" });
                        true
                    }
                    None => false,
                },
                "HZMJ" | "HZMN" => match a(0) {
                    Some(p) => {
                        f.y_lines.push(FieldLine { position: p, major: key == "HZMJ" });
                        true
                    }
                    None => false,
                },
                "HZHS" => match a(0) {
                    Some(p) => {
                        f.hashes.push(p);
                        true
                    }
                    None => false,
                },
                k if CAPTION_KEYS.contains(&k) => {
                    f.captions.push((k.to_string(), args.join(" ").replace('_', " ")));
                    true
                }
                _ => false,
            };
            if !handled {
                f.other_records.push(rec.clone());
            }
        }
        let by_pos = |a: &FieldLine, b: &FieldLine| a.position.total_cmp(&b.position);
        f.x_lines.sort_by(by_pos);
        f.x_lines.dedup_by(|a, b| a.position == b.position);
        f.y_lines.sort_by(by_pos);
        f.y_lines.dedup_by(|a, b| a.position == b.position);
        f.hashes.sort_by(f64::total_cmp);
        f.hashes.dedup();
        f
    }

    pub fn caption(&self, key: &str) -> Option<&str> {
        self.captions.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recs(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_synthetic_field() {
        let f = SourceField::parse(&recs(&[
            "UNIT 0 0",
            "GRID 8 5 8 5 1 1 247 187 50",
            "BORD 50.0 26.25 -50.0 -26.25 255 255 255",
            "VTMJ 0.0 1 `",
            "VTMJ -5.0 1 `",
            "VTMN -2.5",
            "HZMJ -26.25 0 `",
            "HZHS 8.75 `",
            "LEFT Side_1",
            "MARK -2.0 -1.0 2.0 0 51 51",
        ]));
        let g = f.grid.unwrap();
        assert_eq!(g.steps_per_unit_x(), 1.6);
        assert_eq!(f.border.unwrap(), Bounds { x1: -50.0, y1: -26.25, x2: 50.0, y2: 26.25 });
        assert_eq!(f.x_lines.iter().map(|l| l.position).collect::<Vec<_>>(), vec![-5.0, -2.5, 0.0]);
        assert!(f.x_lines[0].major && !f.x_lines[1].major);
        assert_eq!(f.hashes, vec![8.75]);
        assert_eq!(f.caption("LEFT"), Some("Side 1"));
        assert_eq!(f.other_records, vec!["MARK -2.0 -1.0 2.0 0 51 51".to_string()]);
    }

    #[test]
    fn rejects_degenerate_grid() {
        let f = SourceField::parse(&recs(&["GRID 8 0 8 5"]));
        assert!(f.grid.is_none());
        assert_eq!(f.other_records.len(), 1);
    }
}
