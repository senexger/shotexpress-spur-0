# ShotExpress Control Server

Single-binary Rust rewrite of the Next.js `webserver/`: serves the control dashboard
(embedded static HTML/JS/CSS) and talks MQTT to the train. No Node, no build step —
`cargo build --release` produces one self-contained binary.

## Run

```sh
cp .env.example .env   # set MQTT_URL / MQTT_USERNAME / MQTT_PASSWORD
cargo run              # dev, http://localhost:3000
cargo build --release  # binary at target/release/shotexpress-server
```

The binary only needs a `.env` (or the env vars) next to it. `PORT` overrides the
HTTP port (default 3000). Logging via `RUST_LOG` (default `info`).

Requires a running MQTT broker (see `../mqtt-broker/`). `MQTT_URL` must be plain
TCP (`mqtt://host:port`) — websocket URLs are not supported.

## API

- `GET /` — dashboard (assets embedded at compile time from `assets/`)
- `GET /api/train/state` — controller snapshot
- `POST /api/train/command` — dispatch a `move_to` command; optional body
  `{ttl_ms, parameters}` merged over defaults (409 when busy, 503 when disconnected)
- `POST /api/train/manual` — `{action: "stop"|"steps"|"run"}`, published as the raw
  `0`/`1`/`2` integers the current Train.ino firmware understands
- `GET /api/train/stream` — SSE stream of service events (snapshot first, 25 s heartbeat)

## Protocol notes

- Wire shapes follow `webserver/lib/schemas.ts` (`exec_type` discriminator); the root
  README's spec is stale where it says `event_type`/`started` — do not "fix" the code
  toward it.
- Unparseable MQTT payloads (e.g. the firmware's literal `Alive` status) are logged
  and dropped, matching the old zod behavior.
- The TS simulator remains the protocol oracle: `pnpm --dir ../webserver fake:train`.

## Testing

`cargo test` covers the serde wire formats, including a golden `move_to` fixture
generated from the TS `createMoveCommand` implementation.
