//! Per-performer paths (schema 5, `drill.paths.v1`): the holds, straight legs and circular
//! arcs one performer follows during one move, so curved and uneven moves stay exact without
//! derived subsets.
//!
//! A port of Fieldwork's `spec/examples/paths_reference.py`, following
//! `docs/FIELDWORK_SCHEMA5.md`: evaluation (§3), fitting per-count samples (§4) and canonical
//! form (§5). Points are meters from field center; durations are exact count fractions.

use std::cmp::Ordering;
use std::ops::{Add, Sub};

pub type Point = (f64, f64);

/// Largest distance between a fitted path and the source at any whole count (1 mm).
pub const FIT_TOLERANCE: f64 = 0.001;
/// Mid-count corners are rounded to fractions with at most this denominator.
pub const MAX_DENOMINATOR: i64 = 960;
const POINT_TOLERANCE: f64 = 1e-9;
const ANGLE_TOLERANCE: f64 = 1e-9;
const MINIMUM_SAGITTA: f64 = 1e-6;
const MINIMUM_CHORD: f64 = 1e-6;

/// An exact fraction in lowest terms with a positive denominator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ratio {
    pub num: i64,
    pub den: i64,
}

fn gcd(a: i64, b: i64) -> i64 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Ratio {
    pub const ZERO: Ratio = Ratio { num: 0, den: 1 };

    pub fn new(num: i64, den: i64) -> Ratio {
        assert!(den != 0, "zero denominator");
        let g = gcd(num, den).max(1) * den.signum();
        Ratio { num: num / g, den: den / g }
    }

    pub fn int(n: i64) -> Ratio {
        Ratio { num: n, den: 1 }
    }

    pub fn to_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// The closest fraction to `x` with a denominator of at most `max_den`, as Python's
    /// `Fraction(x).limit_denominator(max_den)` computes it from the float's exact value.
    pub fn limit_denominator(x: f64, max_den: i64) -> Ratio {
        if !x.is_finite() || x == 0.0 {
            return Ratio::ZERO;
        }
        let (n0, d0) = match exact(x.abs()) {
            Some(v) => v,
            None => return if x.abs() < 1.0 { Ratio::ZERO } else { Ratio::new(x.round() as i64, 1) },
        };
        let max = max_den as i128;
        let sign = if x < 0.0 { -1 } else { 1 };
        if d0 <= max {
            return Ratio::new(sign * n0 as i64, d0 as i64);
        }
        let (mut p0, mut q0, mut p1, mut q1) = (0i128, 1i128, 1i128, 0i128);
        let (mut n, mut d) = (n0, d0);
        loop {
            let a = n / d;
            let q2 = q0 + a * q1;
            if q2 > max {
                break;
            }
            (p0, q0, p1, q1) = (p1, q1, p0 + a * p1, q2);
            (n, d) = (d, n - a * d);
        }
        let k = (max - q0) / q1;
        let (p, q) = if 2 * d * (q0 + k * q1) <= d0 { (p1, q1) } else { (p0 + k * p1, q0 + k * q1) };
        Ratio::new(sign * p as i64, q as i64)
    }
}

/// The exact value of a positive float as `numerator / 2^k`, when it fits comfortably in i128.
fn exact(x: f64) -> Option<(i128, i128)> {
    let bits = x.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    if exponent == 0 || x >= (1u64 << 60) as f64 {
        return None;
    }
    let mut m = ((bits & ((1u64 << 52) - 1)) | (1u64 << 52)) as i128;
    let mut shift = 1075 - exponent;
    if shift > 120 {
        return None;
    }
    while shift > 0 && m & 1 == 0 {
        m >>= 1;
        shift -= 1;
    }
    if shift <= 0 {
        return Some((m << -shift, 1));
    }
    Some((m, 1i128 << shift))
}

