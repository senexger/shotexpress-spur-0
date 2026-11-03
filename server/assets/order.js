"use strict";

// Public order page: pick a station, enter shots, "Bestellen". The server
// queues the order and runs it (train to the bar, pour, deliver); this page
// only polls the order list and draws it like a train connection.

const $ = (id) => document.getElementById(id);
const STORAGE_KEY = "shotexpress.myOrder";
const POLL_MS = 1500;

const PHASES = ["queued", "to_refill", "fueling", "delivering", "arrived"];

let track = null;
let myOrderId = orderFromHash() ?? loadMyOrder();

/** "#SE7" opens order 7 (shareable link to an order). */
function orderFromHash() {
  const match = /^#SE(\d+)$/i.exec(location.hash);
  return match ? Number(match[1]) : null;
}
let maxShots = 6;

function loadMyOrder() {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw ? Number(raw) : null;
  } catch {
    return null;
  }
}

function saveMyOrder(id) {
  myOrderId = id;
  history.replaceState(null, "", id == null ? location.pathname : `#SE${id}`);
  try {
    if (id == null) localStorage.removeItem(STORAGE_KEY);
    else localStorage.setItem(STORAGE_KEY, String(id));
  } catch {
    // private mode: the order still shows until reload
  }
}

/** "schachbrett" → "Schachbrett", "raucher_ecke" → "Raucher Ecke" */
function label(name) {
  return String(name)
    .split(/[_-]/)
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(" ");
}

function hhmm(ms) {
  return new Date(ms).toLocaleTimeString("de-DE", { hour: "2-digit", minute: "2-digit" });
}

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (key === "class") node.className = value;
    else node.setAttribute(key, value);
  }
  for (const child of children) {
    if (child != null) node.append(child);
  }
  return node;
}

async function api(path, options) {
  const res = await fetch(path, options);
  const body = await res.json().catch(() => ({}));
  if (!res.ok) {
    throw new Error(body.details || body.error || `HTTP ${res.status}`);
  }
  return body;
}

// ---- form ----

async function loadTrack() {
  try {
    track = await api("/api/track");
  } catch (err) {
    showFormError("Fahrplan konnte nicht geladen werden. Bitte Seite neu laden.");
    return;
  }
  $("from").value = label(track.refill.station);
  maxShots = track.refill.max_shots || 6;
  $("shots").max = String(maxShots);

  const select = $("to");
  for (const station of track.stations.filter((s) => s.role === "unload")) {
    select.append(el("option", { value: station.name }, label(station.name)));
  }
  try {
    const last = localStorage.getItem("shotexpress.lastTarget");
    if (last && [...select.options].some((o) => o.value === last)) select.value = last;
  } catch {}
}

function clampShots(value) {
  const n = Math.round(Number(value));
  if (!Number.isFinite(n)) return 1;
  return Math.min(maxShots, Math.max(1, n));
}

for (const button of document.querySelectorAll(".step")) {
  button.addEventListener("click", () => {
    const input = $("shots");
    input.value = clampShots(Number(input.value) + Number(button.dataset.step));
  });
}

$("shots").addEventListener("change", (e) => {
  e.target.value = clampShots(e.target.value);
});

function showFormError(message) {
  const box = $("form-error");
  box.textContent = message || "";
  box.hidden = !message;
}

$("order-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  showFormError(null);
  const target = $("to").value;
  if (!target) {
    showFormError("Bitte wählen Sie einen Zielbahnhof.");
    $("to").focus();
    return;
  }
  const shots = clampShots($("shots").value);
  $("shots").value = shots;

  const submit = $("submit");
  submit.disabled = true;
  submit.textContent = "Wird gebucht …";
  try {
    const body = await api("/api/order", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ target, shots }),
    });
    try { localStorage.setItem("shotexpress.lastTarget", target); } catch {}
    saveMyOrder(body.order.id);
    renderTrip(body.order);
    $("trip").scrollIntoView({ behavior: "smooth", block: "start" });
    poll();
  } catch (err) {
    showFormError(`Buchung nicht möglich: ${err.message}`);
  } finally {
    submit.disabled = false;
    submit.textContent = "Bestellen";
  }
});

$("t-new").addEventListener("click", () => {
  saveMyOrder(null);
  $("trip").hidden = true;
  window.scrollTo({ top: 0, behavior: "smooth" });
});

// ---- the order as a connection ----

function phaseIndex(order) {
  const phase = order.phase === "failed" ? order.failed_in : order.phase;
  return Math.max(0, PHASES.indexOf(phase));
}

function statusLine(order) {
  switch (order.phase) {
    case "queued":
      return order.ahead > 0
        ? ["wait", `Bestellung bestätigt – vor Ihnen ${order.ahead === 1 ? "1 Bestellung" : `${order.ahead} Bestellungen`}`]
        : ["wait", "Bestellung bestätigt – Abfahrt in Kürze"];
    case "to_refill":
      return ["wait", "Der Zug wird bereitgestellt"];
    case "fueling":
      return ["wait", `Betankung läuft (${order.poured} von ${order.shots})`];
    case "delivering":
      return ["ok", "Pünktlich unterwegs"];
    case "arrived":
      return ["ok", "Angekommen. Zum Wohl!"];
    case "failed":
      return ["err", `Störung im Betriebsablauf: ${order.error || "unbekannt"}`];
    default:
      return ["wait", ""];
  }
}

