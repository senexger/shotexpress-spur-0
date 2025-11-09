# ShotExpress

A wifi-controlled shotexpress with:
- `/train` - ESP controlling the motor and RFID reader
- `/webserver` -  Next.js/Typescript orchestrating routes and state

## 1) Communication Architecture

Webserver and train connect to an MQTT broker and communicate over the following topics:

```
shotexpress/
  command              // server → train (commands)
  status           // train → server (heartbeat + live state)
  event/exec       // train → server (command lifecycle events)
```

Use the following QoS settings for topics:
- "at least once" for `command` and `event/*`, we deduplicate in the application logic.
- `status` at "at most once", it's frequent and transient.

Idempotency and ordering:
- Every command and event carries seq (uint32, monotonically increasing per producer) and id (`cmd_{$i}`, with `i` monotonically increasing).
- Train and server deduplicate commands by id and ignores older seq than the last applied.

## 2) Message contract

All payloads are JSON, UTF‑8.

### Common envelope

Events end commandos have these `common_envelope` fields:

```json
{
  "producer": "server",          // or "train"
  "msg_id": "550e8400-e29b-11d4-a716-446655440000",         // str, unique per message UUID-4
  "seq": 231,                    // uint32, per-producer monotonic
  "ts_ms": 1762593665123         // int64 unix epoch ms
}
```

### server → train: command

Topic: `shotexpress/command` (QoS 1, not retained, with MQTT message_expiry set).

```json
{
  ...common_envelope,
  "cmd_type": "move_to",                       // "move_to" | "stop" | "continue" | "wait_for_load"
  "params": {
    "target": "raucherecke",
    "speed": 0.60,                         // 0.00–1.00 (normalized)
    "direction": "forward",                // "forward" | "reverse"
    "expected_tags": ["tag_03","tag_04","tag_05"],
    "stop_on_tag": "tag_06",
    "offline_plan": {
      "approach_slowdown_ms": 2000,        // begin crawl this long after first expected tag miss
      "max_run_ms_without_tag": 7000,      // hard stop if no tag for this long
      "crawl_speed": 0.15,                 // speed used while searching for tag
      "dwell_ms": 1500                     // stop dwell after reaching stop_on_tag
    },
    "ttl_ms": 15000                        // discard command if not started in time
  }
}
```

If the train looses connection to the MQTT broker, then it falls back to the `offline_plan`:
- If the next `expected_tag` has been missed, the train slows down ("crawls") at `crawl_speed` after `approach_slowdown_ms` has passed.
- If the train doesn't read an RFID tag for `max_run_ms_without_tag`, it stops and `IDLE`s until it receives a new server message.
- If the train does contine reading RFID tags, it continues driving, until it reads the final `stop_on_tag`.
- It then waits for `dwell_ms` before it goes to `IDLE` state.


### train → server: event/exec

Topic: `shotexpress/event/exec` (QoS 1, not retained)

```json
{
  ...common_envelope,
  "event_type": "progress",              // accepted|started|progress|completed|failed|cancelled|expired
  "progress": { "last_tag": "tag_05", "distance_m": 2.3 },   // optional
  "error": null                     // or { "code": "RFID_TIMEOUT", "reason": "no tag in 7s" }
}
```

### train → server: status

These are heartbeat messages from the train.

Topic: `shotexpress/status` (QoS 0, not retained)

```json
{
  ...common_envelope,
  "state": "IDLE",                  // IDLE|RUNNING|APPROACH|DWELL|SAFE_STOP|ERROR
  "battery_pct": 82,          // uint32 between 0 and 100
  "status_uptime_ms": 1762593600000      // uint64, in ms for how long the current status exists
}
```


## 3) Minimal train state machine

Normal states running from the bar to the Raucherecke: `IDLE → RUNNING → APPROACH → DWELL → IDLE`.

- `IDLE`: Train waits and saves power. Waits for new commands from webserver.
- `RUNNING`: Train executes a command.
- `APPROACH`: begins after N consecutive missed expected tags, or on a configured distance/time threshold; speed = crawl_speed.
- `SAFE_STOP`: engages on max_run_ms_without_tag, cancel, or broker disconnect; motor off, networking on; emits failed or cancelled. 
- `DWELL`: Train is stopped and waits, but remains at full networking capacity to quickly react to new commands.

**Train Pseudocode:**

```
on command(move_to p):
  if now > command_ts + ttl: ignore  # each webserver command has a ttl. If that expires, fall back to offline_plan
  ack_id = msg_id
  set speed = p.speed, dir = p.direction
  deadline = now + p.offline_plan.max_run_ms_without_tag
  while running:
    if tag in p.stop_on_tag_set: stop(); wait p.offline_plan.dwell_ms; publish exec:completed; break
    if now > deadline: stop(); error=RFID_TIMEOUT; publish exec:failed; break
    if missed expected tag: speed = p.offline_plan.crawl_speed
```

## 4) RFID Layout

We can consider adding additional tags, e.g. `tag_01_a` and `tag_01_b` for redundancy.
This should be okay for a first iteration though.

| Location    | Tag ID |
| ----------- | ------ |
| Bar         | tag_01 |
| Midpoint 1  | tag_02 |
| Schachbrett | tag_03 |
| Midpoint 2  | tag_04 |
| Name Vergessen | tag_05 |
| Midpoint 3  | tag_06 |
| Raucherecke  | tag_07 |

## Getting Started

### Train Setup
Todo: Write this.

### Webserver Setup
1. Navigate to the `webserver/` directory
2. Run `pnpm install` to install dependencies
3. Run `npm run build` to compile TypeScript
4. Run `npm run dev` to start the development server
