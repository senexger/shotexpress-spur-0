"use strict";

const SNAPSHOT_URL = "/api/train/state";
const COMMAND_URL = "/api/train/command";
const MANUAL_URL = "/api/train/manual";
const SPEED_URL = "/api/train/speed";
const MOVE_URL = "/api/train/move";
const MAX_SPEED_HZ = 20000;
const MAX_MOVE_STEPS = 1000000;
const ACCEL_URL = "/api/train/accel";
const MAX_ACCEL = 1000000;
const FUEL_DISPENSE_URL = "/api/fuel/dispense";
const FUEL_STOP_URL = "/api/fuel/stop";
const MAX_FILL_ML = 500;
// the station heartbeats every second; quiet this long = offline
const FUEL_OFFLINE_AFTER_MS = 5000;
// must match GLASS_PRESENT_MM in fuel-station.ino (display only)
const GLASS_PRESENT_MM = 200;
const FUEL_CHART_WINDOW_MS = 30000;
// forward speed presets in steps/s; the server caps at 20000 (Train.ino MAX_SPEED_HZ)
const SPEED_PRESETS_HZ = [2000, 4000, 6000, 8000, 10000, 12000, 14000, 16000, 18000, 20000];
const TRACK_URL = "/api/track";
const STREAM_URL = "/api/train/stream";

const state = {
  connection: "connecting",
  lastStatus: null,
  events: [],
  isBusy: false,
  error: null,
  lastTagRead: null,
  lastRawStatus: null,
  trainOnline: false,
  speedHz: null,
  accel: null,
  fuelStatus: null,
  // browser receive time of the last fuel heartbeat (station clock may be unsynced)
  fuelStatusSeenAt: null,
  fuelLastEvent: null,
  fuelError: null,
  fuelDistanceMm: null,
  // [browser time, mm] for the chart; -1 (no echo) kept as a gap
  fuelDistanceHistory: [],
  stations: [],
  manualError: null,
};

const el = {
  connectionLabel: document.getElementById("connection-label"),
  controlError: document.getElementById("control-error"),
  stationButtons: document.getElementById("station-buttons"),
  manualError: document.getElementById("manual-error"),
  manualButtons: document.querySelectorAll("[data-action]"),
  speedButtons: document.getElementById("speed-buttons"),
  speedCurrent: document.getElementById("speed-current"),
  accelCurrent: document.getElementById("accel-current"),
  typedAccel: document.getElementById("typed-accel"),
  typedHz: document.getElementById("typed-hz"),
  typedSteps: document.getElementById("typed-steps"),
  typedMl: document.getElementById("typed-ml"),
  fuelState: document.getElementById("fuel-state"),
  fuelLastEvent: document.getElementById("fuel-last-event"),
  fuelError: document.getElementById("fuel-error"),
  fuelDistance: document.getElementById("fuel-distance"),
  fuelGlass: document.getElementById("fuel-glass"),
  fuelChartLine: document.getElementById("fuel-chart-line"),
  fuelChartRange: document.getElementById("fuel-chart-range"),
  fuelButtons: [document.getElementById("fuel-fill"), document.getElementById("fuel-stop")],
  fireButtons: [
    document.getElementById("fire-speed"),
    document.getElementById("fire-steps"),
    document.getElementById("fire-accel"),
  ],
  statusEmpty: document.getElementById("status-empty"),
  statusGrid: document.getElementById("status-grid"),
  statusState: document.getElementById("status-state"),
  statusBattery: document.getElementById("status-battery"),
  statusUptime: document.getElementById("status-uptime"),
  statusHeartbeat: document.getElementById("status-heartbeat"),
  statusLastTag: document.getElementById("status-last-tag"),
  lifecycleEmpty: document.getElementById("lifecycle-empty"),
  lifecycleDetails: document.getElementById("lifecycle-details"),
  latestSummary: document.getElementById("latest-summary"),
  latestError: document.getElementById("latest-error"),
  eventHistory: document.getElementById("event-history"),
};

