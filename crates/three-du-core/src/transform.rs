//! Conversion between raw source coordinates and canonical field coordinates.
//!
//! Canonical coordinates are **marching steps**:
//! * `x` from the center line (the 50), positive toward the audience's right (Side 2);
//! * `y` from the front sideline, positive toward the back sideline.
//!
//! This is the convention of the OpenMarch Universal Standard (`yOrigin = "front"`).
//! Metric, center-origin coordinates (Fieldwork) are derived with [`CoordinateTransform::to_meters_centered`].

use serde::{Deserialize, Serialize};

pub const METERS_PER_INCH: f64 = 0.0254;
pub const INCHES_PER_YARD: f64 = 36.0;

/// An affine, axis-aligned mapping from source units to canonical steps.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CoordinateTransform {
    /// Steps per source unit along x (1.6 for yards at 8-to-5).
    pub steps_per_unit_x: f64,
    /// Steps per source unit along y.
    pub steps_per_unit_y: f64,
    /// Source x of the center line.
    pub center_x: f64,
    /// Source y of the front sideline.
    pub front_y: f64,
    /// +1 when source +x points to the audience's right; -1 when mirrored.
    pub x_sign: f64,
    /// +1 when source +y points toward the back sideline; -1 when it points toward the audience.
    pub y_sign: f64,
    /// Length of one step in inches (22.5 for 8-to-5).
    pub step_inches: f64,
    /// Distance in steps from the front to the back sideline (used for centered output).
    pub field_depth_steps: f64,
}

impl CoordinateTransform {
    pub fn to_steps(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.x_sign * (x - self.center_x) * self.steps_per_unit_x,
            self.y_sign * (y - self.front_y) * self.steps_per_unit_y,
        )
    }

    pub fn from_steps(&self, xs: f64, ys: f64) -> (f64, f64) {
        (
            xs / (self.x_sign * self.steps_per_unit_x) + self.center_x,
            ys / (self.y_sign * self.steps_per_unit_y) + self.front_y,
        )
    }

    pub fn step_meters(&self) -> f64 {
        self.step_inches * METERS_PER_INCH
    }

    /// Meters from field center: +x to the audience's right, +y toward the back sideline.
    pub fn to_meters_centered(&self, x: f64, y: f64) -> (f64, f64) {
        let (xs, ys) = self.to_steps(x, y);
        let m = self.step_meters();
        (xs * m, (ys - self.field_depth_steps / 2.0) * m)
    }

    /// Validates that the transform is finite and invertible.
    pub fn check(&self) -> Result<(), String> {
        let vals = [
            self.steps_per_unit_x,
            self.steps_per_unit_y,
            self.center_x,
            self.front_y,
            self.step_inches,
            self.field_depth_steps,
        ];
        if vals.iter().any(|v| !v.is_finite()) {
            return Err("transform contains a non-finite value".into());
        }
        if self.steps_per_unit_x <= 0.0 || self.steps_per_unit_y <= 0.0 || self.step_inches <= 0.0 {
            return Err("scales must be positive".into());
        }
        if self.x_sign.abs() != 1.0 || self.y_sign.abs() != 1.0 {
            return Err("axis signs must be +1 or -1".into());
        }
        Ok(())
    }
}

/// User overrides for when source metadata is missing or wrong.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TransformOverrides {
    pub steps_per_unit: Option<f64>,
    pub flip_x: bool,
    pub flip_y: bool,
    pub front_y: Option<f64>,
    pub center_x: Option<f64>,
}

impl TransformOverrides {
    pub fn apply(&self, mut t: CoordinateTransform) -> CoordinateTransform {
        if let Some(s) = self.steps_per_unit {
            t.steps_per_unit_x = s;
            t.steps_per_unit_y = s;
        }
        if self.flip_x {
            t.x_sign = -t.x_sign;
        }
        if self.flip_y {
            t.y_sign = -t.y_sign;
        }
        if let Some(v) = self.front_y {
            t.front_y = v;
        }
        if let Some(v) = self.center_x {
            t.center_x = v;
        }
        t
    }

    pub fn is_empty(&self) -> bool {
        *self == TransformOverrides::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The verified 8-to-5 yard field: 100 × 52.5 yd, front sideline at y = -26.25.
    fn pyware_yards() -> CoordinateTransform {
        CoordinateTransform {
            steps_per_unit_x: 1.6,
            steps_per_unit_y: 1.6,
            center_x: 0.0,
            front_y: -26.25,
            x_sign: 1.0,
            y_sign: 1.0,
            step_inches: 22.5,
            field_depth_steps: 84.0,
        }
    }

    fn close(a: (f64, f64), b: (f64, f64)) {
        assert!((a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9, "{a:?} != {b:?}");
    }

    #[test]
    fn center_field() {
        let t = pyware_yards();
        close(t.to_steps(0.0, 0.0), (0.0, 42.0));
        close(t.to_meters_centered(0.0, 0.0), (0.0, 0.0));
    }

    #[test]
    fn sidelines() {
        let t = pyware_yards();
        close(t.to_steps(0.0, -26.25), (0.0, 0.0));
        close(t.to_steps(0.0, 26.25), (0.0, 84.0));
        let back = t.to_meters_centered(0.0, 26.25);
        assert!((back.1 - 42.0 * 0.5715).abs() < 1e-9);
    }

    #[test]
    fn both_goal_lines() {
        let t = pyware_yards();
        close(t.to_steps(-50.0, -26.25), (-80.0, 0.0));
        close(t.to_steps(50.0, -26.25), (80.0, 0.0));
        let (xm, _) = t.to_meters_centered(50.0, 0.0);
        assert!((xm - 45.72).abs() < 1e-9, "50 yd is 45.72 m, got {xm}");
    }

    #[test]
    fn yard_line_and_hash_intersections() {
        let t = pyware_yards();
        // Side-1 35 yard line (15 yd left of center) on the front hash (28 steps from front).
        close(t.to_steps(-15.0, -8.75), (-24.0, 28.0));
        // Side-2 20 yard line on the back hash.
        close(t.to_steps(30.0, 8.75), (48.0, 56.0));
    }

    #[test]
    fn axis_inversions() {
        let base = pyware_yards();
        let flipped = TransformOverrides { flip_x: true, flip_y: true, ..Default::default() }.apply(base);
        let (x, y) = base.to_steps(12.5, 10.0);
        let (fx, fy) = flipped.to_steps(12.5, 10.0);
        assert_eq!(fx, -x);
        assert_eq!(fy, -y);
        // Flipping y measures from the same front line in the other direction.
        close(flipped.to_steps(0.0, -26.25 - 10.0), (0.0, 16.0));
    }

    #[test]
    fn round_trip() {
        for t in [pyware_yards(), TransformOverrides { flip_y: true, steps_per_unit: Some(2.0), ..Default::default() }.apply(pyware_yards())] {
            for &(x, y) in &[(-46.875, -28.125), (51.875, 29.375), (0.3, -0.7), (33.731647, 17.612957)] {
                let (xs, ys) = t.to_steps(x, y);
                close(t.from_steps(xs, ys), (x, y));
            }
        }
    }

    #[test]
    fn rejects_invalid() {
        let mut t = pyware_yards();
        t.steps_per_unit_x = f64::NAN;
        assert!(t.check().is_err());
        assert!(pyware_yards().check().is_ok());
    }
}
