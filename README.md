# 3du-converter

A free, open-source, cross-platform converter for Pyware/UDB **`.3du`** drill files. It reads
the binary file directly and writes:

| Output | Extension | Status |
|---|---|---|
| OpenMarch project | `.dots` | Opens and edits in OpenMarch desktop 0.1.7 |
| OpenMarch Universal Standard | `.om`, `.omz` | Valid under the official `@openmarch/schema` |
| Fieldwork show | `.fieldwork` | Opens in Fieldwork's own reader and evaluator; GUI check pending |
| MusicXML count track | `.musicxml` | Measures, meters, tempos, rehearsal marks and sets; imports in MuseScore 4 with exact timing |
| Intermediate JSON | `.json` | The canonical model, for debugging and other tools |

It runs as a command-line tool and as a native library (C ABI) for embedding in Fieldwork.
No Node, Python or other runtime is needed.

> **Unofficial.** This project is not affiliated with or endorsed by Pyware or UDBapp. It
> exists to support interoperability with drill files you have lawfully obtained.

## Install

You need Rust 1.75 or newer.

```sh
cargo build --release -p three-du-cli
# binary: target/release/3du-converter (3du-converter.exe on Windows)
```

## Use

```sh
3du-converter inspect show.3du                       # counts, sets, warnings (no names)
3du-converter inspect show.3du --reveal              # also title, set names, labels
3du-converter convert show.3du --format dots --output show.dots
3du-converter convert show.3du --format fieldwork --output show.fieldwork
3du-converter convert show.3du --format om  --output show.om
3du-converter convert show.3du --format omz --output show.omz
3du-converter convert show.3du --format musicxml --output show.musicxml --audio show.wav
3du-converter align show.3du --audio show.wav --click-check check.wav   # where the drill sits in a recording
3du-converter extract show.3du --output show.json
3du-converter validate show.dots                     # also .fieldwork, .om, .omz, .3du
```

| Option | Effect |
|---|---|
| `--overwrite` | Replace an existing output (otherwise refused) |
| `--strict` | Refuse any conversion that is not exact |
| `--allow-lossy` | Accept approximations (for example, sets inferred from motion) |
| `--preserve-curves` | Keep curved and follow-the-leader moves exact by adding derived subsets |
| `--page-map FILE` | Supply set boundaries yourself |
| `--flip-x`, `--flip-y`, `--steps-per-unit N` | Override the coordinate transform |
| `--audio FILE`, `--audio-offset S`, `--follow-audio`, `--click-check FILE` | Align with a `.wav` recording (MusicXML, `align`) |
| `--json` | Machine-readable result on stdout |
| `--verbose` / `--quiet` | More or less output |

Outputs are validated, written to a temporary file, and renamed into place only when valid.

Exit codes:

| Code | Meaning |
|---|---|
| 0 | OK |
| 1 | Failure |
| 2 | Usage error |
| 3 | Invalid or unsupported input |
| 4 | Refused by the loss policy |
| 5 | Output exists |
| 6 | Validation failed |
| 7 | I/O error |

To try it without a real drill:

```sh
cargo run -p three-du-core --example make_synthetic -- synthetic.3du
```

## File formats

| Extension | What it is |
|---|---|
| `.3du` | Pyware/UDB drill export: big-endian binary starting with `3DUD`. Holds sets, roster, per-count positions, timing, field and notes. **Input.** |
| `.3dudb` | A different UDB format. Detected and refused (not supported yet). |
| `.om` / `.omz` | OpenMarch Universal Standard: JSON, or gzip-compressed JSON, for exchanging shows. |
| `.dots` | OpenMarch desktop's project file: an SQLite database, not JSON. |
| `.fieldwork` | Fieldwork's show file: a ZIP with a manifest, an SQLite database and assets. |

## What is preserved

- **Performers**: stable IDs and labels.
- **Positions**: every performer at every set, at source precision, with field orientation.
- **Sets**: names, subsets and rehearsal marks.
- **Measures**: measure ranges.
- **Timing**: exact per-count timing and tempo.
- **Field**: the source's yard lines, hashes and step size.
- **Notes**: text notes (Fieldwork only).

Curved moves between sets are flattened unless `--preserve-curves` is used. See
[docs/FORMAT_LIMITATIONS.md](docs/FORMAT_LIMITATIONS.md).

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [`.3du` format findings](docs/3DU_FORMAT.md)
- [Intermediate model](docs/INTERMEDIATE_FORMAT.md)
- [OpenMarch mapping](docs/OPENMARCH.md)
- [Fieldwork mapping](docs/FIELDWORKS.md)
- [MusicXML export and audio alignment](docs/MUSICXML.md)
- [Embedding in Fieldwork (C ABI, Swift, XCFramework)](docs/INTEGRATION.md)
- [Limitations](docs/FORMAT_LIMITATIONS.md)
- [Privacy and testing with real files](docs/PRIVACY.md)

## Development

```sh
sh scripts/install-hooks.sh       # pre-commit guard against committing drill data
cargo test --workspace            # synthetic fixtures only
```

Never commit real drill files. See [docs/PRIVACY.md](docs/PRIVACY.md).

## License

Licensed under either the [Apache License 2.0](LICENSE) or the [MIT license](LICENSE-MIT), at
your option.

Third-party components:

- SQLite, bundled by `rusqlite`: public domain.
- Rust crates: MIT/Apache-2.0.

No OpenMarch (AGPL-3.0) code is included; see [docs/OPENMARCH.md](docs/OPENMARCH.md#licensing).
