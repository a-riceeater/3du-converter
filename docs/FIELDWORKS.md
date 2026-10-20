# Fieldwork mapping (`.fieldwork`, format 1.0, database schema 4 or 5)

The exporter follows Fieldwork's format document (`fieldwork-file-schema.md`) and the reader
in Fieldwork's `packages/DrillDocument`. The extension is `.fieldwork`; `--format fieldworks`
is accepted as an alias.

## Validation status

| Check | Result |
|---|---|
| Built-in validator: same rules as Fieldwork's reader (ZIP subset, path rules, CRC, manifest hashes, unlisted members, schema version, invariants in §7, schema 5 path invariants) | Passes; also accepts a real Fieldwork-written example |
| Fieldwork's own `fwtool info` | Opens the real sample: all performers, sections, 45 sets at the correct counts, 120 measures, the source field |
| Fieldwork's own `fwtool info` with paths | The real sample: 45 designer sets, no derived subsets; `paths: 1743 performer paths with 3046 segments in 27 of 44 moves` |
| Fieldwork's own `fwtool evaluate`, compared with the source frames | Set positions within 2 µm, set times within 0.5 µs; every performer at every count (107,271 positions) within 1 mm, worst 0.99 mm, median 3 µm |
| Opening in the Fieldwork GUI | Not yet run (only the command-line tool was available) |

## Container

ZIP entries are stored uncompressed, with UTF-8 names (flag `0x0800`), version 20, the time
fixed to 1980-01-01, and no extra fields. The order is:

1. `manifest.json` (sorted keys, pretty-printed)
2. `show.sqlite` (serialized in memory; schema 5 when any performer follows a path, otherwise 4)
3. `extensions/org.3du-converter.props/props.json` (when the drill has props)
4. `extensions/org.3du-converter.provenance/provenance.json`

Both extensions are listed with `required: false`. Fieldwork keeps optional extensions
byte-for-byte on save. Capabilities are `drill.core.v1` and `timing.anchors.v1`; there is no
media. When paths are written, `drill.paths.v1` is added to both `capabilities` and
`requiredCapabilities`, so a Fieldwork build without paths opens the show read-only instead
of flattening it.

## Tables

| Table | Written as |
|---|---|
| `meta` | `writer = 3du-converter x.y.z`, `units = meters` |
| `show` | Title, `max_stride 0.762`, `min_clearance 0.5`, `counting_mode = counts` |
| `field` | Preset `custom`; the **source** field in meters (length to the end lines, width sideline to sideline, step 0.5715 m at 8-to-5) |
| `field_checkpoint` | Yard lines (`G`, `5` … `50`; tens are major, fives minor), end lines, sidelines, hashes (`hash`) |
| `section` | One per label prefix (`T`, `bd`, …); symbols and palette colors rotate |
| `performer` | Marchers only, in source order; labels made unique (`label (2)`); `name` NULL |
| `movement` | One, no audio, `preserveAudioPins` |
| `measure` | One per known measure start. The span until the next start becomes the meter `n/4`; `number` is the source measure number (a display marker in counts mode) |
| `timing_event` | One `beat` per count, `1/4`, `baseline_seconds` = the source clock's duration for that count |
| `arrival_set` | Each set at the cursor of its count (next event at fraction 0; the final count is the last event at fraction 1); `is_subset`, `rehearsal_mark`, `measure_marker` = source measure text |
| `transition` | `straight` into every set after the preset (the model for performers without a path) |
| `placement` | `placed`, x/y meters from center (+y back), z 0, facing 0 (the source stores 0 rotation for performers) |
| `field_note` | Source text annotations, top-left corner in meters, on their set (or the next set when between sets) |
| `path_segment` | Schema 5 only: the route of each performer whose move is not straight at even steps (see below) |

IDs are deterministic version-8 UUIDs derived from the source file hash and the source IDs, so
re-converting yields the same identities.

## Curved moves (paths)

The source stores every performer's position at every count. For each performer and move,
`paths.rs` fits those samples with holds, even-step lines and even-speed circular arcs,
following [FIELDWORK_SCHEMA5.md](FIELDWORK_SCHEMA5.md), a port of Fieldwork's
`paths_reference.py`:

1. Greedily take the longest hold, line or arc from the current count that stays within 1 mm
   of every sample (ties prefer hold, then line).
2. When only a 1-count chord fits between two legs, solve the mid-count corner and round it to
   a fraction with a denominator of at most 960.
3. Anything left becomes 1-count lines. Arc ends are sample points; the sweep comes from the
   samples. The last line or arc ends on the arrival placement's exact doubles.
4. Reduce to canonical form. Performers who move straight at even steps, or stand still, get
   no rows.

Every count is then evaluated with Fieldwork's path math. A fit that misses any count by more
than 1 mm is retried without corners, then as 1-count lines; if even that fails the export is
refused. Durations are exact `counts_num/counts_den` INTEGER pairs that add up to each move.
The plan reports `curves_as_paths` (info). Performers missing a position inside a curved move
cannot be fitted; they move straight and are reported as `curves_flattened`.

## Props

Fieldwork 1.0 has no props, so the source's props (objects with type 1, typically labeled
`unlabeled`) are **not** written as performers. They are kept in
`extensions/org.3du-converter.props/props.json`:

```json
{
  "layoutVersion": 1,
  "units": "meters",
  "sets": ["1", "2", "2A", "…"],
  "props": [{
    "sourceId": "…16 hex…", "label": "unlabeled",
    "sizeMeters": [4.572, 0.0, 4.572], "shapeCode": 2, "rotationRadians": 0.0,
    "colorRgb": [192, 192, 192],
    "placements": [{"x": -12.6, "y": 25.7}, "…one per set…"],
    "stationary": true
  }]
}
```

To support props natively later:

1. Add the tables to Fieldwork.
2. Replace `props::extension_member` in `crates/three-du-fieldwork/src/props.rs` with a writer
   for them. `props::collect` already returns every prop in Fieldwork units.

## Provenance extension

`provenance.json` records:

- the converter version;
- the source format, revision, writer version and SHA-256;
- the audio file name the source refers to;
- the coordinate notes;
- every conversion diagnostic.

## Losses

- **Curved moves** are kept as paths, within 1 mm at every count. `--preserve-curves` adds no
  subsets to Fieldwork output.
- **Drawn lines** (polyline annotations) are dropped.
- **Audio** is never embedded or invented. The source only names a file.
