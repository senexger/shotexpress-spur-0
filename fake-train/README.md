# fake-train

A virtual ShotExpress train with a ratatui TUI. Full MQTT drop-in for the real
ESP32: it subscribes to `shotexpress/command`, understands both the JSON
protocol (`drive`, `steps`, `stop`) and the raw firmware integers
(`0`/`1`/`2`/`3`), and publishes status heartbeats + exec events — so the
whole chain (dashboard → server → train) can be debugged without hardware.

## Run

```sh
cargo run -p fake-train    # reads MQTT_URL/MQTT_USERNAME/MQTT_PASSWORD from the root .env
```

Needs a terminal of at least 104x22.

Keys: `q` quit · `space` pause (heartbeats keep running) · `e` toggle shots
loaded/empty · `r` teleport to a random position (resets the belief) ·
`s` force the wifi signal off/on · `+`/`-` simulation speed (0.25x–8x).

The train also loses signal randomly (more often near the turnaround loops,
which sit far from the access point). While offline it keeps driving: exec
events are queued and flushed on reconnect (the server dedupes by `msg_id`),
statuses are dropped, and incoming commands are held back and delivered late —
stale ones then die on their TTL.

## The track

A main line with a passive turnaround loop at each end (dogbone), flattened
into a ring of 14 tag readings (`fake-train/src/track.rs`). The train only
ever runs forward — the switches route every lap the same way, so reaching a
station "behind" the train means continuing through the next turnaround.
Main-line tags (`station1`, `track12`, `station2`, `track23`, `station3`) are
read twice per lap (once per pass direction), curve tags (`curveA1/2`,
`curveB1/2`) once. Speeds are not the train's decision: each drive leg carries
its own speed, so slowing through a turnaround is just the server respecting
the curve nodes' `speed_limit` in `track.toml`.

## Localization

The simulation always knows the true position, but the *train's belief* is
computed only from tag readings + motor direction (`belief.rs`):
random spawn → unknown; one main-line tag → two candidate positions; a second
consecutive tag (or a single unique curve tag) → localized. The TUI shows
ground truth and belief side by side; `r` lets you watch relocalization.

## Command semantics

`drive` is an ordered list of legs ("hold `speed` until you read `tag`"),
compiled by the server from `track.toml` — the train holds no map and can
finish any single command with zero connectivity. The last leg's tag is the
stop tag. Lifecycle: accepted → progress per read tag (last_tag + odometer
distance, plus `skipped` when the reader missed tags and the train resynced
forward past them) → APPROACH once only the stop leg remains → DWELL +
completed → IDLE. Safe-stop reflexes: no read within
`max_run_ms_without_tag` → failed `RFID_TIMEOUT`; a tag that is not in the
remaining schedule → failed `UNEXPECTED_TAG`, or `OVERSHOT` if only the stop
tag remained. `steps` nudges the train by a step count — the only command
that may go backward (refill positioning under live server control).
Duplicate `msg_id`s are ignored, expired TTLs answer `expired`, and a new
command cancels the active one (with the old `cmd_id`). Raw integers mirror
the firmware and emit no exec events: `0` stop, `1` move 1000 steps, `2` run
backward (manual-only; the on-screen arrow keeps pointing at the tip of the
train), `3` run forward.