function connectionLabel(connection) {
  switch (connection) {
    case "connected":
      return "Connected";
    case "connecting":
      return "Connecting";
    case "reconnecting":
      return "Reconnecting";
    case "disconnected":
      return "Disconnected";
    case "error":
      return "Error";
    default:
      return connection;
  }
}

function formatTimestamp(ts) {
  return new Date(ts).toLocaleTimeString();
}

function formatMillis(ms) {
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60000) return `${(ms / 1000).toFixed(1)} s`;
  return `${(ms / 60000).toFixed(1)} min`;
}

function reduce(event) {
  switch (event.type) {
    case "snapshot":
      state.connection = event.payload.connection;
      state.lastStatus = event.payload.lastStatus;
      state.events = event.payload.events;
      state.isBusy = event.payload.isBusy;
      state.error = event.payload.error;
      state.lastTagRead = event.payload.lastTagRead;
      state.lastRawStatus = event.payload.lastRawStatus;
      state.trainOnline = event.payload.trainOnline;
      state.speedHz = event.payload.speedHz;
      state.accel = event.payload.accel;
      state.fuelStatus = event.payload.fuelStatus;
      state.fuelLastEvent = event.payload.fuelLastEvent;
      state.fuelDistanceMm = event.payload.fuelDistanceMm;
      break;
    case "fueldistance": {
      const now = Date.now();
      state.fuelDistanceMm = event.payload;
      state.fuelDistanceHistory.push([now, event.payload]);
      while (state.fuelDistanceHistory.length && state.fuelDistanceHistory[0][0] < now - FUEL_CHART_WINDOW_MS) {
        state.fuelDistanceHistory.shift();
      }
      // 4 Hz: only redraw the fuel card, not the whole page
      renderFuel();
      return;
    }
    case "fuelstatus":
      state.fuelStatus = event.payload;
      state.fuelStatusSeenAt = Date.now();
      break;
    case "fuelevent":
      state.fuelLastEvent = event.payload;
      break;
    case "speed":
      state.speedHz = event.payload;
      break;
    case "accel":
      state.accel = event.payload;
      break;
    case "connection":
      state.connection = event.payload;
      break;
    case "status":
      state.lastStatus = event.payload;
      break;
    case "events":
      state.events = event.payload;
      break;
    case "busy":
      state.isBusy = event.payload;
      break;
    case "error":
      state.error = event.payload;
      break;
    case "tag":
      state.lastTagRead = event.payload;
      break;
    case "rawstatus":
      state.lastRawStatus = event.payload;
      break;
    case "liveness":
      state.trainOnline = event.payload;
      break;
    default:
      break;
  }
  render();
}

function setText(node, text) {
  if (node.textContent !== text) node.textContent = text;
}

function renderStations() {
  el.stationButtons.replaceChildren(
    ...state.stations.map((station) => {
      const button = document.createElement("button");
      button.className = "button";
      button.textContent = `Send to ${station.name}`;
      button.dataset.station = station.name;
      button.addEventListener("click", () => sendCommand(station.name));
      return button;
    }),
  );
  render();
}

