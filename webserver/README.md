# ShotExpress Webserver (Next.js)

Next.js UI that publishes MQTT commands to the train and tracks responses in real time.

## Getting Started

### Prerequisites

- Node.js version pinned via `.nvmrc`
- `pnpm` (enable via `corepack enable` if needed)

### Installation

```bash
cd webserver
nvm install
nvm use
pnpm install
```

### Required environment variables

Create `.env.local` (see `.env.local.example`) with at least:

```
MQTT_URL=mqtt://localhost:1883           # broker URL (use wss://... if required by the broker)
MQTT_USERNAME=shotexpress                # optional
MQTT_PASSWORD=secret                     # optional
```

For the simulator (`fake_express`) you can override with:

```
FAKE_EXPRESS_MQTT_URL=mqtt://localhost:1883
FAKE_EXPRESS_MQTT_USER=shotexpress
FAKE_EXPRESS_MQTT_PASS=secret
```

### Run the web UI

```bash
pnpm dev
```

Open http://localhost:3000 to dispatch the train. The page shows:

- **Send train to Raucherecke** button publishing a spec-compliant `move_to` command.
- Live heartbeat (MQTT `shotexpress/status`) showing current state and uptime.
- Command lifecycle feed derived from `shotexpress/event/exec` events.

### Run the fake train simulator

```bash
pnpm fake:train
```

This starts `lib/fakeExpress.ts`, which consumes `shotexpress/command`, simulates a run to the Raucherecke, publishes status heartbeats, and emits lifecycle events.

Stop with `Ctrl+C`.