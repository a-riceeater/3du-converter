# Architecture

```
            .3du bytes
                │  three-du-core::raw::decode      bounds-checked record reader
                ▼
            RawDrill                               every record, incl. unknown bytes
                │  three-du-core::convert          sets, measures, orientation, diagnostics
                ▼
          DrillProject  ──────────────► intermediate JSON (extract)
           │        │
           │        └── three-du-fieldwork ──► .fieldwork (ZIP + SQLite + extensions)
           └─────────── three-du-openmarch ──► .om / .omz (Universal Standard)
                                         └──► .dots (OpenMarch SQLite project)

  three-du-cli  (3du-converter)  ─┐
  three-du-ffi  (libthree_du)    ─┴─ both call the same functions above
```

| Crate | Responsibility | Depends on |
|---|---|---|
| `three-du-core` | Format detection, decoder, canonical model, coordinate transform, loss policy, atomic writes, synthetic fixture encoder | serde, sha2, thiserror |
| `three-du-openmarch` | `.om`/`.omz` builder and validator; `.dots` writer and validator | core, rusqlite (bundled SQLite), flate2 |
| `three-du-fieldwork` | `.fieldwork` archive: ZIP subset, manifest, schema-4 database, provenance and props extensions, validator | core, rusqlite, crc32fast, sha2 |
| `three-du-cli` | The `3du-converter` command | all of the above, clap |
| `three-du-ffi` | C ABI (`cdylib` + `staticlib`) and `include/three_du.h` | all of the above |

The core knows nothing about OpenMarch or Fieldwork. Exporters only read `DrillProject`.

## Data flow details

1. **Detect** by content (`detect.rs`): `3DUD`, SQLite, ZIP + Fieldwork manifest, gzip, JSON.
2. **Decode** (`raw.rs`): sequential, length-checked reads. Each count is validated against the
   bytes remaining, so a corrupt count cannot cause a large allocation. File size and total
   position samples are capped. A progress callback can cancel.
3. **Convert** (`convert.rs`):
   - match frame objects to the roster;
   - classify props;
   - build the timeline and sets (or apply a user page map);
   - test each move for straightness;
   - place measure markers;
   - derive the coordinate transform from the field records;
   - collect diagnostics.
4. **Export**:
   - each exporter computes its diagnostics first (`plan`) and enforces the loss policy before writing anything;
   - the output is built, validated in memory, written to a hidden temporary sibling, re-read and validated again;
   - only then is it renamed into place.

## Loss policy

Each diagnostic has a severity:

- `info`: nothing lost.
- `warning`: a representational loss that keeps the drill faithful at every set. For example, curves flattened between sets, or labels renumbered.
- `lossy`: an approximation of drill data. For example, set boundaries inferred from motion, or missing samples filled in.

How the CLI flags map to those severities:

| Flag | Refuses |
|---|---|
| `--strict` | warnings and lossy |
| default | lossy |
| `--allow-lossy` | nothing |

## Determinism

The same input always produces byte-identical output, because nothing depends on the clock:

- UUIDs are version-8 UUIDs derived from the source file's SHA-256 plus the record's source ID.
- Timestamps are fixed: 1970 in the database rows, 1980-01-01 in ZIP entries, and gzip `mtime = 0`.
- Iteration order follows the source.

Tests assert this.
