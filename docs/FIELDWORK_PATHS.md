# Fieldwork per-performer paths: a brief for the Fieldwork agent

**How to use this file:** paste everything below the line into the agent working in the
Fieldwork repository. It is self-contained. Section 8 is the contract the 3du-converter will
follow when it writes paths, so keep it in sync with the Fieldwork implementation.

---

## 1. Task

Add **per-performer paths** to Fieldwork. A path is the route one performer takes during one
move (transition): holds, straight legs and circular arcs, each lasting an exact number of
counts. With paths, a drill imported from Pyware `.3du` reproduces every performer at every
count **without** inventing subsets.

Today, `transition.model` is always `straight`. The 3du-converter can only keep curved moves
exact by adding derived subsets (`60~1`, `60~2`, …) at every count where anyone turns or
changes speed. On the real sample, that adds **170 subsets** to a 45-set show (about 44,000
extra placement rows). Designers did not write those pages, and editing them breaks the curves.

This is the "custom paths / midsets / hold phases" item from M2 in `DESIGN.md`. It fits the
`Transition` and `Path` rows in `ARCHITECTURE.md` ("line/Bezier/composite geometry, waypoint
cursors, hold/move segments"). Build the storage and evaluation (P0) first, then editing (P1).
Generators come later (P2).

Read `AGENTS.md`, `ARCHITECTURE.md`, `DESIGN.md`, `docs/fieldwork-file-schema.md` and
`spec/fieldwork-format.md` before starting. Follow the repo's git rules.

## 2. What real `.3du` drills contain (evidence)

The `.3du` stores every performer's position at **every count**. The converter classified
every performer's motion in every move of a real 261-marcher, 45-set show (44 moves,
11,484 performer-moves). Only aggregate numbers are given here.