function render() {
  setText(el.connectionLabel, connectionLabel(state.connection));

  el.controlError.hidden = !state.error;
  if (state.error) setText(el.controlError, state.error);

  const disabled = state.isBusy || state.connection !== "connected";
  for (const button of el.stationButtons.querySelectorAll("button")) {
    button.disabled = disabled;
  }

  // manual override is available even while a ride is in flight — sending
  // one ends the ride — so it only needs the MQTT connection
  el.manualError.hidden = !state.manualError;
  if (state.manualError) setText(el.manualError, state.manualError);
  for (const button of el.manualButtons) {
    button.disabled = state.connection !== "connected";
  }

  setText(el.speedCurrent, state.speedHz ? `${state.speedHz} steps/s` : "—");
  setText(el.accelCurrent, state.accel ? `${state.accel} steps/s²` : "—");
  for (const button of el.fireButtons) {
    button.disabled = state.connection !== "connected";
  }
  for (const button of el.speedButtons.querySelectorAll("button")) {
    button.disabled = state.connection !== "connected";
    button.classList.toggle("buttonActive", Number(button.dataset.hz) === state.speedHz);
  }

  renderFuel();

  const tagRead = state.lastTagRead;
  setText(
    el.statusLastTag,
    tagRead ? `${tagRead.tag} · ${formatTimestamp(tagRead.ts_ms)}` : "—",
  );

  const status = state.lastStatus;
  const rawStatus = state.lastRawStatus;
  el.statusEmpty.hidden = Boolean(status || rawStatus);
  el.statusGrid.hidden = !(status || rawStatus);
  el.statusState.classList.toggle("statusDead", !state.trainOnline);
  if (!state.trainOnline && (status || rawStatus)) {
    // heartbeat went quiet: the server flipped the train to dead
    const lastSeen = status?.ts_ms ?? rawStatus.ts_ms;
    setText(el.statusState, `Dead (last seen ${formatTimestamp(lastSeen)})`);
    setText(el.statusBattery, "—");
    setText(el.statusUptime, "—");
    setText(el.statusHeartbeat, formatTimestamp(lastSeen));
  } else if (status) {
    setText(el.statusState, status.state);
    setText(el.statusBattery, `${status.battery_pct}%`);
    setText(el.statusUptime, formatMillis(status.status_uptime_ms));
    setText(el.statusHeartbeat, formatTimestamp(status.ts_ms));
  } else if (rawStatus) {
    // legacy firmware heartbeat ("Alive"): online, but no telemetry
    setText(el.statusState, `Online (“${rawStatus.raw}”)`);
    setText(el.statusBattery, "—");
    setText(el.statusUptime, "—");
    setText(el.statusHeartbeat, formatTimestamp(rawStatus.ts_ms));
  }

  const latest = state.events.at(-1) ?? null;
  el.lifecycleEmpty.hidden = Boolean(latest);
  el.lifecycleDetails.hidden = !latest;
  if (latest) {
    let summary = latest.exec_type;
    if (latest.last_tag) {
      summary += `, last tag ${latest.last_tag}`;
    }
    if (typeof latest.distance_m === "number") {
      summary += `, distance ${latest.distance_m.toFixed(1)} m`;
    }
    if (latest.skipped) {
      summary += `, ${latest.skipped} tag(s) skipped`;
    }
    setText(el.latestSummary, summary);

    el.latestError.hidden = !latest.error;
    if (latest.error) {
      setText(el.latestError, `Error ${latest.error.code}: ${latest.error.reason}`);
    }

    el.eventHistory.replaceChildren(
      ...state.events
        .slice(-5)
        .reverse()
        .map((event) => {
          const item = document.createElement("li");
          const type = document.createElement("span");
          type.className = "valueText";
          type.textContent = event.exec_type;
          item.append(type, ` · ${formatTimestamp(event.ts_ms)}`);
          return item;
        }),
    );
  }
}

async function sendCommand(target) {
  state.error = null;
  try {
    const response = await fetch(COMMAND_URL, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ target }),
    });
    if (!response.ok) {
      const data = await response.json().catch(() => null);
      throw new Error(data?.error ?? `Command failed (${response.status})`);
    }
    state.events = [];
    state.isBusy = true;
  } catch (err) {
    state.error = err instanceof Error ? err.message : "Failed to send command";
  }
  render();
}

async function sendManual(action) {
  state.manualError = null;
  try {
    const response = await fetch(MANUAL_URL, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ action }),
    });
    if (!response.ok) {
      const data = await response.json().catch(() => null);
      throw new Error(data?.error ?? `Manual command failed (${response.status})`);
    }
    // the server drops the busy slot on manual override; reflect it
    // immediately instead of waiting for the SSE round trip
    state.isBusy = false;
  } catch (err) {
    state.manualError = err instanceof Error ? err.message : "Failed to send manual command";
  }
  render();
}

