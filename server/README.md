# ShotExpress Control Server

Single-binary Rust control server: serves the dashboard (static HTML/JS/CSS embedded
at compile time from `assets/`), owns the track map, and talks MQTT to the train.
No Node, no build step — `cargo build --release` produces one self-contained binary.

The wire protocol lives in `../protocol/` (shared serde types + golden-shape tests);
`../fake-train/` is the reference implementation of the train side. The old Next.js
`webserver/` is kept only until the real train is verified against this server.

## Run

```sh
cp .env.example .env   # set MQTT_URL / MQTT_USERNAME / MQTT_PASSWORD
cargo run              # dev, http://localhost:3000
cargo build --release  # binary at target/release/shotexpress-server
```

TODO: Why cant this be configured in a toml file?
The binary needs a `.env` (or the env vars) next to it, plus a `track.toml`
(path via `TRACK_CONFIG`, default repo-root `track.toml`). `PORT` overrides the
HTTP port (default 3000). Logging via `RUST_LOG` (default `info`).

Requires a running MQTT broker (see `../mqtt-broker/`). `MQTT_URL` must be plain
TCP (`mqtt://host:port`) — websocket URLs are not supported.

## Order page (http://shotexpress.local)

`/` is the guest page, styled like a train booking site: "Von" is fixed to the refill
station, "Nach" lists the `unload` stations, guests enter a number of shots and hit
**Bestellen**. Orders queue up and run one at a time, fully server-side
(`src/orders.rs`): drive to the bar (skipped if the last tag read was the bar's),
pour `shot_ml` per shot with a `steps` nudge between glasses, drive to the target.
Shot size, max shots per order and the nudge wait live in `[refill]` in `track.toml`.

- mDNS: the server advertises `shotexpress.local` itself (env `MDNS_HOSTNAME`,
  empty = off). Works next to avahi. If `MQTT_URL` points at this machine
  (`localhost`), it also advertises `_mqtt._tcp`, which the train uses to find the
  broker without a hard-coded IP.
- Port 80: the server also listens on `PUBLIC_PORT` (default 80, empty = off) so the
  URL needs no port. Without root, grant it once:
  `sudo setcap cap_net_bind_service=+ep target/release/shotexpress-server`
  (redo after every rebuild); otherwise the page is at `http://shotexpress.local:3000`.

## API

- `GET /` — guest order page (plus `/order.css`, `/order.js`); `#SE<n>` opens order n
- `GET /admin` — dashboard (plus `/style.css`, `/app.js`)
- `POST /api/order` — `{target, shots}`: queue an order (400 on unknown/non-unload
  station or shots outside `1..=max_shots`)
- `GET /api/orders` — orders, newest first (`phase`: queued, to_refill, fueling,
  delivering, arrived, failed; `poured`, `ahead`, `error`, `failed_in`)
- `GET /api/orders/{id}` — one order
- `GET /api/train/state` — controller snapshot (`connection`, `lastStatus`, `events`,
  `isBusy`, `error`, `lastKnownTag`, `lastTagRead`)
- `POST /api/train/command` — `{target, speed?, ttl_ms?}`: compiles a `drive` leg
  schedule from `track.toml` to the named station (409 when busy, 503 when disconnected)
- `POST /api/train/steps` — `{direction, steps?, speed?, ttl_ms?}`: precise positioning
  nudge for refill; defaults come from `[refill]` in `track.toml`
- `POST /api/train/manual` — `{action: "stop"|"steps"|"forward"|"backward"}`, published
  as raw firmware integers `0`/`1`/`3`/`2` (Train.ino: stopMove, move 1000 steps,
  runForward, runBackward). Debug/setup only — no cmd_id, no exec events
- `GET /api/train/stream` — SSE stream of service events (snapshot first, 25 s heartbeat)
- `GET /api/track` — track summary (train params, refill, stations, ring order)
- `POST /api/admin/track/reload` — re-read and validate `track.toml` without restart

## Protocol notes

- Source of truth for wire shapes is the `shotexpress-protocol` crate (`exec_type` /
  `cmd_type` discriminators); the fake-train simulator is the behavioral oracle:
  `cargo run -p fake-train`.
- Unparseable MQTT payloads (e.g. the current firmware's literal `Alive` status) are
  logged and dropped.
- `shotexpress/event/tag` carries every raw RFID read as a bare uid string (no JSON,
  works with the current Train.ino). The server surfaces it as `lastTagRead` in the
  snapshot and as a `tag` SSE event — a setup aid for mapping physical tags to
  `track.toml` names. It is *not* used for localization (that stays on consecutive
  progress-tag pairs, which are forward-only by construction).
- The fuel station (refill dispenser) speaks the same envelope pattern on
  `shotexpress/fuel/command` / `shotexpress/fuel/event` / `shotexpress/fuel/status`;
  types are in the protocol crate, firmware guide in `../fuel-station/README.md`.
  Orders from the guest page drive it (one pour per shot, see `src/orders.rs`).

## Testing

`cargo test --workspace` covers the serde wire formats (golden fixtures in the
protocol crate), track validation/route compilation against the repo `track.toml`,
and the simulator's command semantics.