| Motion in one move, for one performer | Performer-moves | Representable as |
|---|---:|---|
| Straight at even steps, or standing still | 9,741 | Nothing new (today's `straight`) |
| One circular arc at even steps | 709 | 1 `arc` segment |
| Delayed start or early arrival (hold + straight leg) | 693 | `hold` + `line` |
| A chain of holds, legs and arcs, joints on whole counts | 273 | 2–7 segments |
| Chain with a corner that falls **between** counts | 45 | Segments with fractional durations |
| Not a line or circle (a smooth curve, sampled per count) | 23 | Chain of 1-count `line` segments |

Findings that shape the design:

- **Three primitives cover everything:** hold, straight leg at even steps, and circular arc at
  even angular speed. Rebuilt from segment parameters alone, every count matches the source
  within **1.1 mm**. The single arcs still match within 0.3 mm, so they are true circles.
- **No easing.** Within a segment, speed is always constant. When speed changes, a new
  segment starts. No Bézier or acceleration curve is needed to import.
- **Joints can fall mid-count.** In 19 checked corners, the corner sits at a fraction of a
  count, for example 4.603 counts into the move, and each leg keeps its own pace.
  Segment durations must therefore be **exact rationals**, not whole counts.
- **Holds** appear only at the start (300) or end (482) of a move, never in the middle. Still,
  allow them anywhere.
- **Pace:** of the 289 paths with two or more moving segments, 149 keep one step size
  throughout and 140 change pace between segments. Both cases must be stored.
- **Arcs:** sweeps range from 1.4° to 240°, with 100 sweeps over 180° and none of a full
  360°. Radii range from 0.74 to 171 steps (median 6). There are 573 clockwise and 405
  counter-clockwise.
- **Follow-the-leader** (30 performer-moves, each 5 counts behind its leader) and **block
  rotations** (groups of 8–13 performers on a shared center at the same angular speed) appear.
  Both reduce to explicit per-performer lines and arcs. They do not need their own storage
  model, only optional authoring tools (P2).
- The largest deviation from a straight line mid-move is 8.75 steps (about 5 m). Flattening is
  not a cosmetic loss.

## 3. Feature list

### P0: storage, evaluation, analysis (needed for lossless import)

1. **Model:** `PerformerPath` (ordered segments) on `Transition`, keyed by performer.
   Performers without a path move straight, as today.
2. **Schema 5:** a `path_segment` table, a migration from 4 to 5, and the
   `drill.paths.v1` capability.
3. **Evaluator:** `SceneEvaluator` follows a performer's path when it has one. `isMoving`
   is false during a hold.
4. **Validation:** the invariants in §6.
5. **Analysis:** stride uses each segment's pace, not the endpoint chord. Close passes work
   on paths.
6. **Commands keep paths consistent** through every existing edit (see §5).
7. **fwtool:** `evaluate` uses paths automatically; `info` reports path counts.
8. **Docs:** update `docs/fieldwork-file-schema.md` (§6.2, §6.6, §7, §8, §10) and
   `spec/fieldwork-format.md`.

### P1: seeing and editing paths

1. **Draw paths** for selected performers (an option shows all), with a tick at each count,
   hold markers and a segment-joint handle. Use dashed warning paths per `DESIGN.md`.
2. **Hold phases:** "Start late by n counts" and "Arrive early by n counts", for a selection,
   in the transition inspector (`DESIGN.md`: "Transition: duration, path, waypoints, stepping
   policy, hold phases").
3. **Waypoints:** add a waypoint at count *u* to split a segment, drag a waypoint, delete a
   waypoint (merging two segments).
4. **Curve a leg:** drag a leg's midpoint handle to turn it into an arc (sweep from sagitta).
   Drag back to straight within snap.
5. **Pace policy** per path: "even pace along the whole path" redistributes the moving
   segments' durations by length. "Keep segment timings" leaves them as they are. Real drills
   use both about equally.
6. **Straighten:** clear the path (back to straight) for a selection.
7. Inspector shows per-segment counts, length and step size (`n to 5`).

### P2: generators (author new drill, output plain segments)

- Follow-the-leader: pick a leader path and an interval in counts. Followers get explicit
  segments.
- Rotate a block about a pivot by an angle: each performer gets an arc with the shared center.
- Optional later `cubic` segment kind for authoring. It is not needed for import (§2), so
  keep the schema open for it but do not block P0 on it.

## 4. Data model and file format

### Swift model (`DrillCore`)

```swift
/// One piece of a performer's route through a move. Durations are in count pulses,
/// the same parameter the straight model uses, so tempo never bends a path.
public enum PathSegment: Hashable, Codable, Sendable {
    /// Stand still (mark time) at the current point.
    case hold(counts: Rational)
    /// Straight leg at constant speed to `to`.
    case line(counts: Rational, to: Point2)
    /// Circular arc at constant speed to `to`, turning `sweep` degrees.
    /// Positive = clockwise seen from above, which bulges to the left of start→end,
    /// the same side as a positive `form.bulge`. 0 < |sweep| < 360.
    case arc(counts: Rational, to: Point2, sweep: Double)
}

public struct PerformerPath: Hashable, Codable, Sendable {
    public var segments: [PathSegment]
}

public struct Transition {               // existing
    ...
    /// Performers listed here follow their path; everyone else moves straight.
    public var paths: [PerformerID: PerformerPath] = [:]
}
```

- A path's **start** is implicit: the performer's placement at the previous set.
- Its **end** is the end of the last `line`/`arc` (or the start, if there are none). It must be
  the performer's placement at `toSetID`.
- `Transition` must decode **without** a `paths` key (use `decodeIfPresent`, default `[:]`).
  Recovery journals and fixtures written before this change still replay.
- `transition.model` stays `straight`. It is the model for performers without a path. Do not
  add a new model value, because old readers map unknown values to `straight` and would
  silently flatten.

### Schema 5 (`ShowDatabase`)

```sql
-- Schema 5
CREATE TABLE path_segment (
    transition_id TEXT NOT NULL REFERENCES transition(id),
    performer_id  TEXT NOT NULL REFERENCES performer(id),
    ordinal       INTEGER NOT NULL,                 -- 0-based within (transition, performer)
    kind          TEXT NOT NULL CHECK (kind IN ('hold','line','arc')),
    counts_num    INTEGER NOT NULL,                 -- duration in count pulses, exact, > 0
    counts_den    INTEGER NOT NULL,                 -- > 0
    x1            REAL,                             -- end point, meters; NULL for hold
    y1            REAL,
    sweep         REAL,                             -- degrees, arc only; NULL otherwise
    PRIMARY KEY (transition_id, performer_id, ordinal));
PRAGMA user_version = 5;
```

- **Writer:** inserts rows ordered by arrival-set ordinal, then performer ordinal, then segment
  ordinal. It only writes canonical paths (§6), so the same show encodes to the same bytes.
- **Reader:** `SELECT … ORDER BY transition_id, performer_id, ordinal`. It rebuilds each
  path, rejects any `kind` it does not know (this is drill data, so it must not fall back to
  `straight`), and validates.
- **Migration 4 → 5:** create the empty table. Old files open with no paths, and the next
  save writes schema 5.
- **Manifest:** when any path exists, add `drill.paths.v1` to both `capabilities` and
  `requiredCapabilities`. A reader that does not understand paths then opens the file
  read-only instead of editing it and dropping the routes. Add `drill.paths.v1` to
  `FormatIdentity`'s known capabilities.
- A file without paths may still be written as schema 5 by Fieldwork. The converter writes
  schema 4 when it has no paths (§8).

## 5. Semantics

### Evaluation

For a move from set *k* (pulse `p_k`) to set *k+1*, a performer with a path is evaluated at
`u = pulse − p_k` counts:

1. Walk the segments, accumulating durations, to the segment containing `u`. A boundary
   belongs to the **later** segment, as with cursors. Clamp `u` to `[0, total]`.
2. Let `s = (u − segmentStart) / segmentCounts` ∈ [0, 1], and let `P0` be the segment's start
   point (the previous segment's end, or the from-placement).
   - `hold`: `P0`.
   - `line`: `P0 + (P1 − P0)·s`.
   - `arc` with sweep θ (radians, + = clockwise):
     ```
     c = P1 − P0,  L = |c|,  M = (P0 + P1)/2,  n = (−c.y, c.x)/L      // left normal
     h = (L/2) / tan(θ/2)
     C = M − n·h,  r = |P0 − C|,  φ0 = atan2(P0.y − C.y, P0.x − C.x)
     P(s) = C + r·(cos(φ0 − θ·s), sin(φ0 − θ·s))
     ```
     Return exactly `P1` at `s = 1`, so floating-point error cannot drift a joint.
3. `z` interpolates linearly over the whole move, as today. Facing keeps today's interpolation
   (travel-direction facing is out of scope).
4. `isMoving` is true unless the active segment is a `hold`.

Do the same in every consumer: macOS and Windows canvases (through `SceneSnapshot`), the
bridge JSON scenes, playback and `makeSplitMove`.

### Analysis (`DrillAnalyzer`)

- **Stride:** for each performer, the largest segment pace: `line` length / counts, `arc`
  `r·|θ|` / counts, holds excluded. A delayed start therefore correctly reports the faster
  leg. Report the count range of the offending segment.
- **Close passes:** split the move at the union of both performers' segment boundaries. On
  sub-intervals where both are lines or holds, keep the exact analytic minimum. Where an arc is
  involved, sample adaptively (at least 8 samples per count, then refine the minimum) and mark
  the result as sampled, per `ARCHITECTURE.md`. The broad phase uses each path's bounding box
  (an arc's box includes any axis extremes it crosses).
- **Bounds:** check path extremes (arc bulges), not only set positions.

### Commands that must keep paths valid

Every command below is atomic and undoable. Each should get a test.

| Command | Path behavior |
|---|---|
| `setPaths(TransitionID, [PerformerID: PerformerPath?])` (new) | Set or clear paths. `nil` clears (straight). |
| `setPlacements` (and form apply/edit, nudge, coordinate entry) at set *S* | For the move **into** *S*, move the last non-hold segment's end to the new placement (an arc keeps its sweep). For the move **out of** *S*, nothing is stored: the start is implicit. If the result is degenerate (zero-length arc), turn it into a line. |
| `makeSplitMove` at count *u* | The subset's placement is the path position at *u*. Each path is cut at *u*: a line splits at the point, an arc splits its sweep in proportion to *s*, a hold splits its counts. The left part goes to the new transition, the right part stays. Playback must be bit-for-bit unchanged at every count. |
| `deleteSet` *S* (merging A→S and S→B) | For each performer, concatenate (A→S path, or one straight line) with (S→B path, or one straight line). Normalize; if the result is one straight line, store no path. The deleted set's position becomes a waypoint, so playback is unchanged. |
| `moveSetCursors` and change-move-counts | When a move's length changes from *T* to *T′*, scale every segment's counts by *T′/T* (exact `Rational`). Offer "keep holds fixed" later. |
| `removePerformers` | Delete their paths. |
| A placement becomes `unresolved` or `inactive` at either end | Delete that performer's path for the affected moves. |
| `createSet` (duplicate or new) | The new move has no paths. |

### Canonical form (encoder normalizes; editors produce it)

- No path at all if the performer's route is one straight line over the whole move, or a single
  hold (start = end).
- No zero-length `line` (it becomes a `hold`). Adjacent holds are merged.
- Adjacent lines in the same direction at the same pace are merged, and so are adjacent arcs on
  the same circle at the same angular pace (tolerance 1e-9 m and 1e-9 rad).
- An arc whose sagitta is below 1e-6 m becomes a line.
- Full loops are stored as two arcs, since `|sweep| < 360`.

## 6. Invariants (add to `Validation.swift` and the schema doc §7)

For each path on transition *t* (into set *B*, from the previous set *A* in the movement), for
performer *p*:

1. *t* exists and is not the preset's (the preset has no transition anyway). *p* exists.
2. *p* is `placed` at both *A* and *B*.
3. There is at least one segment. Ordinals are 0…n−1 with no gaps or duplicates.
4. Every `counts` is greater than 0. The sum equals the move's length in pulses
   **exactly** (`Rational`), that is `pulse(B) − pulse(A)`.
5. `line` and `arc` have finite `x1, y1`. `hold` has NULL point and sweep, and `line` has a
   NULL sweep.
6. An `arc` has a finite sweep with 0 < |sweep| < 360, and a chord longer than 1e-6 m.
7. The path's end equals *p*'s placement at *B* within 1e-6 m. Writers store the
   placement's exact doubles.
8. `counts_den` > 0 and both parts fit in `Int64`. Reject anything else as corrupt.

## 7. Tests (in addition to the command tests in §5)

Evaluator vectors (meters, counts). Expected values are exact to 1e-6.

| # | From | Segments | Expect |
|---|---|---|---|
| 1 | (0, 0) | hold 4, line 4 → (4, 0) | u=2 (0,0); u=4 (0,0); u=6 (2,0); u=8 (4,0); stride 1 m/count over counts 4–8 |
| 2 | (0, 10) | arc 4 → (10, 0), sweep +90 | center (0,0); u=1 (3.826834, 9.238795); u=2 (7.071068, 7.071068); u=4 (10, 0); length 15.707963 |
| 3 | (10, 0) | arc 4 → (0, 10), sweep −90 | center (0,0); u=2 (7.071068, 7.071068) |
| 4 | (10, 0) | arc 3 → (0, 10), sweep +270 | u=1 (0, −10); u=2 (−10, 0); u=3 (0, 10) |
| 5 | (0, 0) | line 9/2 → (4.5, 0), line 7/2 → (4.5, 3.5) | u=4 (4, 0); u=5 (4.5, 0.5); u=8 (4.5, 3.5) (mid-count corner) |
| 6 | Vector 2 split at u=1 | | left: arc 1 → (3.826834, 9.238795), sweep +22.5; right: arc 3 → (10, 0), sweep +67.5; all counts unchanged |
| 7 | Vector 1, move stretched 8 → 16 | | hold 8, line 8 → (4, 0) |
| 8 | Delete the middle set of a straight A→S→B with a corner at S | | one path with 2 lines; counts unchanged |

Also test:

- validation rejections, one per invariant;
- 4 → 5 migration (old fixtures open unchanged);
- encode → decode identity and byte-identical re-encode;
- a pre-paths recovery journal replays;
- analysis stride and close passes on arcs;
- the capability makes a file read-only in a reader built without `drill.paths.v1`
  (simulate it).

Add an original fixture, `spec/fixtures/paths.fieldwork` (synthetic only, never real drill),
using every segment kind, with `fwtool evaluate` golden output.

**Acceptance:** the converter (§8) writes a `.fieldwork` from a real drill. `fwtool info`
shows the designer's 45 sets and **no** `~n` subsets. `fwtool evaluate` at every integer
count matches the source positions within 1 mm. The converter's owner runs this privately.
Real drills and their conversions must never enter either repository.

## 8. Converter contract (what 3du-converter will write)

So both sides agree:

- **Fitting**, per performer per move, from the per-count samples:
  1. Greedily take the longest run that is a hold, an even-step line, or an even-speed
     circular arc, within 1 mm.
  2. Solve a 1-count chord between two legs as a mid-count corner when the legs' paces
     reach the corner point exactly within that count. The fractional duration is
     rationalized with a denominator of at most 960.
  3. Anything left becomes 1-count lines.
  4. Arc ends are the sample points, and the sweep comes from the samples.
- Writes **schema 5** with `path_segment` rows and `drill.paths.v1` only when at least one
  path exists. Otherwise it writes schema 4, as today.
- Rows are written only for performers whose move is not straight (1,743 of 11,484 in the
  sample, about 3,100 segment rows). Paths are in canonical form.
- Before writing, the converter evaluates every count with the §5 math and refuses its own
  output if any count is off by more than 1 mm.
- The `--preserve-curves` derived subsets are no longer used for Fieldwork, and
  `curves_flattened` is no longer reported for it. OpenMarch targets keep subsets, because
  OpenMarch 0.1.7 ignores its midsets.
- Text notes and props are unchanged.

## 9. Out of scope

Facing along the path, easing or acceleration, cubic Bézier import, traditional maneuvers
(counter-march, flank and to-the-rear as named moves; they import as segments), 3D and
OpenMarch midset export.
