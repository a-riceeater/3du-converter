# `.fieldwork` schema 5: curved paths (converter add-on spec)

**How to use this file:** this is the spec the converter's `.fieldwork` writer follows for paths ([`paths.rs`](../crates/three-du-fieldwork/src/paths.rs), mapping in [FIELDWORKS.md](FIELDWORKS.md)). It describes only what is **new** in schema 5. Everything not mentioned here (container, manifest layout, the other tables, timing, placements) is unchanged from schema 4.

Fieldwork has a longer version with a fitting walk-through and a runnable Python reference: `spec/converter-paths.md` and `spec/examples/paths_reference.py` in the Fieldwork repository. If this file and the Fieldwork code disagree, the code wins.

---

## 1. What changed

| | Schema 4 (today's converter) | Schema 5 |
|---|---|---|
| Curved, staggered or pace-changing moves | Kept exact only by adding derived subsets (`60~1`, `60~2`, …) at every count where someone turns or changes speed | One row per hold, straight leg or arc in a new `path_segment` table |
| Sets | Designer sets plus derived `~n` subsets | **Designer sets only.** Subsets the designer actually wrote keep `is_subset = 1`. |
| `transition.model` | `straight` | Still `straight`. It is the model for performers **without** path rows. |
| `PRAGMA user_version` | 4 | 5 when at least one path exists, otherwise 4 |
| Manifest | `drill.core.v1`, `timing.anchors.v1` (+ `media.audio.v1`) | Adds `drill.paths.v1` to **both** `capabilities` and `requiredCapabilities` when at least one path exists |

Nothing else changes. OpenMarch output keeps using subsets, because OpenMarch 0.1.7 ignores its midsets.

---

## 2. The new table

Add this to the schema 4 database, exactly as written:

```sql
CREATE TABLE path_segment (transition_id TEXT NOT NULL REFERENCES transition(id), performer_id TEXT NOT NULL REFERENCES performer(id),
    ordinal INTEGER NOT NULL, kind TEXT NOT NULL CHECK (kind IN ('hold','line','arc')),
    counts_num INTEGER NOT NULL, counts_den INTEGER NOT NULL, x1 REAL, y1 REAL, sweep REAL,
    PRIMARY KEY (transition_id, performer_id, ordinal));
PRAGMA user_version = 5;
```

| Column | Value |
|---|---|
| `transition_id` | The `transition` of the move into set *B*. The move starts at set *A*, the previous set in the same movement. |
| `performer_id` | The performer. They must be `placed` at both *A* and *B*. |
| `ordinal` | 0, 1, 2, … within `(transition_id, performer_id)`, with no gaps |
| `kind` | `hold`, `line` or `arc`, in lowercase |
| `counts_num`, `counts_den` | Duration in **count pulses** as an exact fraction. Bind both as SQLite **INTEGER** (never REAL), in lowest terms, both > 0. Durations need not be whole, because corners can fall between counts. |
| `x1`, `y1` | End point in meters for `line` and `arc`. NULL for `hold`. |
| `sweep` | `arc` only: the signed turn in degrees, 0 < \|sweep\| < 360. NULL for `hold` and `line`. |

Rules for the rows:

- **Implicit start.** A path starts at the performer's placement at *A*. Each segment starts where the previous one ends, and a hold ends where it starts.
- **Exact end.** The last `line` or `arc` ends at the performer's placement at *B*. Write that placement's exact doubles; the reader allows only 1e-6 m.
- **Exact length.** The durations add up **exactly** to the move's length in counts, `count(B) − count(A)`.
- **Sweep sign.** Positive is **clockwise seen from above** (+x is the audience's right, +y is the back sideline), so the arc bulges to the **left** of start→end. A full circle is written as two arcs.
- **Straight is the default.** A performer with no rows moves straight from *A* to *B* at constant speed, as in schema 4. Write rows **only** when the move is not one straight line at even steps.
- **Order.** Insert rows by arrival-set ordinal, then performer ordinal, then `ordinal`.
- `PRAGMA foreign_key_check` must return nothing.

---

## 3. How Fieldwork plays a path

For count `u` into the move, clamped to `[0, move length]`:

1. Walk the segments, adding durations, to the one that contains `u`. **A boundary belongs to the later segment.**
2. Let `s = (u − segmentStart) / segmentCounts`, `P0` the segment's start and `P1` its end.
   - `hold`: `P0`.
   - `line`: `P0 + (P1 − P0)·s`.
   - `arc` with sweep θ in radians:
     ```
     c = P1 − P0,  L = |c|,  M = (P0 + P1)/2,  n = (−c.y, c.x)/L      // left normal
     h = (L/2) / tan(θ/2)
     C = M − n·h,  r = |P0 − C|,  φ0 = atan2(P0.y − C.y, P0.x − C.x)
     P(s) = C + r·(cos(φ0 − θ·s), sin(φ0 − θ·s))       // exactly P1 at s = 1
     ```
3. Speed is constant within a segment, and there is no easing. `z` and facing interpolate over the whole move, as before.

```python
import math

def point_at(start, segments, u):
    """segments: [(kind, counts, end or None, sweep or None)], counts as fractions.Fraction."""
    total = sum(s[1] for s in segments)
    u = min(max(u, 0.0), float(total))
    p, t = start, 0
    for i, (kind, counts, end, sweep) in enumerate(segments):
        if u < float(t + counts) or i == len(segments) - 1:
            s = (u - float(t)) / float(counts)
            if kind == "hold": return p
            if s >= 1: return end
            if kind == "line": return (p[0] + (end[0] - p[0]) * s, p[1] + (end[1] - p[1]) * s)
            cx, cy = end[0] - p[0], end[1] - p[1]
            L, th = math.hypot(cx, cy), math.radians(sweep)
            h = (L / 2) / math.tan(th / 2)
            C = ((p[0] + end[0]) / 2 + cy / L * h, (p[1] + end[1]) / 2 - cx / L * h)
            r, a0 = math.dist(p, C), math.atan2(p[1] - C[1], p[0] - C[0])
            return (C[0] + r * math.cos(a0 - th * s), C[1] + r * math.sin(a0 - th * s))
        p = end if end is not None else p
        t += counts
    return p
```

Check values: from (0, 10), `arc 4 → (10, 0)` with sweep +90 has its center at (0, 0) and is at (7.071068, 7.071068) at u = 2. From (0, 0), `line 9/2 → (4.5, 0)` then `line 7/2 → (4.5, 3.5)` is at (4.5, 0.5) at u = 5.

---

## 4. Turning what the converter has into segments

Today the converter knows every performer's position at every count, and it uses that to decide where to add subsets. Use the same per-count positions as the input here. For one performer and one designer move, the input is `samples[0…n]`, the positions at counts 0…n of the move. `samples[0]` and `samples[n]` are the placements at *A* and *B*.

1. **Greedy runs.** From the current sample, take the longest run that is, within **1 mm** at every sample, one of these:
   - a **hold**: no movement;
   - a **line**: even steps along one line;
   - an **arc**: even angular steps on one circle, all turning the same way. Use the circle through the run's start, middle and end samples, and take the sweep from the samples.

   On a tie, prefer hold, then line, then arc. A segment ends on a sample point.
2. **Mid-count corners.** If only a 1-count chord fits between two legs, solve for the moment in that count where leg A (a hold or line) turns into leg B (a hold or line of at least 2 counts). Let `w` be the chord, and `va`, `vb` the legs' velocities in meters per count:

   | Case | Corner time `t1` into the count |
   |---|---|
   | line → line, turning | `t1 = cross(w, vb) / cross(va, vb)`; accept if `t2 = cross(va, w) / cross(va, vb)` gives `t1 + t2 ≈ 1` within 1 mm |
   | line → line, same direction, new pace | `t1 = (\|w\| − \|vb\|) / (\|va\| − \|vb\|)` |
   | hold → line (starts mid-count) | `t1 = 1 − \|w\| / \|vb\|` |
   | line → hold (arrives mid-count) | `t1 = \|w\| / \|va\|` |

   Accept only when 0 < t1 < 1. Round `t1` to the nearest fraction with a denominator of **at most 960**, and use `t2 = 1 − t1` exactly. Leg A grows by `t1` counts and ends at the corner; leg B starts there and gains `t2`. For example, a corner at count 4.5 gives `line 9/2`, `line 7/2`.
3. **Fallback.** Anything left becomes 1-count lines to each sample. A smooth curve that is neither a line nor a circle ends up as a chain of 1-count lines.
4. **Snap the end.** Set the last line or arc's end to the placement at *B*, using its exact doubles.
5. **Canonicalize** (§5). If what is left is one straight line, or no movement at all, write **no rows**.

Then **check before writing**: evaluate every whole count with §3. If any count is more than **1 mm** from the source, fit again without corners. If it still fails, refuse the output.

The converter's survey of a real show found three primitives that cover every move: holds, even-step lines and even-speed arcs. Rebuilt from segments, every count matched within 1.1 mm, so no Bézier or easing is needed.

---

## 5. Canonical form

Fieldwork rewrites paths into this form on save. Write it this way yourself so a converted file stays byte-stable.

- An arc whose chord is ≤ 1e-6 m, or whose sagitta `(L/2)·tan(θ/4)` is below 1e-6 m, is written as a line.
- A line shorter than 1e-9 m is written as a hold.
- Two adjacent holds merge.
- Two adjacent lines with the same velocity merge (speed within 1e-9 m per count, direction within 1e-9 rad).
- Two adjacent arcs on the same circle at the same angular speed merge (center and radius within 1e-9 m, angular speed within 1e-9 rad per count), as long as the merged sweep stays below 360°.
- Write no rows for a route that is a single line ending at *B*, or that never moves.

---

## 6. Manifest

When at least one path exists, append `drill.paths.v1` to both lists, then recompute the size and SHA-256 of `show.sqlite` as usual:

```json
"capabilities" : [ "drill.core.v1", "timing.anchors.v1", "drill.paths.v1" ],
"requiredCapabilities" : [ "drill.core.v1", "drill.paths.v1" ],
```

If `media.audio.v1` is present, it goes before `drill.paths.v1`. With no paths, write schema 4 and the manifest exactly as today.

What readers do with the result:

- Current Fieldwork plays the paths.
- A Fieldwork build without paths support refuses schema 5 ("schema 5 is newer than this version supports"), or opens the file **read-only** because of the unknown required capability. Either way, it never saves the show with its curves flattened.

---

## 7. What Fieldwork rejects as damaged

Fieldwork does not repair a file that breaks any of these. It reports the file as damaged.

- A row whose transition or performer does not exist, or a path on the preset (the preset has no transition).
- A path for a performer who is not `placed` at both *A* and *B*.
- Ordinals that are not 0…n−1.
- `counts_num` or `counts_den` that is not an INTEGER, or not > 0.
- Durations that do not add up exactly to the move's length.
- A `hold` with a point or sweep, a `line` without a point or with a sweep, or an `arc` without a point or sweep.
- A point or sweep that is not finite, an arc with \|sweep\| ≥ 360 or a chord ≤ 1e-6 m.
- A path that ends more than 1e-6 m from the placement at *B*.
- Any `kind` other than `hold`, `line` or `arc`. It is never read as straight.

---

## 8. Example

These are the rows for a synthetic 8-count move, set 1 → set 2. The IDs are shortened.

| Performer | Set 1 | Set 2 | Motion |
|---|---|---|---|
| T1 | (−8, −4) | (−2, −4) | Waits 2 counts, then 6 even steps |
| T2 | (0, 10) | (10, 0) | A quarter circle, clockwise, around (0, 0) |
| T3 | (0, 0) | (4.5, 3.5) | East 4.5 m, then north 3.5 m; the corner is at count 4.5 |
| T4 | (10, 0) | (0, 10) | Three quarters of a circle in 6 counts, then arrives 2 counts early |
| T5 | (0, −6) | (4, −6) | Straight at even steps: **no rows** |

```
transition  performer ord kind  counts_num counts_den  x1    y1     sweep
TR-SET2     T1        0   hold  2          1           NULL  NULL   NULL
TR-SET2     T1        1   line  6          1           -2.0  -4.0   NULL
TR-SET2     T2        0   arc   8          1           10.0  0.0    90.0
TR-SET2     T3        0   line  9          2           4.5   0.0    NULL
TR-SET2     T3        1   line  7          2           4.5   3.5    NULL
TR-SET2     T4        0   arc   6          1           0.0   10.0   270.0
TR-SET2     T4        1   hold  2          1           NULL  NULL   NULL
```

Under schema 4, the converter would have added subsets at counts 2, 4, 5 and 6 for this move, plus one per count for the arcs. Under schema 5 the move stays one designer set with seven rows.

---

## 9. Checklist

- [ ] No `~n` derived subsets are written for Fieldwork output.
- [ ] Per-count positions are fitted into holds, lines, arcs and mid-count corners (§4), and every count is within 1 mm.
- [ ] Rows are canonical (§5); straight and still performers have no rows.
- [ ] Durations are INTEGER fractions that add up exactly to each move. Each path's end is the placement's exact doubles.
- [ ] Schema 5 and `drill.paths.v1` (in both lists) are written only when paths exist; otherwise schema 4 as before.
- [ ] `transition.model` stays `straight`.
- [ ] `fwtool info out.fieldwork` shows only the designer's sets and a `paths:` line. `fwtool evaluate out.fieldwork 0 1 2 …` matches the source within 1 mm at every count. Run this privately: real drills and their conversions never go into either repository.