function renderTrip(order) {
  $("trip").hidden = false;
  const from = label(order.from);
  const to = label(order.target);
  $("t-train").textContent = `SE ${order.id}`;
  $("t-dep").textContent = hhmm(order.created_ms);
  $("t-arr").textContent =
    order.phase === "arrived" ? hhmm(order.updated_ms) : `ca. ${hhmm(order.created_ms + 90_000 + order.shots * 20_000)}`;
  $("t-from").textContent = from;
  $("t-to").textContent = to;
  $("t-shots").textContent = `${order.shots} ${order.shots === 1 ? "Shot" : "Shots"} · 0,00 €`;

  const [cls, text] = statusLine(order);
  const status = $("t-status");
  status.className = `conn-status ${cls}`;
  status.textContent = text;

  const current = phaseIndex(order);
  const failed = order.phase === "failed";
  const steps = [
    ["Bestellung aufgenommen", `${hhmm(order.created_ms)} · ${order.shots} × ${label(order.target)}`],
    [`Zug fährt zur ${from}`, null],
    [`Betankung an der ${from}`, `${order.poured} von ${order.shots} Shots eingeschenkt`],
    [`Fahrt nach ${to}`, null],
    [`Ankunft ${to}`, "Bitte Shots entnehmen. Prost!"],
  ];
  const list = $("t-steps");
  list.replaceChildren(
    ...steps.map(([title, sub], i) => {
      let cls = "";
      if (order.phase === "arrived" || i < current) cls = "done";
      else if (i === current) cls = failed ? "fail" : "now";
      return el("li", { class: cls }, title, sub ? el("small", {}, sub) : null);
    }),
  );
}

function boardInfo(order) {
  switch (order.phase) {
    case "queued":
      return ["", "Bereitstellung"];
    case "to_refill":
      return ["", "Zug fährt ein"];
    case "fueling":
      return ["", `Betankung ${order.poured}/${order.shots}`];
    case "delivering":
      return ["ok", "Pünktlich"];
    case "arrived":
      return ["ok", "Angekommen"];
    case "failed":
      return ["err", "Fällt aus"];
    default:
      return ["", ""];
  }
}

function renderBoard(orders) {
  const board = $("board");
  const shown = orders.slice(0, 8);
  if (shown.length === 0) {
    board.replaceChildren(el("p", { class: "muted" }, "Keine Bestellungen eingetroffen."));
    return;
  }
  const head = el("div", { class: "board-row head" },
    el("span", {}, "Zeit"), el("span", {}, "Zug"), el("span", {}, "Ziel"), el("span", { class: "info" }, "Hinweis"));
  const rows = shown.map((order) => {
    const [cls, text] = boardInfo(order);
    return el("div", { class: `board-row${order.id === myOrderId ? " mine" : ""}` },
      el("span", {}, hhmm(order.created_ms)),
      el("span", {}, `SE ${order.id}`),
      el("span", { class: "dest" }, `${label(order.target)} · ${order.shots}×`),
      el("span", { class: `info ${cls}` }, text));
  });
  board.replaceChildren(head, ...rows);
}

// ---- polling ----

let pollTimer = null;

async function poll() {
  clearTimeout(pollTimer);
  try {
    const orders = await api("/api/orders");
    renderBoard(orders);
    if (myOrderId != null) {
      const mine = orders.find((o) => o.id === myOrderId);
      if (mine) renderTrip(mine);
      else {
        // server restarted: the order is gone
        saveMyOrder(null);
        $("trip").hidden = true;
      }
    }
  } catch {
    // keep the last picture; the notice bar reports outages
  }
  pollTimer = setTimeout(poll, POLL_MS);
}

async function pollTrain() {
  const notice = $("notice");
  try {
    const state = await api("/api/train/state");
    notice.hidden = false;
    if (state.connection !== "connected") {
      notice.className = "notice";
      notice.textContent = "Verkehrsmeldung: Stellwerksstörung. Bestellungen derzeit nicht möglich.";
    } else if (!state.trainOnline) {
      notice.className = "notice";
      notice.textContent = "Verkehrsmeldung: Der ShotExpress steht derzeit im Depot. Bestellungen fallen aus.";
    } else {
      notice.className = "notice ok";
      notice.textContent = "Der ShotExpress ist betriebsbereit.";
    }
  } catch {
    notice.hidden = false;
    notice.className = "notice";
    notice.textContent = "Verkehrsmeldung: Keine Verbindung zum Fahrplan-Server.";
  }
  setTimeout(pollTrain, 5000);
}

function tickClock() {
  $("now").textContent = hhmm(Date.now());
}

tickClock();
setInterval(tickClock, 10_000);
loadTrack();
poll();
pollTrain();
