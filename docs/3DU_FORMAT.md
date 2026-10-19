# The `.3du` drill format (`3DUD`)

Findings from reverse-engineering, verified against a private real-world file written by
version **11.3.6** (format revision 12) and against an earlier independent extraction of the
same file. No real drill content appears in this document.

Confidence levels:

- **Verified**: decoded for every record of the sample, cross-checked against independent data,
  and the whole file parses with no bytes left over.
- **Strong**: consistent across every record, meaning inferred from behavior.
- **Unknown**: byte layout known, meaning not; preserved verbatim in `unknown_records`.

## Encoding

- Big-endian throughout.
- **String**: `u16` byte length, then that many bytes. UTF-8 when valid, otherwise read as
  Latin-1/Windows-1252. No terminator.
- **Count**: `u16` element count before every list. This caps lists at 65,535 entries.
- No compression, checksums or offsets: the file is one sequential stream of sections.
- IDs are 8 opaque bytes, stable across frames (shown as 16 hex digits).

The decoder reads each record where the previous one ended. Any mismatch fails at an exact
byte offset; nothing is located by searching.

## Layout

| # | Section | Layout | Confidence |
|---|---|---|---|
| 1 | Header | `"3DUD"`, `u16 revision`, `u16 major`, `u16 minor`, `u16 patch`, string version (e.g. `11.3.6`), string title, `u32` reserved (0), 5 × string note captions | Verified |
| 2 | Sets | count × set record (below) | Verified |
| 3 | Performers | count × { `id[8]`, string label, string info (empty in sample), string symbol (`X`) } | Verified |
| 4 | Audio | string file name (e.g. `….ogg`). **No audio bytes follow** anywhere in the file | Verified |
| 5 | Count times | count × `f64` seconds since count 0 | Verified |
| 6 | Tempos | count × `u32` BPM | Verified |
| 7 | Field | count × string record (below) | Verified layout |
| 8 | Frames | count × frame (below); one frame per count | Verified |
| 9 | Trailer | remaining bytes (33 in the sample: contains `"Main"`, `"N;-1;-1"`) | Unknown |

The section boundaries are reported by `3du-converter extract` under
`metadata.source.sections`.

### Set record (drill tab)

| Field | Type | Meaning | Confidence |
|---|---|---|---|
| start count | `u16` | Count at which the set's formation is reached | Verified (matches movement breakpoints) |
| id | `[8]` | Stable tab identifier | Verified |
| rehearsal mark | string | e.g. `Q`, `AA`, `END`; sometimes whitespace | Verified |
| name | string | Set label, e.g. `60`, `78A` | Verified |
| measures | string | Measure range of the move **into** this set: `"175-176"`, `"177"`, or empty | Strong |
| reserved | `[10]` | Zero in sample | Unknown |
| tab type | string | `StandardTab` (set) or `SubTab` (subset) | Verified |
| reserved | `u32` | Zero | Unknown |
| color | string | `"R,G,B"` or `"."` (default) | Strong |
| reserved | `u16` | Zero | Unknown |

Subsets have no measure text of their own. The next set's measure range covers the span
since the last set that had one.

### Frame (one per count)

```
u16 object_count
  object × object_count
u16 annotation_count
  annotation × annotation_count
```

**Object** (33 bytes, or 47 bytes with a size block):

| Field | Type | Meaning | Confidence |
|---|---|---|---|
| id | `[8]` | Performer id from section 3 | Verified |
| x, y | `f32`, `f32` | Position in yards (see coordinates) | Verified |
| color | 3 × `u16` | RGB 0–255 | Strong |
| symbol | `u16` | Character code (`'X'`) | Strong |
| state | `u16` | 0; `0xFF7F` on some props in some frames | Unknown |
| kind | `u16` | 1 = prop/scenery, 2 = performer, 7 = performer drawn with an alternate marker | Strong |
| rotation | `f32` | Radians (π/2 seen on props) | Strong |
| has size | `u8` | 0 or 1 | Verified |
| size | 3 × `f32` + `u16` | Width, depth, height (yards) and a shape code | Strong |

Objects in a frame are sorted by id. The sample stores every performer in every frame.

**Annotation**: `u16 type`, `id[8]`, `u16` reserved, then:

- type 2, **text box**: `rect` 4 × `f32` (x1, y1, x2, y2), 3 × `f32` (`0, 0, 1` seen), string text.
- type 1, **line**: 43-byte style block (contains an `f32` and a color byte), `u16` point count, points as `f32` pairs.

Annotations appear on the frames of set start counts, and occasionally in between. Unknown
annotation types stop decoding with an error, because their length cannot be known.

## Timing

- `count_times[c]` is the absolute time of count `c`; count 0 is time 0.
- `tempos[c]` governs the interval from count `c` to `c + 1`, so
  `count_times[c+1] − count_times[c] = 60 / tempos[c]`. This holds for every interval in the sample.
- The sample stores twice as many frames as the drill uses: nothing moves after the final
  (`END`) set, and every later frame is static. Exports end at the final set and report the
  trailing counts.

## Coordinates and field

Field records are space-separated text:

| Record | Meaning |
|---|---|
| `UNIT 0 0` | Unit code; `0 0` seen with yard fields |
| `GRID sx ux sy uy …` | `sx` steps per `ux` units horizontally (8 per 5 = 8-to-5) |
| `BORD x1 y1 x2 y2 r g b` | Playing surface: goal line to goal line, sideline to sideline |
| `VTMJ x …` / `VTMN x` | Major / minor vertical lines (yard lines; minor every 2.5 yd, end-zone lines) |
| `HZMJ y …` / `HZMN y` | Major / minor horizontal lines (sidelines; minor grid) |
| `HZHS y …` | Hash lines |
| `LEFT`, `RIGHT`, `TOP`, `BOTTOM`, … | Captions: `Side_1`, `Side_2`, `Back`, `Front`, … |
| `MARK`, `MRK2` | Unknown; preserved |

Conventions:

- **Units**: yards, with the origin at the field center. Yard lines are at x = ±5k, goal lines at ±50,
  and the field is 52.5 yd deep (84 steps at 8-to-5), Pyware's approximation of a high-school field.
  This is strong evidence.
- **x**: increases toward the audience's right. `LEFT` is captioned Side 1. Not yet confirmed
  against a real performance.
- **y**: screen-style. It grows toward the `BOTTOM` caption, which is the **front** sideline in
  the sample. This was confirmed by the owner of the real drill. (An earlier version of this
  converter assumed the opposite and mirrored drills front to back.)
- **Hashes** at ±8.75 yd are 28 steps from each sideline, the high-school hash.

Users can override all of this: `--flip-x`, `--flip-y`, `--steps-per-unit`.

## Version support

Only format revision 12 is accepted; other revisions fail with `unsupported_version` rather
than producing misaligned data. Files from other versions are needed to widen support.
`.3dudb` files are a different format and are detected and refused.
