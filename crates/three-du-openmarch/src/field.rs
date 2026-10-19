//! Field checkpoints derived from the source field, in OpenMarch's step conventions.

use three_du_core::DrillProject;

#[derive(Debug, Clone, PartialEq)]
pub struct Checkpoint {
    pub name: String,
    pub terse_name: String,
    pub use_as_reference: bool,
    pub field_label: Option<String>,
    pub visible: bool,
    /// x: steps from center (+ = Side 2). y: steps from the front sideline (+ = back).
    pub steps: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldLayout {
    pub step_inches: f64,
    pub x: Vec<Checkpoint>,
    pub y: Vec<Checkpoint>,
    pub use_hashes: bool,
    /// Grid spacing (steps) of the minor lines drawn between checkpoints.
    pub half_line_x: f64,
    pub half_line_y: f64,
    pub side_labels: [String; 2],
}

fn fmt_num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v}")
    }
}

pub fn layout(p: &DrillProject) -> FieldLayout {
    let t = p.coordinates.transform;
    let f = &p.field;
    let front = f.front_sideline_y;
    let mut x = Vec::new();
    for line in f.yard_lines.iter().filter(|l| l.major) {
        let steps = t.to_steps(line.position, front).0;
        match line.yard_number {
            Some(n) => {
                let label = fmt_num(n);
                x.push(Checkpoint {
                    name: format!("{label} yard line"),
                    terse_name: label.clone(),
                    use_as_reference: true,
                    field_label: (n > 0.0 && (n / 10.0).fract() == 0.0).then_some(label),
                    visible: true,
                    steps,
                });
            }
            None => x.push(Checkpoint {
                name: format!("line at {} steps", fmt_num(steps)),
                terse_name: fmt_num(steps),
                use_as_reference: true,
                field_label: None,
                visible: true,
                steps,
            }),
        }
    }
    // End lines beyond the playing surface keep the canvas the size of the source field.
    for (pos, name) in [(f.extent_x_min, "Side 1 end line"), (f.extent_x_max, "Side 2 end line")] {
        let steps = t.to_steps(pos, front).0;
        if !x.iter().any(|c| (c.steps - steps).abs() < 1e-9) {
            x.push(Checkpoint { name: name.into(), terse_name: "EL".into(), use_as_reference: false, field_label: None, visible: true, steps });
        }
    }
    x.sort_by(|a, b| a.steps.total_cmp(&b.steps));

    let depth = t.to_steps(0.0, f.back_sideline_y).1;
    let mut y = vec![Checkpoint {
        name: "front sideline".into(),
        terse_name: "FSL".into(),
        use_as_reference: true,
        field_label: None,
        visible: false,
        steps: 0.0,
    }];
    let mut hashes: Vec<f64> = f.hashes.iter().map(|&h| t.to_steps(0.0, h).1).collect();
    hashes.sort_by(f64::total_cmp);
    let n = hashes.len();
    for (i, steps) in hashes.into_iter().enumerate() {
        let (name, terse) = match (n, i) {
            (2, 0) => ("front hash".to_string(), "FH".to_string()),
            (2, 1) => ("back hash".to_string(), "BH".to_string()),
            _ => (format!("hash {}", i + 1), format!("H{}", i + 1)),
        };
        y.push(Checkpoint { name, terse_name: terse, use_as_reference: true, field_label: None, visible: true, steps });
    }
    y.push(Checkpoint {
        name: "back sideline".into(),
        terse_name: "BSL".into(),
        use_as_reference: true,
        field_label: None,
        visible: false,
        steps: depth,
    });

    // Minor source lines are drawn every 2.5 yards (4 steps at 8-to-5).
    let minor_step = |lines: &[f64]| -> f64 {
        let mut v: Vec<f64> = lines.to_vec();
        v.sort_by(f64::total_cmp);
        v.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 1e-9).fold(f64::INFINITY, f64::min)
    };
    let xs: Vec<f64> = f.source.x_lines.iter().map(|l| l.position * t.steps_per_unit_x).collect();
    let ys: Vec<f64> = f.source.y_lines.iter().map(|l| l.position * t.steps_per_unit_y).collect();
    let hx = minor_step(&xs);
    let hy = minor_step(&ys);

    FieldLayout {
        step_inches: t.step_inches,
        x,
        y,
        use_hashes: !f.hashes.is_empty(),
        half_line_x: if hx.is_finite() { hx } else { 0.0 },
        half_line_y: if hy.is_finite() { hy } else { 0.0 },
        side_labels: f.side_labels.clone(),
    }
}

impl FieldLayout {
    pub fn x_range(&self) -> (f64, f64) {
        let min = self.x.iter().map(|c| c.steps).fold(0.0, f64::min);
        let max = self.x.iter().map(|c| c.steps).fold(0.0, f64::max);
        (min, max)
    }
    pub fn y_range(&self) -> (f64, f64) {
        let min = self.y.iter().map(|c| c.steps).fold(0.0, f64::min);
        let max = self.y.iter().map(|c| c.steps).fold(0.0, f64::max);
        (min, max)
    }
}