impl Add for Ratio {
    type Output = Ratio;
    fn add(self, o: Ratio) -> Ratio {
        Ratio::new(self.num * o.den + o.num * self.den, self.den * o.den)
    }
}

impl Sub for Ratio {
    type Output = Ratio;
    fn sub(self, o: Ratio) -> Ratio {
        Ratio::new(self.num * o.den - o.num * self.den, self.den * o.den)
    }
}

impl PartialOrd for Ratio {
    fn partial_cmp(&self, o: &Ratio) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Ratio {
    fn cmp(&self, o: &Ratio) -> Ordering {
        (self.num as i128 * o.den as i128).cmp(&(o.num as i128 * self.den as i128))
    }
}

/// One piece of a performer's route through a move (spec §2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    /// Stand still at the current point.
    Hold { counts: Ratio },
    /// Straight leg at constant speed to `to`.
    Line { counts: Ratio, to: Point },
    /// Circular arc at constant speed to `to`, turning `sweep` degrees, positive clockwise
    /// seen from above (bulging to the left of start → end).
    Arc { counts: Ratio, to: Point, sweep: f64 },
}

impl Segment {
    pub fn counts(&self) -> Ratio {
        match *self {
            Segment::Hold { counts } | Segment::Line { counts, .. } | Segment::Arc { counts, .. } => counts,
        }
    }

    pub fn end(&self) -> Option<Point> {
        match *self {
            Segment::Hold { .. } => None,
            Segment::Line { to, .. } | Segment::Arc { to, .. } => Some(to),
        }
    }

    pub fn sweep(&self) -> Option<f64> {
        match *self {
            Segment::Arc { sweep, .. } => Some(sweep),
            _ => None,
        }
    }

    /// The `kind` column value.
    pub fn kind(&self) -> &'static str {
        match self {
            Segment::Hold { .. } => "hold",
            Segment::Line { .. } => "line",
            Segment::Arc { .. } => "arc",
        }
    }

    pub fn is_hold(&self) -> bool {
        matches!(self, Segment::Hold { .. })
    }

    fn with_end(self, end: Point) -> Segment {
        match self {
            Segment::Hold { .. } => self,
            Segment::Line { counts, .. } => Segment::Line { counts, to: end },
            Segment::Arc { counts, sweep, .. } => Segment::Arc { counts, to: end, sweep },
        }
    }
}

fn hold(counts: Ratio) -> Segment {
    Segment::Hold { counts }
}

fn line(counts: Ratio, to: Point) -> Segment {
    Segment::Line { counts, to }
}

fn sub(a: Point, b: Point) -> Point {
    (a.0 - b.0, a.1 - b.1)
}
fn add(a: Point, b: Point) -> Point {
    (a.0 + b.0, a.1 + b.1)
}
fn mul(a: Point, k: f64) -> Point {
    (a.0 * k, a.1 * k)
}
fn dist(a: Point, b: Point) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}
fn cross(a: Point, b: Point) -> f64 {
    a.0 * b.1 - a.1 * b.0
}
fn dot(a: Point, b: Point) -> f64 {
    a.0 * b.0 + a.1 * b.1
}
fn norm(a: Point) -> f64 {
    a.0.hypot(a.1)
}

// ---------------------------------------------------------------------------
// Evaluation (§3), matching Fieldwork's ArcGeometry and ResolvedPath.

/// Center, radius, start angle and sweep in radians of the arc from `p0` to `p1`, or `None`
/// when the chord is too short.
fn arc_geometry(p0: Point, p1: Point, sweep_degrees: f64) -> Option<(Point, f64, f64, f64)> {
    let c = sub(p1, p0);
    let length = norm(c);
    let theta = sweep_degrees.to_radians();
    if length <= 1e-12 || theta == 0.0 || !theta.is_finite() {
        return None;
    }
    let mid = mul(add(p0, p1), 0.5);
    let left = (-c.1 / length, c.0 / length);
    let h = (length / 2.0) / (theta / 2.0).tan();
    let center = sub(mid, mul(left, h));
    Some((center, dist(p0, center), (p0.1 - center.1).atan2(p0.0 - center.0), theta))
}

