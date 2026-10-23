# MusicXML export and audio alignment

```sh
3du-converter convert show.3du --format musicxml --output show.musicxml
3du-converter convert show.3du --format musicxml --output show.musicxml --audio show.wav
3du-converter align show.3du --audio show.wav --click-check check.wav
```

## What the score contains

A MusicXML 4.0 partwise score with one percussion staff, `Drill counts`: one wood-block note
per count, from count 1 to the final set. Opened in notation software or a DAW, it acts as a
click track that carries the drill's structure:

| Drill data | In the score |
|---|---|
| Measure ranges of each set | Measures with the source's numbers. The meter (`n/4`) is the number of counts in the measure. |
| Per-count tempo | Metronome marks (quarter = BPM) at every tempo change |
| Rehearsal marks | `<rehearsal>` on the measure that starts right after the set |
| Sets and subsets | Text (`Set 60`, `Set 78A (subset)`) above the count where the set is hit |
| Counts within each move | Lyrics `1 … n`, restarting after every set |
| Source and alignment details | `<miscellaneous>` fields in `<identification>` |

Timing: count *c* sounds at the drill's `count_times[c]` and lasts `60 / tempo[c]`.
Measure numbers come from the source; the first measure of a range starts on the count after
the previous ranged set.

### Measures the source does not pin down

- **A missing measure number** (no set's range includes it) is filled in when that makes the
  counts divide evenly. It is reported as `measure_numbers_filled`.
- **A range whose counts don't divide evenly** among its measures (mixed meter) is split as
  evenly as possible, longer bars first. A `bar lines estimated` marking is printed and
  `bar_lines_estimated` is reported. Correct those bars in your notation program.

## Audio

MusicXML cannot hold audio. The score records the file name, the length and the alignment.
Its **lead-in bar** (one rest) lasts exactly from the start of the recording to the first
beat, so bar 1 lines up when the score and the recording start together in a DAW or notation
project.

Only `.wav` is read. That covers PCM 8/16/24/32-bit, float and extensible formats, including
recordings whose size field was never finalized.

### How alignment works

1. **Onsets.** Onset-strength envelopes are computed from the recording in ten frequency
   bands (spectral flux with 1024-sample windows and a 128-sample hop). A metronome or click
   in the background usually has a band to itself, so it is not drowned out by the band's
   attacks.
2. **Single offset.** The drill's beat times are slid along the envelopes in windows of 16
   beats. Each band and each window is normalized on its own: a quiet passage where only the
   metronome is heard counts as much as a loud one. Each window may sit up to 15 ms from the
   others, so a recording that drifts a little still has one clear offset. A window at one
   tempo fits every offset that is a whole number of its beats away from the true one.
   Because the drill changes tempo, only the true offset lines every section up at once. The
   result is the median of the windows' own best offsets near the peak. It is **confident**
   only when the best offset clearly beats its rivals and its beats hit onsets (three times
   their local average) well above chance. Only then is it applied automatically.
3. **Beat tracking** (`--follow-audio`). A dynamic-programming tracker places every drill beat
   in the recording. It knows the beat count and each beat's notated length, allowing 0.75×–1.33×.
   It listens mostly to the bands that follow the beats (the metronome, when there is one).
   Each beat is pulled toward the drill clock, because a band's off-beat pattern can be as
   loud as the click it plays to. The pull is tight (±0.25 s) when the offset is confident or
   given, and loose (±2.5 s) otherwise. Use beat tracking for performances that drift from
   the drill's metronome tempo. Playback tempos then follow the recording beat by beat, while
   the visible tempo marks still show the drill's tempos. Its confidence is measured against
   the same tracker run on the time-reversed recording, which gives the chance level.
4. **Verification.** `--click-check out.wav` writes the recording mixed with a click on every
   beat; set arrivals get a higher, accented click. Listen to it before trusting the result.

When alignment is not confident:

- count 0 is placed at the start of the recording (the only alignment the `.3du` implies);
- the estimate is reported;
- you choose `--audio-offset SECONDS` or `--follow-audio`.

Following an unconfirmed track is a lossy approximation and needs `--allow-lossy`.

## Validation

- Unit tests cover:
  - recovering known offsets from synthetic click tracks (22.05 and 48 kHz, early and late starts);
  - following a synthetic performance that slows and rushes (within 6 ms);
  - a quiet metronome deciding the offset against a loud band that has drifted 25 ms, where a
    one-beat-late offset fits the loud section equally well (the previous broadband method
    chose the late one);
  - rejecting pure noise.
- **MuseScore 4** imports the scores. Its MIDI export matches the intended onset of every
  count within 0.03 ms on the drill clock and within 0.01 ms when following a recording.
- Playback tempo is written into every metronome mark's `<sound>`. MuseScore plays a mark's
  number unless told otherwise, which would override a separate tempo at the same position.

## Results on the real sample

The recording is dense with attacks and has a quiet metronome in the background (around
12 kHz). Measured directly, the click sits at +0.125 s against the drill clock for counts
1–175 (spread 1–2 ms) and at +0.153 s through the 160 BPM section. The band runs about
0.18 s behind the clock through the slow 84/80 BPM passage, which has no audible click, and
catches up during the 152 BPM counts before the 160 BPM section.

| Measurement | Broadband method | Band-wise method |
|---|---|---|
| Single offset: best estimate | +0.53 s (one beat late in both fast sections) | **+0.153 s, confident** |
| Single offset: peak ratio | 1.05 | 1.32 |
| Single offset: beat hits | 65% (chance: 46%) | 48% (chance: 23%) |
| Beat tracking: beats in the 160 BPM section more than 30 ms off the click | most (hops between beats) | 1 of 177 |

A single offset is within 30 ms of the click everywhere except the slow passage, where it is
a sixteenth note early. Beat tracking follows that passage too. Its confidence still cannot be
shown against chance: in this band's sound there is an attack in the click band four times a
beat, so even a track of the reversed recording finds onsets. Check it with `--click-check`
and pass `--allow-lossy`.
