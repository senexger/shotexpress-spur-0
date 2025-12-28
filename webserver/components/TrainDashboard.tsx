"use client";

import React from "react";
import { useTrainController } from "../hooks/useTrainController";
import styles from "../components/TrainDashboard.module.css";

export function TrainDashboard() {
  const { connection, lastStatus, events, sendTrainToRaucherecke, isBusy, error } = useTrainController();

  const latestEvent = events.at(-1) ?? null;
  const commandHistory = [...events].slice(-5).reverse();

  return (
    <div className={styles.dashboard}>
      <section className={styles.card}>
        <h2 className={styles.sectionTitle}>Train Control</h2>
        <p className={styles.mutedText}>Connection: {connectionLabel(connection)}</p>
        {error && (
          <p className={`${styles.warningBox} ${styles.sectionError}`}>
            {error}
          </p>
        )}
        <button
          onClick={sendTrainToRaucherecke}
          disabled={isBusy || connection !== "connected"}
          className={styles.button}
        >
          {isBusy ? "Command in progress…" : "Send train to Raucherecke"}
        </button>
      </section>

      <section className={styles.card}>
        <h2 className={styles.sectionTitle}>Live Status</h2>
        {lastStatus ? (
          <dl className={styles.statusGrid}>
            <div>
              <dt className={styles.statusTerm}>State</dt>
              <dd className={styles.statusValue}>{lastStatus.state}</dd>
            </div>
            <div>
              <dt className={styles.statusTerm}>Battery</dt>
              <dd className={styles.statusValue}>{lastStatus.battery_pct}%</dd>
            </div>
            <div>
              <dt className={styles.statusTerm}>Status uptime</dt>
              <dd className={styles.statusValue}>{formatMillis(lastStatus.status_uptime_ms)}</dd>
            </div>
            <div>
              <dt className={styles.statusTerm}>Last heartbeat</dt>
              <dd className={styles.statusValue}>{formatTimestamp(lastStatus.ts_ms)}</dd>
            </div>
          </dl>
        ) : (
          <p className={styles.mutedText}>No status messages yet.</p>
        )}
      </section>

      <section className={styles.card}>
        <h2 className={styles.sectionTitle}>Command Lifecycle</h2>
        {latestEvent ? (
          <div className={styles.latestDetails}>
            <p className={styles.latestSummary}>
              <span className={styles.valueText}>Latest:</span> {latestEvent.exec_type}
              {"progress" in latestEvent && latestEvent.progress && latestEvent.progress.last_tag && (
                <span>, last tag {latestEvent.progress.last_tag}</span>
              )}
              {"progress" in latestEvent && latestEvent.progress && typeof latestEvent.progress.distance_m === "number" && (
                <span>, distance {latestEvent.progress.distance_m.toFixed(1)} m</span>
              )}
            </p>
            {"error" in latestEvent && latestEvent.error && (
              <p className={styles.warningBox}>
                Error {latestEvent.error.code}: {latestEvent.error.reason}
              </p>
            )}
            <ul className={styles.commandList}>
              {commandHistory.map((event) => (
                <li key={event.msg_id}>
                  <span className={styles.valueText}>{event.exec_type}</span> · {formatTimestamp(event.ts_ms)}
                </li>
              ))}
            </ul>
          </div>
        ) : (
          <p className={styles.mutedText}>No command events yet.</p>
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
