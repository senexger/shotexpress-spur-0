"use client";

import type { CSSProperties } from "react";
import { useTrainController } from "../hooks/useTrainController";

export function TrainDashboard() {
  const { connection, sendCommand, error } = useTrainController();

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "1.5rem", alignItems: "center", justifyContent: "center", minHeight: "100vh" }}>
      <section style={cardStyle}>
        <h2 style={sectionTitleStyle}>Train Control</h2>
        <p style={subtleTextStyle}>Connection: {connectionLabel(connection)}</p>
        {error && (
          <p style={{ marginTop: "0.5rem", backgroundColor: "#fee2e2", padding: "0.75rem", borderRadius: 6, color: "#b91c1c" }}>
            {error}
          </p>
        )}
        <div style={{ marginTop: "1.5rem", display: "flex", flexDirection: "column", gap: "1rem" }}>
          <button
            onClick={() => sendCommand(1)}
            disabled={connection !== "connected"}
            style={{
              ...buttonStyle,
              backgroundColor: connection !== "connected" ? "#94a3b8" : "#10b981",
            }}
          >
            Forward
          </button>
          <button
            onClick={() => sendCommand(0)}
            disabled={connection !== "connected"}
            style={{
              ...buttonStyle,
              backgroundColor: connection !== "connected" ? "#94a3b8" : "#ef4444",
            }}
          >
            Stop
          </button>
          <button
            onClick={() => sendCommand(2)}
            disabled={connection !== "connected"}
            style={{
              ...buttonStyle,
              backgroundColor: connection !== "connected" ? "#94a3b8" : "#f59e0b",
            }}
          >
            Backward
          </button>
        </div>
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

const cardStyle: CSSProperties = {
  border: "1px solid #e2e8f0",
  borderRadius: 8,
  padding: "2rem",
  boxShadow: "0 1px 2px rgba(15, 23, 42, 0.08)",
  backgroundColor: "white",
  minWidth: "320px",
};

const sectionTitleStyle: CSSProperties = {
  fontSize: "1.5rem",
  fontWeight: 600,
  margin: 0,
  textAlign: "center",
};

const subtleTextStyle: CSSProperties = {
  fontSize: "0.9rem",
  color: "#475569",
  margin: 0,
  textAlign: "center",
};

const buttonStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
  padding: "1rem 2rem",
  borderRadius: 6,
  color: "white",
  border: "none",
  cursor: "pointer",
  transition: "background-color 0.15s ease",
  fontSize: "1.1rem",
  fontWeight: 600,
};