async function postRaw(url, body) {
  state.manualError = null;
  try {
    const response = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!response.ok) {
      const data = await response.json().catch(() => null);
      const details = data?.details ? `: ${data.details}` : "";
      throw new Error(`${data?.error ?? `Command failed (${response.status})`}${details}`);
    }
    // raw commands are manual overrides: the busy slot is dropped
    state.isBusy = false;
  } catch (err) {
    state.manualError = err instanceof Error ? err.message : "Failed to send command";
  }
  render();
}

function renderFuel() {
  const online =
    state.fuelStatusSeenAt !== null && Date.now() - state.fuelStatusSeenAt < FUEL_OFFLINE_AFTER_MS;
  const fuel = state.fuelStatus;
  let label = "no heartbeat yet";
  if (fuel && online) label = `${fuel.state} · tank ~${fuel.tank_pct}%`;
  else if (fuel) label = "offline";
  setText(el.fuelState, label);
  el.fuelState.classList.toggle("statusDead", Boolean(fuel) && !online);

  const event = state.fuelLastEvent;
  let reply = "—";
  if (event) {
    reply = event.exec_type;
    if (event.error) reply += ` (${event.error.code}: ${event.error.reason})`;
  }
  setText(el.fuelLastEvent, reply);

  const mm = state.fuelDistanceMm;
  if (mm === null || mm === undefined) {
    setText(el.fuelDistance, "—");
    setText(el.fuelGlass, "");
  } else if (mm < 0) {
    setText(el.fuelDistance, "no echo");
    setText(el.fuelGlass, "· counts as no glass");
  } else {
    setText(el.fuelDistance, `${mm} mm`);
    setText(el.fuelGlass, mm <= GLASS_PRESENT_MM ? "· glass detected" : "· no glass");
  }
  renderFuelChart();

  el.fuelError.hidden = !state.fuelError;
  if (state.fuelError) setText(el.fuelError, state.fuelError);
  for (const button of el.fuelButtons) {
    button.disabled = state.connection !== "connected";
  }
}

function renderFuelChart() {
  const points = state.fuelDistanceHistory.filter(([, mm]) => mm >= 0);
  if (points.length === 0) {
    el.fuelChartLine.setAttribute("points", "");
    setText(el.fuelChartRange, "no data");
    return;
  }
  const values = points.map(([, mm]) => mm);
  const min = Math.min(...values);
  const max = Math.max(...values);
  const span = Math.max(max - min, 10); // keep a flat line from filling the box
  const end = Date.now();
  const coords = points.map(([t, mm]) => {
    const x = 300 - ((end - t) / FUEL_CHART_WINDOW_MS) * 300;
    const y = 55 - ((mm - min) / span) * 50;
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  });
  el.fuelChartLine.setAttribute("points", coords.join(" "));
  setText(el.fuelChartRange, `${min}–${max} mm`);
}

async function postFuel(url, body) {
  state.fuelError = null;
  try {
    const response = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body ?? {}),
    });
    if (!response.ok) {
      const data = await response.json().catch(() => null);
      const details = data?.details ? `: ${data.details}` : "";
      throw new Error(`${data?.error ?? `Fuel command failed (${response.status})`}${details}`);
    }
  } catch (err) {
    state.fuelError = err instanceof Error ? err.message : "Failed to send fuel command";
  }
  render();
}

function fireFill() {
  const volume = Number(el.typedMl.value);
  if (!Number.isInteger(volume) || volume < 1 || volume > MAX_FILL_ML) {
    state.fuelError = `Amount must be a whole number between 1 and ${MAX_FILL_ML} ml`;
    render();
    return;
  }
  postFuel(FUEL_DISPENSE_URL, { volume_ml: volume });
}

function sendSpeed(hz) {
  return postRaw(SPEED_URL, { hz });
}

// typed values: whole numbers in range, else an inline error
function readTyped(input, name, min, max) {
  const value = Number(input.value);
  if (!Number.isInteger(value) || value < min || value > max || value === 0) {
    state.manualError = `${name} must be a non-zero whole number between ${min} and ${max}`;
    render();
    return null;
  }
  return value;
}

