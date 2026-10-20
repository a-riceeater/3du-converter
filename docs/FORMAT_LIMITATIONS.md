# Known limitations and conversion losses

## Source decoding

| Area | Status |
|---|---|
| Format revisions | Only revision 12 (writer 11.3.x) is verified; others are refused with `unsupported_version` |
| `.3dudb` | Different format; detected and refused |
| Set reserved bytes (10 + 4 + 2) | Zero in every sample; meaning unknown, preserved if non-zero |
| Object `state` word, `kind` 7 | Meaning unknown (alternate marker style); positions unaffected; reported |
| Prop shape code, rotation direction | Recorded, not interpreted |
| Line annotation style block | Preserved raw; not exported |
| Trailer after frames | Preserved raw; meaning unknown |
| Performer "info" string | Empty in samples; used as the section when present |
| Facing | Not stored for performers (rotation 0); exported as facing the front |
| Side-to-side orientation | Side 1 is read as audience left; not yet confirmed against a real performance (use `--flip-x` if mirrored) |

## Timing

- **Meters**: the source has no meters or beat grouping, only counts, per-count tempo and
  per-set measure ranges.
  - Measure lines inside a range are placed only when the counts divide evenly; otherwise only
    the first measure of the range is known.
  - Fieldwork gets one measure per known start, so an uneven range becomes one long measure
    with the right number.
  - OpenMarch `.dots` gets measures only when every measure is known.
- **Trailing counts**: counts stored after the final set (the sample stores twice as many as
  used) are not exported. Movement during them would be reported as lossy.
- **Audio**: the file only names the audio. Nothing is embedded or invented.
- **Audio sync**: the file has no offset between count 0 and its audio. Automatic alignment
  is applied only when confident. Recordings dense with attacks, such as live band
  recordings, often cannot be confirmed; verify with `--click-check`. Only `.wav` is read.
- **MusicXML meters**: the counts in each measure become an `n/4` meter. Ranges that don't
  divide evenly get estimated bar lines, which are marked in the score.

## Movement between sets

The source stores positions at every count. Fieldwork output keeps curved, follow-the-leader
and stop-and-go moves as per-performer paths (schema 5, `drill.paths.v1`): every count is
reproduced within 1 mm without adding sets (see [FIELDWORKS.md](FIELDWORKS.md)). Fieldwork
builds without path support open such files read-only.

OpenMarch interpolates straight, even-step lines between pages, so these moves are flattened
by default. That is reported as `curves_flattened`, and `--strict` refuses it.
- In the sample, 27 of 44 moves are non-straight. Mid-move deviation reaches several meters.
- `--preserve-curves` inserts derived subsets at each count where a performer changes
  direction or speed. The result reproduces every stored count, but adds subsets the designer
  did not write; they are labeled `<set>~n`. Fieldwork output does not need it and ignores it.

## Targets

| Target | Not representable |
|---|---|
| OpenMarch | Annotations; props are ordinary marchers; labels must be prefix+number (renumbered when not); measure numbers are sequential; field depth is the source's (84 steps) |
| Fieldwork | Props (kept in an extension); line annotations; duplicate labels are suffixed |
| Universal Standard | Annotations, props, page notes |
| MusicXML | Positions (it holds timing only); audio is referenced, not embedded |

## Without a set table

If a file has no set table, exports that need pages refuse to run unless:
- `--page-map map.json` supplies boundaries (`[{"name": "1", "start_count": 0}, …]`), or
- `--allow-lossy` infers them from movement breakpoints, reported as `sets_inferred_from_motion`.

Those inferred sets are not the designer's pages.
