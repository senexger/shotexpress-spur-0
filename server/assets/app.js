"use strict";

const SNAPSHOT_URL = "/api/train/state";
const COMMAND_URL = "/api/train/command";
const MANUAL_URL = "/api/train/manual";
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
  stations: [],
  manualError: null,
};

const el = {
  connectionLabel: document.getElementById("connection-label"),
  controlError: document.getElementById("control-error"),
  stationButtons: document.getElementById("station-buttons"),
  manualError: document.getElementById("manual-error"),
  manualButtons: document.querySelectorAll("[data-action]"),
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

async function init() {
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
