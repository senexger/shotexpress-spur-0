# fake-train

A virtual ShotExpress train with a ratatui TUI. Full MQTT drop-in for the real
ESP32: it subscribes to `shotexpress/command`, understands both the JSON
protocol (`move_to`, `stop`, …) and the raw firmware integers (`0`/`1`/`2`),
and publishes status heartbeats + exec events — so the whole chain
(dashboard → server → train) can be debugged without hardware.

## Run

```sh
cargo run -p fake-train    # reads MQTT_URL/MQTT_USERNAME/MQTT_PASSWORD from the root .env
```

Needs a terminal of at least 104x22.

Keys: `q` quit · `space` pause (heartbeats keep running) · `e` toggle shots
loaded/empty · `r` teleport to a random position (resets the belief) ·
`+`/`-` simulation speed (0.25x–8x).

## The track

A main line with a passive turnaround loop at each end (dogbone), flattened
into a ring of 14 tag readings (`fake-train/src/track.rs`). Main-line tags
(`station1`, `track12`, `station2`, `track23`, `station3`) are read twice per
lap (once per direction), curve tags (`curveA1/2`, `curveB1/2`) once. The
first curve tag slows the train to crawl, the second releases it — symmetric,
because the track has no start and no end.

## Localization

The simulation always knows the true position, but the *train's belief* is
computed only from tag readings + motor direction (`belief.rs`):
random spawn → unknown; one main-line tag → two candidate positions; a second
consecutive tag (or a single unique curve tag) → localized. The TUI shows
ground truth and belief side by side; `r` lets you watch relocalization.

## Command semantics

`move_to` routes by `target` (bar → station1, schachbrett → station2,
raucherecke → station3, forgot_name → failed `UNROUTABLE`); the literal
`expected_tags`/`stop_on_tag` names are ignored. Lifecycle: accepted →
progress per tag (last_tag + accumulated distance) → APPROACH (crawl) once
the next reading is the target → DWELL + completed → IDLE. No tag for
`max_run_ms_without_tag` → SAFE_STOP + failed `RFID_TIMEOUT`. Duplicate
`msg_id`s and expired TTLs are rejected like the old fakeExpress simulator.
Raw `0`/`1`/`2` mirror the current firmware and emit no exec events.
