# Intermediate format (`DrillProject`, model version 1.0.0)

`3du-converter extract show.3du -o show.json` writes the canonical model as JSON. The same
structure is what every exporter consumes. It is defined in
`crates/three-du-core/src/model.rs`.

**Privacy:** this file contains the whole drill. Treat it like the `.3du` itself.

## Provenance

Values that are not plain source facts carry a `provenance` field:

| Value | Meaning |
|---|---|
| `source` | Read directly from a decoded record |
| `derived` | Computed exactly from source values by a documented rule |
| `inferred` | An evidence-backed assumption the file does not state |
| `user` | From a command-line override or page map |

## Top level

| Key | Contents |
|---|---|
| `model_version` | `"1.0.0"` |
| `metadata` | `title`, `description`, `source` (format, magic, revision, writer version, size, SHA-256, section byte ranges), `music` (`filename`, `embedded: false`), `note_labels` |
| `coordinates` | `source_unit` (`"yard"`), `transform` (below), provenance, human-readable `notes` |
| `field` | Decoded field records (`source`), sidelines, playing surface and full extents (source units), hashes, `yard_lines` with yard numbers, side/front/back labels |
| `performers[]` | `index`, `source_id` (16 hex digits), `label`, `section`, `symbol`, `kind` (`marcher` or `prop`), `size` (w, d, h), `shape_code`, `rotation_radians`, `color_rgb` |
| `timeline` | `count_times_seconds[]`, `tempos_bpm[]` (governing count c → c+1), `tempo_changes[]`, `drill_end_count`, `trailing_counts` |
| `sets[]` | `name`, `start_count`, `counts` (length of the move into it), `is_subset`, `rehearsal_mark`, `measures_text`, `measure_range`, `tab_type`, `color`, `straight_transition`, `nonlinear_performers`, `provenance` |
| `measures[]` | `number`, `start_count`, `rehearsal_mark`, `provenance` |
| `positions[]` | Per performer: `xy[count] = [x, y]` raw source coordinates (`f32` precision) or `null` |
| `annotations[]` | `count`, `set_index`, `type: text` (`rect`, `text`) or `type: polyline` (`points`) |
| `diagnostics[]` | `severity` (`info`/`warning`/`lossy`), stable `code`, `message` |
| `unknown_records[]` | Undecoded bytes with context and offset |

## Coordinates

Raw `positions` are never altered. Canonical coordinates come from `coordinates.transform`:

```
x_steps = x_sign · (x − center_x) · steps_per_unit_x      from the 50, + = audience right
y_steps = y_sign · (y − front_y)  · steps_per_unit_y      from the front sideline, + = back
meters  = steps · step_inches · 0.0254                    (Fieldwork re-centres y on midfield)
```

## Measures

A set's measure range covers the counts after the previous set that had a range. That gives
the first measure an exact start count (`derived`). Inner measure lines are placed only when
the counts divide evenly (`inferred`). Otherwise the range keeps only its first line.

A rehearsal mark on a set labels the measure that starts right after that set (`inferred`).
For example, the opening set's mark labels the first measure.