/// Position at fraction `s` (0..1) of one segment that starts at `p0`.
pub fn segment_point(p0: Point, seg: &Segment, s: f64) -> Point {
    let (to, sweep) = match *seg {
        Segment::Hold { .. } => return p0,
        Segment::Line { to, .. } => (to, None),
        Segment::Arc { to, sweep, .. } => (to, Some(sweep)),
    };
    if s >= 1.0 {
        return to;
    }
    if s <= 0.0 {
        return p0;
    }
    match sweep.and_then(|w| arc_geometry(p0, to, w)) {
        Some((center, r, phi0, theta)) => {
            let a = phi0 - theta * s;
            (center.0 + r * a.cos(), center.1 + r * a.sin())
        }
        None => add(p0, mul(sub(to, p0), s)),
    }
}

/// Position `u` counts into the move. A boundary belongs to the later segment.
pub fn path_point(start: Point, segments: &[Segment], u: f64) -> Point {
    let total = segments.iter().fold(Ratio::ZERO, |t, s| t + s.counts());
    let u = u.clamp(0.0, total.to_f64());
    let (mut point, mut offset) = (start, Ratio::ZERO);
    for (i, seg) in segments.iter().enumerate() {
        let last = i == segments.len() - 1;
        if u < (offset + seg.counts()).to_f64() || last {
            return segment_point(point, seg, (u - offset.to_f64()) / seg.counts().to_f64());
        }
        point = seg.end().unwrap_or(point);
        offset = offset + seg.counts();
    }
    point
}

/// Where the path ends: the end of its last line or arc, or its start.
pub fn path_end(start: Point, segments: &[Segment]) -> Point {
    segments.iter().rev().find_map(|s| s.end()).unwrap_or(start)
}

// ---------------------------------------------------------------------------
// Canonical form (§5), a port of PerformerPath.canonical and merge.

fn sagitta(chord: f64, sweep_degrees: f64) -> f64 {
    (chord / 2.0) * (sweep_degrees.to_radians() / 4.0).tan()
}

fn merge(a: &Segment, a_start: Point, b: &Segment) -> Option<Segment> {
    match (*a, *b) {
        (Segment::Hold { counts: c1 }, Segment::Hold { counts: c2 }) => Some(hold(c1 + c2)),
        (Segment::Line { counts: c1, to: e1 }, Segment::Line { counts: c2, to: e2 }) => {
            let v1 = mul(sub(e1, a_start), 1.0 / c1.to_f64());
            let v2 = mul(sub(e2, e1), 1.0 / c2.to_f64());
            let (l1, l2) = (norm(v1), norm(v2));
            if (l1 - l2).abs() > POINT_TOLERANCE || l1 <= 0.0 || l2 <= 0.0 {
                return None;
            }
            if cross(v1, v2).abs() / (l1 * l2) > ANGLE_TOLERANCE || dot(v1, v2) <= 0.0 {
                return None;
            }
            Some(line(c1 + c2, e2))
        }
        (Segment::Arc { counts: c1, to: e1, sweep: s1 }, Segment::Arc { counts: c2, to: e2, sweep: s2 }) => {
            let g1 = arc_geometry(a_start, e1, s1)?;
            let g2 = arc_geometry(e1, e2, s2)?;
            let (w1, w2) = (g1.3 / c1.to_f64(), g2.3 / c2.to_f64());
            (dist(g1.0, g2.0) <= POINT_TOLERANCE && (g1.1 - g2.1).abs() <= POINT_TOLERANCE && (w1 - w2).abs() <= ANGLE_TOLERANCE && (s1 + s2).abs() < 360.0)
                .then_some(Segment::Arc { counts: c1 + c2, to: e2, sweep: s1 + s2 })
        }
        _ => None,
    }
}

