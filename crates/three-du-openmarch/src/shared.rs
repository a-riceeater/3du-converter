use three_du_core::{Diagnostic, DrillProject, Performer, Severity};

/// Duration in seconds of each beat `0..=drill_end_count`; beat 0 (the first page) has none.
pub fn beat_durations(p: &DrillProject) -> Vec<f64> {
    let end = p.timeline.drill_end_count as usize;
    let mut out = Vec::with_capacity(end + 1);
    out.push(0.0);
    for c in 1..=end {
        out.push(p.timeline.duration_of(c));
    }
    out
}

pub fn performer_section(q: &Performer) -> String {
    q.section.clone().unwrap_or_else(|| "Other".to_string())
}

/// Canonical step coordinates of every performer at every set: `[set][performer] = (x, y)`.
/// A missing sample is filled from the nearest stored count and reported as lossy.
pub fn page_coordinates(p: &DrillProject, diags: &mut Vec<Diagnostic>) -> Vec<Vec<(f64, f64)>> {
    let mut filled = 0;
    let rows = p
        .sets
        .iter()
        .map(|s| {
            let c = s.start_count as usize;
            (0..p.performers.len())
                .map(|i| {
                    p.steps(i, c).unwrap_or_else(|| {
                        filled += 1;
                        nearest(p, i, c).unwrap_or((0.0, 0.0))
                    })
                })
                .collect()
        })
        .collect();
    if filled > 0 {
        diags.push(Diagnostic::new(
            Severity::Lossy,
            "missing_set_positions_filled",
            format!("{filled} performer position(s) at set boundaries were missing and copied from the nearest stored count"),
        ));
    }
    rows
}

fn nearest(p: &DrillProject, performer: usize, count: usize) -> Option<(f64, f64)> {
    let n = p.timeline.count_len();
    (1..n).find_map(|d| {
        count.checked_sub(d).and_then(|c| p.steps(performer, c)).or_else(|| p.steps(performer, count + d))
    })
}

/// Splits a drill label into OpenMarch's prefix + order ("T12" → ("T", 12)).
pub fn split_label(label: &str) -> (String, Option<u32>) {
    let digits = label.chars().rev().take_while(|c| c.is_ascii_digit()).count();
    let (prefix, number) = label.split_at(label.len() - digits);
    (prefix.to_string(), number.parse().ok())
}

/// Assigns unique (prefix, order) pairs, keeping the source numbering whenever it is unique.
pub fn unique_prefix_orders(labels: &[String], diags: &mut Vec<Diagnostic>) -> Vec<(String, u32)> {
    use std::collections::{HashMap, HashSet};
    let split: Vec<(String, Option<u32>)> = labels.iter().map(|l| split_label(l)).collect();
    let mut used: HashSet<(String, u32)> = HashSet::new();
    let mut out: Vec<Option<(String, u32)>> = vec![None; labels.len()];
    for (i, (prefix, order)) in split.iter().enumerate() {
        if let Some(o) = order {
            if used.insert((prefix.clone(), *o)) {
                out[i] = Some((prefix.clone(), *o));
            }
        }
    }
    let mut next: HashMap<String, u32> = HashMap::new();
    let mut renamed = 0;
    for (i, (prefix, _)) in split.iter().enumerate() {
        if out[i].is_some() {
            continue;
        }
        let prefix = if prefix.is_empty() { "P".to_string() } else { prefix.clone() };
        let n = next.entry(prefix.clone()).or_insert(1);
        while used.contains(&(prefix.clone(), *n)) {
            *n += 1;
        }
        used.insert((prefix.clone(), *n));
        out[i] = Some((prefix, *n));
        renamed += 1;
    }
    if renamed > 0 {
        diags.push(Diagnostic::new(
            Severity::Warning,
            "labels_renumbered",
            format!("{renamed} performer label(s) had no number or a duplicate and were numbered to make them unique"),
        ));
    }
    out.into_iter().map(Option::unwrap).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(split_label("T12"), ("T".into(), Some(12)));
        assert_eq!(split_label("ts8"), ("ts".into(), Some(8)));
        assert_eq!(split_label("unlabeled"), ("unlabeled".into(), None));
        let mut d = vec![];
        let labels: Vec<String> = ["T1", "T1", "unlabeled", "unlabeled", "7"].iter().map(|s| s.to_string()).collect();
        let got = unique_prefix_orders(&labels, &mut d);
        assert_eq!(got, vec![("T".into(), 1), ("T".into(), 2), ("unlabeled".into(), 1), ("unlabeled".into(), 2), ("".into(), 7)]);
        assert_eq!(d.len(), 1);
    }
}
