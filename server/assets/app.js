"use strict";

const SNAPSHOT_URL = "/api/train/state";
const COMMAND_URL = "/api/train/command";
const MANUAL_URL = "/api/train/manual";
const STREAM_URL = "/api/train/stream";

const state = {
  connection: "connecting",
  lastStatus: null,
  events: [],
  isBusy: false,
  error: null,
  manualError: null,
};

const el = {
  connectionLabel: document.getElementById("connection-label"),
  controlError: document.getElementById("control-error"),
  sendButton: document.getElementById("send-button"),
  manualError: document.getElementById("manual-error"),
  manualButtons: document.querySelectorAll("[data-action]"),
  statusEmpty: document.getElementById("status-empty"),
  statusGrid: document.getElementById("status-grid"),
  statusState: document.getElementById("status-state"),
  statusBattery: document.getElementById("status-battery"),
  statusUptime: document.getElementById("status-uptime"),
  statusHeartbeat: document.getElementById("status-heartbeat"),
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
    default:
      break;
  }
  render();
}

function setText(node, text) {
  if (node.textContent !== text) node.textContent = text;
}

function render() {
  setText(el.connectionLabel, connectionLabel(state.connection));

  el.controlError.hidden = !state.error;
  if (state.error) setText(el.controlError, state.error);

  el.manualError.hidden = !state.manualError;
  if (state.manualError) setText(el.manualError, state.manualError);

  el.sendButton.disabled = state.isBusy || state.connection !== "connected";
  setText(el.sendButton, state.isBusy ? "Command in progress…" : "Send train to Raucherecke");

  const status = state.lastStatus;
  el.statusEmpty.hidden = Boolean(status);
  el.statusGrid.hidden = !status;
  if (status) {
    setText(el.statusState, status.state);
    setText(el.statusBattery, `${status.battery_pct}%`);
    setText(el.statusUptime, formatMillis(status.status_uptime_ms));
    setText(el.statusHeartbeat, formatTimestamp(status.ts_ms));
  }

  const latest = state.events.at(-1) ?? null;
  el.lifecycleEmpty.hidden = Boolean(latest);
  el.lifecycleDetails.hidden = !latest;
  if (latest) {
    let summary = latest.exec_type;
    if (latest.progress && latest.progress.last_tag) {
      summary += `, last tag ${latest.progress.last_tag}`;
    }
    if (latest.progress && typeof latest.progress.distance_m === "number") {
      summary += `, distance ${latest.progress.distance_m.toFixed(1)} m`;
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

async function sendCommand() {
  try {
    const response = await fetch(COMMAND_URL, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({}),
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
  render();
  try {
    const response = await fetch(MANUAL_URL, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ action }),
    });
    if (!response.ok) {
      const data = await response.json().catch(() => null);
      state.manualError = data?.error ?? `Manual command failed (${response.status})`;
    }
  } catch (err) {
    state.manualError = err instanceof Error ? err.message : "Manual command failed";
  }
  render();
}

async function init() {
  el.sendButton.addEventListener("click", sendCommand);
  for (const button of el.manualButtons) {
    button.addEventListener("click", () => sendManual(button.dataset.action));
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