function readTypedAccel() {
  return readTyped(el.typedAccel, "Acceleration", 1, MAX_ACCEL);
}

function fireTypedSpeed() {
  const hz = readTyped(el.typedHz, "Speed", 1, MAX_SPEED_HZ);
  if (hz === null) return;
  const accel = readTypedAccel();
  if (accel !== null) postRaw(SPEED_URL, { hz, accel });
}

function fireTypedSteps() {
  const hz = readTyped(el.typedHz, "Speed", 1, MAX_SPEED_HZ);
  if (hz === null) return;
  const accel = readTypedAccel();
  if (accel === null) return;
  const steps = readTyped(el.typedSteps, "Steps", -MAX_MOVE_STEPS, MAX_MOVE_STEPS);
  if (steps !== null) postRaw(MOVE_URL, { steps, hz, accel });
}

function fireTypedAccel() {
  const accel = readTypedAccel();
  if (accel !== null) postRaw(ACCEL_URL, { accel });
}

// remember typed values per browser; storage may be unavailable
function restoreTyped(input, key) {
  try {
    const saved = localStorage.getItem(key);
    if (saved !== null) input.value = saved;
  } catch {}
  input.addEventListener("change", () => {
    try {
      localStorage.setItem(key, input.value);
    } catch {}
  });
}

function renderSpeedButtons() {
  el.speedButtons.replaceChildren(
    ...SPEED_PRESETS_HZ.map((hz, i) => {
      const button = document.createElement("button");
      button.className = "button";
      button.textContent = `${i + 1}`;
      button.title = `Forward at ${hz} steps/s`;
      button.dataset.hz = String(hz);
      button.addEventListener("click", () => sendSpeed(hz));
      return button;
    }),
  );
}

async function init() {
  renderSpeedButtons();
  restoreTyped(el.typedHz, "shotexpress.typedHz");
  restoreTyped(el.typedSteps, "shotexpress.typedSteps");
  restoreTyped(el.typedMl, "shotexpress.typedMl");
  el.fuelButtons[0].addEventListener("click", fireFill);
  el.fuelButtons[1].addEventListener("click", () => postFuel(FUEL_STOP_URL));
  // flips the fuel station to "offline" when its heartbeat stops
  setInterval(renderFuel, 1000);
  el.fireButtons[0].addEventListener("click", fireTypedSpeed);
  el.fireButtons[1].addEventListener("click", fireTypedSteps);
  el.fireButtons[2].addEventListener("click", fireTypedAccel);
  restoreTyped(el.typedAccel, "shotexpress.typedAccel");
  for (const button of el.manualButtons) {
    button.addEventListener("click", () => sendManual(button.dataset.action));
  }

  try {
    const response = await fetch(TRACK_URL, { cache: "no-store" });
    if (!response.ok) throw new Error(`Failed to load track (${response.status})`);
    const track = await response.json();
    state.stations = track.stations;
    renderStations();
  } catch (err) {
    el.stationButtons.replaceChildren();
    state.error = err instanceof Error ? err.message : "Failed to load stations";
    render();
  }

  try {
    const response = await fetch(SNAPSHOT_URL, { cache: "no-store" });
    if (!response.ok) throw new Error(`Failed to load train state (${response.status})`);
    reduce({ type: "snapshot", payload: await response.json() });
  } catch (err) {
    state.connection = "error";
    state.error = err instanceof Error ? err.message : "Failed to load train state";
    render();
  }

  const source = new EventSource(STREAM_URL);
  source.onmessage = (event) => {
    try {
      reduce(JSON.parse(event.data));
    } catch (err) {
      console.warn("[train] failed to parse stream event", err);
    }
  };
  source.addEventListener("heartbeat", () => {
    // heartbeat ensures the connection stays warm; no action needed
  });
  source.onerror = () => {
    if (state.error === null) {
      state.error = "Lost connection to train event stream";
      render();
    }
  };
}

init();
