"use client";

import type { CSSProperties } from "react";
import { useTrainController } from "../hooks/useTrainController";

export function TrainDashboard() {
  const { connection, lastStatus, events, sendTrainToRaucherecke, isBusy, error } = useTrainController();

  const latestEvent = events.at(-1) ?? null;
  const commandHistory = [...events].slice(-5).reverse();

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "1.5rem" }}>
      <section style={cardStyle}>
        <h2 style={sectionTitleStyle}>Train Control</h2>
        <p style={subtleTextStyle}>Connection: {connectionLabel(connection)}</p>
        {error && (
          <p style={{ marginTop: "0.5rem", backgroundColor: "#fee2e2", padding: "0.75rem", borderRadius: 6, color: "#b91c1c" }}>
            {error}
          </p>
        )}
        <button
          onClick={sendTrainToRaucherecke}
          disabled={isBusy || connection !== "connected"}
          style={{
            marginTop: "1rem",
            display: "inline-flex",
            alignItems: "center",
            justifyContent: "center",
            padding: "0.75rem 1.5rem",
            borderRadius: 6,
            backgroundColor: isBusy || connection !== "connected" ? "#94a3b8" : "#2563eb",
            color: "white",
            border: "none",
            cursor: isBusy || connection !== "connected" ? "not-allowed" : "pointer",
            transition: "background-color 0.15s ease",
          }}
        >
          {isBusy ? "Command in progress…" : "Send train to Raucherecke"}
        </button>
      </section>

      <section style={cardStyle}>
        <h2 style={sectionTitleStyle}>Live Status</h2>
        {lastStatus ? (
          <dl style={{ marginTop: "0.75rem", display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(140px, 1fr))", gap: "0.75rem", fontSize: "0.9rem" }}>
            <div>
              <dt style={subtleTextStyle}>State</dt>
              <dd style={valueTextStyle}>{lastStatus.state}</dd>
            </div>
            <div>
              <dt style={subtleTextStyle}>Battery</dt>
              <dd style={valueTextStyle}>{lastStatus.battery_pct}%</dd>
            </div>
            <div>
              <dt style={subtleTextStyle}>Status uptime</dt>
              <dd style={valueTextStyle}>{formatMillis(lastStatus.status_uptime_ms)}</dd>
            </div>
            <div>
              <dt style={subtleTextStyle}>Last heartbeat</dt>
              <dd style={valueTextStyle}>{formatTimestamp(lastStatus.ts_ms)}</dd>
            </div>
          </dl>
        ) : (
          <p style={subtleTextStyle}>No status messages yet.</p>
        )}
      </section>

      <section style={cardStyle}>
        <h2 style={sectionTitleStyle}>Command Lifecycle</h2>
        {latestEvent ? (
          <div style={{ marginTop: "0.75rem", display: "flex", flexDirection: "column", gap: "0.5rem", fontSize: "0.9rem" }}>
            <p>
              <span style={valueTextStyle}>Latest:</span> {latestEvent.exec_type}
              {"progress" in latestEvent && latestEvent.progress && latestEvent.progress.last_tag && (
                <span>, last tag {latestEvent.progress.last_tag}</span>
              )}
              {"progress" in latestEvent && latestEvent.progress && typeof latestEvent.progress.distance_m === "number" && (
                <span>, distance {latestEvent.progress.distance_m.toFixed(1)} m</span>
              )}
            </p>
            {"error" in latestEvent && latestEvent.error && (
              <p style={{ backgroundColor: "#fee2e2", padding: "0.75rem", borderRadius: 6, color: "#b91c1c" }}>
                Error {latestEvent.error.code}: {latestEvent.error.reason}
              </p>
            )}
            <ul style={{ listStyle: "disc", paddingLeft: "1.25rem", display: "flex", flexDirection: "column", gap: "0.25rem" }}>
              {commandHistory.map((event) => (
                <li key={event.msg_id}>
                  <span style={valueTextStyle}>{event.exec_type}</span> · {formatTimestamp(event.ts_ms)}
                </li>
              ))}
            </ul>
          </div>
        ) : (
          <p style={subtleTextStyle}>No command events yet.</p>
        )}
      </section>
    </div>
  );
}

function connectionLabel(connection: ReturnType<typeof useTrainController>["connection"]) {
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

function formatTimestamp(ts: number) {
  return new Date(ts).toLocaleTimeString();
}

function formatMillis(ms: number) {
  if (ms < 1_000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1_000).toFixed(1)} s`;
  return `${(ms / 60_000).toFixed(1)} min`;
}

const cardStyle: CSSProperties = {
  border: "1px solid #e2e8f0",
  borderRadius: 8,
  padding: "1.25rem",
  boxShadow: "0 1px 2px rgba(15, 23, 42, 0.08)",
  backgroundColor: "white",
};

const sectionTitleStyle: CSSProperties = {
  fontSize: "1.05rem",
  fontWeight: 600,
  margin: 0,
};

const subtleTextStyle: CSSProperties = {
  fontSize: "0.9rem",
  color: "#475569",
  margin: 0,
};

const valueTextStyle: CSSProperties = {
  fontWeight: 600,
  color: "#0f172a",
};
