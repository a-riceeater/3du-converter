# OpenMarch mapping

Two outputs: the **Universal Standard** (`.om` JSON / `.omz` gzip, schema `0.2.0`, from
<https://github.com/OpenMarch/schema>, Apache-2.0) and the native **`.dots`** project (SQLite).

## Validation status

| Check | Result |
|---|---|
| `.om`/`.omz` against the official `@openmarch/schema` 0.4.2 (`fromOpenMarchSchemaFile` + `safeValidateOpenMarchData`) | Valid, synthetic and real sample |
| `.dots` schema vs. the database OpenMarch's own 17 Drizzle migrations build (columns, types, defaults, keys, indexes, view) | Identical; opt-in test `schema_matches_openmarch_migrations` |
| `.dots` opened in **OpenMarch desktop 0.1.7** (Windows) | Opens with no errors or dialogs; page names, counts, subsets, measures, notes and positions shown correctly (real sample and synthetic) |
| Editing in OpenMarch | Selecting all marchers and nudging them moves every marcher exactly one step; change saved to the file with undo history; file reopens cleanly |

The desktop test drives the installed app over the Chrome DevTools Protocol
(`--remote-debugging-port`). Analytics must be opted out first: with analytics on,
OpenMarch records sessions, which would capture the drill on screen.

## Coordinates

| | `.om` | `.dots` |
|---|---|---|
| Unit | steps | canvas pixels, `pixelsPerStep = stepSizeInches × 0.5` (11.25 at 8-to-5) |
| x origin | the 50, + = audience right | left canvas edge; center at `width / 2` |
| y origin | front sideline (`yOrigin: "front"`), + = back | back canvas edge, + = toward front; front sideline at `height` |
| Field | `performanceArea` checkpoints in steps | `field_properties.json_data` (FieldProperties), y checkpoints negative toward back |

Field geometry comes from the source, not a template:

- the yard lines;
- end lines (so the canvas covers the end zones);
- sidelines and hashes;
- the step size from `GRID`;
- the minor-line interval from the source's minor lines.

The sample's field is 84 steps deep, Pyware's grid; the real field is 85.33 steps deep.

## Timeline

- **Beat 0 / page 0**: OpenMarch's zero-duration first beat holds the opening set.
- **Later pages**: page *i* starts on the beat after set *i − 1* and lasts until set *i*'s count.
- **Beat durations** come straight from the source clock, so the total time matches exactly.
- **`.om` `tempoSections`** start with `{tempo: 0, numberOfBeats: 1}`, as the Universal Standard README requires. Page and measure `startBeatIndex` values count that beat.

  Upstream discrepancy: OpenMarch's own `.dots → .om` exporter omits the zero section. Also,
  the package helper `tempoSectionsToBeats` divides by the zero tempo. We follow the
  normative README; consumers using that helper should skip a leading zero-tempo section.
- **Trailing counts** after the final set are not exported (they would lengthen the last page).

## Pages and names

- **Names**: OpenMarch generates page names itself. The converter sets `pageNumberOffset` to the
  first set's number, so source names such as `59 … 78, 78A, 79 …` reproduce exactly. When a
  name cannot be reproduced, the source name goes into the page notes and a warning is issued.
- **Subsets** map to `is_subset`.
- **Page notes** carry `Measures a-b` and `Rehearsal X`.

## Measures

`.om` measures carry explicit names, so all known measure lines are written.

OpenMarch `.dots` instead numbers measures sequentially from `measurementOffset`. Measure lines
are therefore written only when they cover every measure number. Otherwise (uneven ranges), they
are omitted and kept in page notes, to avoid misnumbering.

## Performers

- **Labels** split into `drill_prefix` + `drill_order` (`T12` → `T`, 12). Labels without a number,
  or duplicates, are numbered (`unlabeled` → `unlabeled1…`) and the source label is noted.
- **Section**: the source info field when present, otherwise `Other`, which OpenMarch accepts.
- **Props** are exported as marchers (OpenMarch has no props).
- **Names** are never written.

## Not representable

- **Curved and follow-the-leader moves**: OpenMarch 0.1.7 interpolates straight lines. Its
  `midsets` table exists but the animation code ignores it. Use `--preserve-curves` to add
  derived subsets that keep every count exact.
- **Text and line annotations.**
- **Audio**: the source has only a file name, so attach the music in OpenMarch.

## `.dots` database details

- `PRAGMA user_version = 7`, which OpenMarch requires before it migrates.
- `__drizzle_migrations` holds all 17 migration rows (hash, timestamp). OpenMarch then finds no
  pending migrations.
- Seed rows match a new OpenMarch file: `history_stats`, `utility` (id 0),
  `workspace_settings`, `field_properties`, and beat 0 / page 0.
- The five guard triggers are created. OpenMarch adds its undo triggers when it opens the file.

## Licensing

No OpenMarch code is bundled. OpenMarch is AGPL-3.0 (plus an internal-use license for
OpenMarch LLC). The `.dots` schema in `dots_schema.rs` is an independent statement of the
database layout, written for interoperability and checked against the migrations by test.
The migration hashes and timestamps are factual identifiers. The Universal Standard
(Apache-2.0) is followed by field name; none of its code is included.