/// The canonical segment list, or `None` when the move needs no path rows.
pub fn canonical(segments: &[Segment], start: Point, end: Point) -> Option<Vec<Segment>> {
    let (mut out, mut starts): (Vec<Segment>, Vec<Point>) = (Vec::new(), Vec::new());
    let mut point = start;
    for &seg in segments {
        if seg.counts() <= Ratio::ZERO {
            continue;
        }
        let mut seg = seg;
        if let Segment::Arc { counts, to, sweep } = seg {
            let chord = dist(point, to);
            if chord <= MINIMUM_CHORD || sagitta(chord, sweep).abs() < MINIMUM_SAGITTA {
                seg = line(counts, to);
            }
        }
        if let Segment::Line { counts, to } = seg {
            if dist(point, to) <= POINT_TOLERANCE {
                seg = hold(counts);
            }
        }
        match out.last().and_then(|last| merge(last, *starts.last().unwrap(), &seg)) {
            Some(merged) => *out.last_mut().unwrap() = merged,
            None => {
                out.push(seg);
                starts.push(point);
            }
        }
        point = seg.end().unwrap_or(point);
    }
    if out.iter().all(Segment::is_hold) && dist(start, end) <= POINT_TOLERANCE {
        return None;
    }
    if let [Segment::Line { to, .. }] = out.as_slice() {
        if *to == end {
            return None;
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Fitting per-count samples (§4).

fn fits_hold(samples: &[Point], i: usize, j: usize, start: Point) -> bool {
    (i + 1..=j).all(|k| dist(samples[k], start) <= FIT_TOLERANCE)
}

fn fits_line(samples: &[Point], i: usize, j: usize, start: Point) -> bool {
    let seg = line(Ratio::int((j - i) as i64), samples[j]);
    (i + 1..j).all(|k| dist(segment_point(start, &seg, (k - i) as f64 / (j - i) as f64), samples[k]) <= FIT_TOLERANCE)
}

fn circle_center(a: Point, b: Point, c: Point) -> Option<Point> {
    let d = 2.0 * (a.0 * (b.1 - c.1) + b.0 * (c.1 - a.1) + c.0 * (a.1 - b.1));
    if d.abs() < 1e-12 {
        return None;
    }
    let (a2, b2, c2) = (dot(a, a), dot(b, b), dot(c, c));
    Some((
        (a2 * (b.1 - c.1) + b2 * (c.1 - a.1) + c2 * (a.1 - b.1)) / d,
        (a2 * (c.0 - b.0) + b2 * (a.0 - c.0) + c2 * (b.0 - a.0)) / d,
    ))
}

/// Signed angle difference in (−π, π].
fn wrap(d: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    if d > PI {
        d - TAU
    } else if d < -PI {
        d + TAU
    } else {
        d
    }
}

/// An arc through samples `i..=j` at even angular speed, or `None`. The sweep comes from the
/// samples.
fn fit_arc(samples: &[Point], i: usize, j: usize, start: Point) -> Option<Segment> {
    if j - i < 2 {
        return None;
    }
    let center = circle_center(start, samples[(i + j) / 2], samples[j])?;
    let angle = |p: Point| (p.1 - center.1).atan2(p.0 - center.0);
    let mut previous = angle(start);
    let (mut total, mut positive, mut negative) = (0.0, true, true);
    for k in i + 1..=j {
        let a = angle(samples[k]);
        let d = wrap(a - previous);
        positive &= d > 0.0;
        negative &= d < 0.0;
        total += d;
        previous = a;
    }
    if !(positive || negative) {
        return None;
    }
    let sweep = -total.to_degrees(); // clockwise is positive
    if !(0.0 < sweep.abs() && sweep.abs() < 360.0) {
        return None;
    }
    let seg = Segment::Arc { counts: Ratio::int((j - i) as i64), to: samples[j], sweep };
    (i + 1..j).all(|k| dist(segment_point(start, &seg, (k - i) as f64 / (j - i) as f64), samples[k]) <= FIT_TOLERANCE).then_some(seg)
}

/// The longest hold, line or arc from sample `i`, with its last sample index. Ties prefer
/// hold, then line.
fn longest_run(samples: &[Point], i: usize, start: Point, arcs: bool) -> (Segment, usize) {
    let n = samples.len() - 1;
    let mut best: Option<(Segment, usize)> = None;
    let mut j = i;
    while j < n && fits_hold(samples, i, j + 1, start) {
        j += 1;
    }
    if j > i {
        best = Some((hold(Ratio::int((j - i) as i64)), j));
    }
    let mut j = i + 1;
    while j < n && fits_line(samples, i, j + 1, start) {
        j += 1;
    }
    if best.map_or(true, |b| j > b.1) {
        best = Some((line(Ratio::int((j - i) as i64), samples[j]), j));
    }
    let mut best = best.unwrap();
    if arcs {
        let mut found = None;
        let mut j = i + 2;
        while j <= n {
            let Some(seg) = fit_arc(samples, i, j, start) else { break };
            found = Some((seg, j));
            j += 1;
        }
        if let Some(f) = found.filter(|f| f.1 > best.1) {
            best = f;
        }
    }
    best
}

fn velocity(seg: &Segment, seg_start: Point) -> Point {
    match seg.end() {
        None => (0.0, 0.0),
        Some(end) => mul(sub(end, seg_start), 1.0 / seg.counts().to_f64()),
    }
}

/// Time `t1` (0 < t1 < 1) after `p` at which leg A (velocity `va`) turns into leg B
/// (velocity `vb`, reaching `q` at t = 1).
fn solve_corner(p: Point, q: Point, va: Point, vb: Point) -> Option<f64> {
    let w = sub(q, p);
    let (sa, sb) = (norm(va), norm(vb));
    if sa == 0.0 && sb == 0.0 {
        return None;
    }
    let det = cross(va, vb);
    let t1 = if sa == 0.0 {
        // A hold, then a line that starts mid-count.
        if cross(w, vb).abs() / sb > FIT_TOLERANCE || dot(w, vb) <= 0.0 {
            return None;
        }
        1.0 - norm(w) / sb
    } else if sb == 0.0 {
        // A line that arrives mid-count, then a hold.
        if cross(w, va).abs() / sa > FIT_TOLERANCE || dot(w, va) <= 0.0 {
            return None;
        }
        norm(w) / sa
    } else if det.abs() / (sa * sb) <= ANGLE_TOLERANCE {
        // Same direction, new pace.
        if cross(w, va).abs() / sa > FIT_TOLERANCE || dot(va, vb) <= 0.0 || (sa - sb).abs() <= 1e-12 {
            return None;
        }
        (norm(w) - sb) / (sa - sb)
    } else {
        // A true corner between two straight legs.
        let (t1, t2) = (cross(w, vb) / det, cross(va, w) / det);
        if (t1 + t2 - 1.0).abs() * sa.max(sb) > FIT_TOLERANCE {
            return None;
        }
        t1
    };
    (0.0 < t1 && t1 < 1.0).then_some(t1)
}

/// Segments for one performer's move from samples at counts 0, 1, …, n, before
/// canonicalization. With `corners` false no mid-count corners are solved; with `runs` false
/// every count becomes its own 1-count line.
fn fit_move(samples: &[Point], corners: bool, runs: bool) -> Vec<Segment> {
    let n = samples.len() - 1;
    let mut out: Vec<Segment> = Vec::new();
    let mut starts: Vec<Point> = Vec::new(); // the point each segment starts at
    let mut fitted: Vec<bool> = Vec::new(); // true for runs, false for 1-count fallbacks
    let (mut point, mut i) = (samples[0], 0);
    while i < n {
        let (seg, j) = if runs { longest_run(samples, i, point, true) } else { (line(Ratio::int(1), samples[i + 1]), i + 1) };
        if corners && j == i + 1 && i + 1 < n {
            // A 1-count chord may hide a corner between two legs (hold or line).
            let a = match out.last() {
                Some(last) if *fitted.last().unwrap() && !matches!(last, Segment::Arc { .. }) => Some((*last, *starts.last().unwrap())),
                None => Some((hold(Ratio::ZERO), point)), // the move's start is a hold of no length
                _ => None,
            };
            if let Some((a, a_start)) = a {
                let (b_seg, k) = longest_run(samples, i + 1, samples[i + 1], false);
                if b_seg.is_hold() || k - (i + 1) >= 2 {
                    let va = if a.counts() == Ratio::ZERO { (0.0, 0.0) } else { velocity(&a, a_start) };
                    let t1 = solve_corner(point, samples[i + 1], va, velocity(&b_seg, samples[i + 1])).map(|t| Ratio::limit_denominator(t, MAX_DENOMINATOR));
                    if let Some(t1) = t1.filter(|t| Ratio::ZERO < *t && *t < Ratio::int(1)) {
                        let corner = add(point, mul(va, t1.to_f64()));
                        let a2 = if a.is_hold() { hold(a.counts() + t1) } else { line(a.counts() + t1, corner) };
                        let b_counts = Ratio::int((k - i - 1) as i64) + (Ratio::int(1) - t1);
                        let b2 = if b_seg.is_hold() { hold(b_counts) } else { line(b_counts, samples[k]) };
                        if out.is_empty() {
                            out.push(a2);
                            starts.push(a_start);
                            fitted.push(true);
                        } else {
                            *out.last_mut().unwrap() = a2;
                        }
                        out.push(b2);
                        starts.push(corner);
                        fitted.push(true);
                        point = b2.end().unwrap_or(corner);
                        i = k;
                        continue;
                    }
                }
            }
        }
        out.push(seg);
        starts.push(point);
        fitted.push(j > i + 1 || seg.is_hold());
        point = seg.end().unwrap_or(point);
        i = j;
    }
    // The path must end on the arrival placement's exact doubles.
    match out.iter().rposition(|s| !s.is_hold()) {
        Some(last) => out[last] = out[last].with_end(samples[n]),
        // Never moves beyond the tolerance, yet ends elsewhere: walk the last hold there.
        None if samples[n] != samples[0] => {
            let last = out.len() - 1;
            out[last] = line(out[last].counts(), samples[n]);
        }
        None => {}
    }
    out
}

/// Largest distance between the path (straight when `None`) and the samples at whole counts.
pub fn max_error(samples: &[Point], segments: Option<&[Segment]>) -> f64 {
    let n = samples.len() - 1;
    let straight = [line(Ratio::int(n as i64), samples[n])];
    let segments = segments.unwrap_or(&straight);
    samples.iter().enumerate().map(|(u, &p)| dist(path_point(samples[0], segments, u as f64), p)).fold(0.0, f64::max)
}

/// Canonical segments for one performer's move from its positions at counts 0…n, or
/// `Ok(None)` when it moves straight at even steps (or not at all) and needs no rows.
/// `samples[0]` and `samples[n]` must be the placements' exact doubles.
///
/// Every whole count is checked against the samples. A fit with mid-count corners is tried
/// first, then one without, then one 1-count line per count. `Err` carries the worst error
/// when even that is off by more than [`FIT_TOLERANCE`] (only for non-finite input).
pub fn path_for_move(samples: &[Point]) -> Result<Option<Vec<Segment>>, f64> {
    assert!(samples.len() >= 2, "a move spans at least one count");
    let (start, end) = (samples[0], samples[samples.len() - 1]);
    let mut worst = f64::NAN;
    for (corners, runs) in [(true, true), (false, true), (false, false)] {
        let segments = canonical(&fit_move(samples, corners, runs), start, end);
        let error = max_error(samples, segments.as_deref());
        if error <= FIT_TOLERANCE && segments.as_deref().map_or(true, |s| dist(path_end(start, s), end) <= 1e-6) {
            return Ok(segments);
        }
        worst = error;
    }
    Err(worst)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(n: i64, d: i64) -> Ratio {
        Ratio::new(n, d)
    }
    fn arc(counts: Ratio, to: Point, sweep: f64) -> Segment {
        Segment::Arc { counts, to, sweep }
    }
    fn near(a: Point, b: Point) -> bool {
        dist(a, b) <= 1e-6
    }
    fn samples_of(start: Point, segments: &[Segment]) -> Vec<Point> {
        let n = segments.iter().fold(Ratio::ZERO, |t, s| t + s.counts()).to_f64() as usize;
        (0..=n).map(|u| path_point(start, segments, u as f64)).collect()
    }
    fn kinds(segments: &Option<Vec<Segment>>) -> Option<Vec<&'static str>> {
        segments.as_ref().map(|s| s.iter().map(Segment::kind).collect())
    }

    #[test]
    fn ratios() {
        assert_eq!(r(6, -4), r(-3, 2));
        assert_eq!(r(1, 3) + r(1, 6), r(1, 2));
        assert!(r(1, 3) < r(1, 2));
        assert_eq!(Ratio::limit_denominator(0.5, 960), r(1, 2));
        // The same results as Python's Fraction(x).limit_denominator(960).
        assert_eq!(Ratio::limit_denominator(0.603, 960), r(442, 733));
        assert_eq!(Ratio::limit_denominator(std::f64::consts::PI - 3.0, 960), r(16, 113));
        assert_eq!(Ratio::limit_denominator(0.123456789, 960), r(10, 81));
        assert_eq!(Ratio::limit_denominator(0.999, 960), r(959, 960));
        assert_eq!(Ratio::limit_denominator(1.0 / 3.0, 960), r(1, 3));
        assert_eq!(Ratio::limit_denominator(1e-9, 960), Ratio::ZERO);
    }

    #[test]
    fn evaluator_vectors() {
        let v = [hold(r(4, 1)), line(r(4, 1), (4.0, 0.0))];
        for (u, p) in [(2.0, (0.0, 0.0)), (4.0, (0.0, 0.0)), (6.0, (2.0, 0.0)), (8.0, (4.0, 0.0))] {
            assert!(near(path_point((0.0, 0.0), &v, u), p));
        }
        assert!(near(arc_geometry((0.0, 10.0), (10.0, 0.0), 90.0).unwrap().0, (0.0, 0.0)));
        let v = [arc(r(4, 1), (10.0, 0.0), 90.0)];
        assert!(near(path_point((0.0, 10.0), &v, 1.0), (3.826834, 9.238795)));
        assert!(near(path_point((0.0, 10.0), &v, 2.0), (7.071068, 7.071068)));
        assert!(near(path_point((10.0, 0.0), &[arc(r(4, 1), (0.0, 10.0), -90.0)], 2.0), (7.071068, 7.071068)));
        let v = [arc(r(3, 1), (0.0, 10.0), 270.0)];
        for (u, p) in [(1.0, (0.0, -10.0)), (2.0, (-10.0, 0.0)), (3.0, (0.0, 10.0))] {
            assert!(near(path_point((10.0, 0.0), &v, u), p));
        }
        let v = [line(r(9, 2), (4.5, 0.0)), line(r(7, 2), (4.5, 3.5))];
        for (u, p) in [(4.0, (4.0, 0.0)), (5.0, (4.5, 0.5)), (8.0, (4.5, 3.5))] {
            assert!(near(path_point((0.0, 0.0), &v, u), p));
        }
    }

    #[test]
    fn fitting_cases() {
        let corner = r(4603, 1000);
        let cases: Vec<(&str, Vec<Point>, Option<Vec<&str>>)> = vec![
            ("corner 4.603 counts in", samples_of((0.0, 0.0), &[line(corner, (4.603, 0.0)), line(r(8, 1) - corner, (4.603, 3.397))]), Some(vec!["line", "line"])),
            ("new pace mid-leg", samples_of((0.0, 0.0), &[line(r(5, 2), (2.5, 0.0)), line(r(7, 2), (4.25, 0.0))]), Some(vec!["line", "line"])),
            ("arrives early mid-count", samples_of((0.0, 0.0), &[line(r(11, 2), (5.5, 0.0)), hold(r(5, 2))]), Some(vec!["line", "hold"])),
            ("straight", samples_of((0.0, 0.0), &[line(r(8, 1), (6.0, 2.0))]), None),
            ("standing still", vec![(3.0, 1.0); 9], None),
            ("noisy arc", samples_of((0.0, 10.0), &[arc(r(8, 1), (10.0, 0.0), 90.0)]).into_iter().enumerate().map(|(k, p)| (p.0 + 0.0003 * if k % 2 == 0 { 1.0 } else { -1.0 }, p.1)).collect(), Some(vec!["arc"])),
        ];
        for (name, samples, want) in cases {
            let got = path_for_move(&samples).unwrap();
            assert!(max_error(&samples, got.as_deref()) <= FIT_TOLERANCE, "{name}");
            assert_eq!(kinds(&got), want, "{name}");
        }
        let curve: Vec<Point> = (0..9).map(|u| (u as f64 * 0.75, 0.04 * (u * u * u) as f64)).collect();
        let got = path_for_move(&curve).unwrap();
        assert!(kinds(&got).unwrap().iter().all(|k| *k == "line"), "smooth curve");
        assert!(max_error(&curve, got.as_deref()) <= FIT_TOLERANCE);
    }

    /// The rows of the example in §8, fitted from whole-count samples only.
    #[test]
    fn spec_example() {
        let moves: Vec<(Point, Vec<Segment>)> = vec![
            ((-8.0, -4.0), vec![hold(r(2, 1)), line(r(6, 1), (-2.0, -4.0))]),
            ((0.0, 10.0), vec![arc(r(8, 1), (10.0, 0.0), 90.0)]),
            ((0.0, 0.0), vec![line(r(9, 2), (4.5, 0.0)), line(r(7, 2), (4.5, 3.5))]),
            ((10.0, 0.0), vec![arc(r(6, 1), (0.0, 10.0), 270.0), hold(r(2, 1))]),
        ];
        for (start, want) in moves {
            let got = path_for_move(&samples_of(start, &want)).unwrap().unwrap();
            assert_eq!(got.len(), want.len());
            for (g, w) in got.iter().zip(&want) {
                assert_eq!((g.kind(), g.counts()), (w.kind(), w.counts()));
                assert!(g.end().zip(w.end()).map_or(g.end() == w.end(), |(a, b)| dist(a, b) <= 1e-9));
                assert!(g.sweep().zip(w.sweep()).map_or(g.sweep() == w.sweep(), |(a, b)| (a - b).abs() <= 1e-6));
            }
        }
        assert_eq!(path_for_move(&samples_of((0.0, -6.0), &[line(r(8, 1), (4.0, -6.0))])).unwrap(), None);
    }

    #[test]
    fn ends_exactly_on_the_arrival_placement() {
        // Jitter below the tolerance around a fixed point, ending 0.4 mm away.
        let samples = [(1.0, 1.0), (1.0002, 1.0), (1.0, 0.9998), (1.0004, 1.0)];
        let got = path_for_move(&samples).unwrap();
        let end = got.as_deref().map_or(samples[3], |s| path_end(samples[0], s));
        assert_eq!(end, samples[3]);
    }
}
