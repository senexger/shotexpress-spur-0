"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import { v4 } from 'uuid';
import mqtt, { IClientOptions, MqttClient } from "mqtt";

type Status = {
  ack_id?: string;
  motor: { speed: number; direction: "forward"|"reverse"; state: "running"|"stopped"|"error" };
  battery_pct?: number;
  last_tag?: { id: string; ts_ms: number; rssi?: number };
  error?: string | null;
};

type Destination = {
  id: string;
  name: string;
  tagId: string;
  description: string;
};

const destinations: Destination[] = [
  { id: "bar", name: "Bar", tagId: "tag_01", description: "Starting position - Bar area" },
  { id: "schachbrett", name: "Schachbrett", tagId: "tag_03", description: "Chess board area" },
  { id: "name_vergessen", name: "Name Vergessen", tagId: "tag_05", description: "The forgotten name location" },
  { id: "raucherecke", name: "Raucherecke", tagId: "tag_07", description: "Smoking corner" }
];

const base = `shotexpress`;

export default function Page() {
  const [connected, setConnected] = useState(false);
  const [presence, setPresence] = useState<string>("offline");
  const [status, setStatus] = useState<Status | null>(null);
  const [speed, setSpeed] = useState(0.6);
  const [direction, setDirection] = useState<"forward"|"reverse">("forward");
  const [selectedDestination, setSelectedDestination] = useState<Destination>(destinations[3]); // Default to raucherecke
  const [showConfirmation, setShowConfirmation] = useState(false);
  const [prevCmdId, setPrevCmdId] = useState(0); // Track previous command sequence ID
  const clientRef = useRef<MqttClient | null>(null);

  const opts: IClientOptions = useMemo(() => ({
    clean: true,
    keepalive: 30,
    reconnectPeriod: 2000,
    username: process.env.NEXT_PUBLIC_MQTT_USER,
    password: process.env.NEXT_PUBLIC_MQTT_PASS,
    protocolVersion: 4
  }), []);

  useEffect(() => {
    const url = process.env.NEXT_PUBLIC_MQTT_URL!;
    const c = mqtt.connect(url, opts);
    clientRef.current = c;

    c.on("connect", () => setConnected(true));
    c.on("reconnect", () => setConnected(false));
    c.on("close", () => setConnected(false));
    c.on("error", () => setConnected(false));

    c.subscribe(`${base}/presence`, { qos: 1 });
    c.subscribe(`${base}/status`, { qos: 0 });

    c.on("message", (topic: string, payload: Buffer) => {
      if (topic.endsWith("/presence")) setPresence(payload.toString());
      if (topic.endsWith("/status")) {
        try { setStatus(JSON.parse(payload.toString())); } catch {}
      }
    });

    return () => { c.end(true); };
  }, [opts]);

  function publishCmd(cmd: any) {
    const c = clientRef.current;
    if (!c || !connected) return;
    c.publish(`${base}/command`, JSON.stringify(cmd), { qos: 1 });
  }

  function sendMoveToDestination() {
    const now = Date.now();
    
    // Calculate expected tags and stop tag based on destination
    let expectedTags: string[] = [];
    let stopOnTag = selectedDestination.tagId;
    
    // Determine route based on current position and destination
    // For simplicity, assume forward route: bar -> schachbrett -> name_vergessen -> raucherecke
    switch (selectedDestination.id) {
      case "bar":
        expectedTags = [];
        break;
      case "schachbrett":
        expectedTags = ["tag_02"];
        break;
      case "name_vergessen":
        expectedTags = ["tag_02", "tag_03", "tag_04"];
        break;
      case "raucherecke":
        expectedTags = ["tag_02", "tag_03", "tag_04", "tag_05", "tag_06"];
        break;
    }

    const cmd = {
      id: v4(),
      seq: prevCmdId + 1,
      ts_ms: now,
      type: "move_to",
      params: {
        target: selectedDestination.id,
        speed,
        direction,
        expected_tags: expectedTags,
        stop_on_tag: stopOnTag,
        offline_plan: {
          approach_slowdown_ms: 2000,
          max_run_ms_without_tag: 7000,
          crawl_speed: 0.15,
          dwell_ms: 1500
        },
        ttl_ms: 15000
      }
    };
    publishCmd(cmd);
    setPrevCmdId(prevCmdId + 1); // Increment the command sequence ID
    setShowConfirmation(false);
  }

  function sendStop() {
    const now = Date.now();
    publishCmd({
      v: 1, id: `cmd_${now}`, seq: prevCmdId + 1, ts_ms: now,
      type: "stop", params: {}
    });
    setPrevCmdId(prevCmdId + 1); // Increment the command sequence ID
  }

  return (
    <main style={{ maxWidth: 720, margin: "2rem auto", fontFamily: "system-ui, sans-serif" }}>
      <h1>ShotExpress</h1>
      <p>Broker: {connected ? "connected" : "disconnected"} · Train: {presence}</p>

      <section style={{ padding: "1rem", border: "1px solid #ddd", borderRadius: 8, marginTop: 12 }}>
        <h2>Select Destination</h2>
        
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: "1rem", marginBottom: "1rem" }}>
          {destinations.map((dest) => (
            <button
              key={dest.id}
              onClick={() => setSelectedDestination(dest)}
              style={{
                padding: "1rem",
                border: selectedDestination.id === dest.id ? "2px solid #007acc" : "1px solid #ddd",
                borderRadius: 8,
                backgroundColor: selectedDestination.id === dest.id ? "#f0f8ff" : "#fff",
                cursor: "pointer",
                textAlign: "left"
              }}
            >
              <div style={{ fontWeight: "bold", fontSize: "1.1em" }}>{dest.name}</div>
              <div style={{ fontSize: "0.9em", color: "#666", marginTop: "0.25rem" }}>
                {dest.description}
              </div>
              <div style={{ fontSize: "0.8em", color: "#999", marginTop: "0.25rem" }}>
                Tag: {dest.tagId}
              </div>
            </button>
          ))}
        </div>

        <div style={{ marginBottom: "1rem" }}>
          <label>Direction&nbsp;
            <select 
              value={direction} 
              onChange={(e) => setDirection((e.target as HTMLSelectElement).value as "forward" | "reverse")}
            >
              <option value="forward">forward</option>
              <option value="reverse">reverse</option>
            </select>
          </label>
          <br />
          <label>Speed {speed.toFixed(2)}
            <input 
              type="range" 
              min="0" 
              max="1" 
              step="0.01"
              value={speed}
              onChange={(e) => setSpeed(parseFloat((e.target as HTMLInputElement).value))} 
            />
          </label>
        </div>

        {!showConfirmation ? (
          <button 
            onClick={() => setShowConfirmation(true)} 
            disabled={!connected}
            style={{
              padding: "0.75rem 1.5rem",
              backgroundColor: "#007acc",
              color: "white",
              border: "none",
              borderRadius: 4,
              fontSize: "1em",
              cursor: connected ? "pointer" : "not-allowed"
            }}
          >
            Go to {selectedDestination.name}
          </button>
        ) : (
          <div style={{ padding: "1rem", backgroundColor: "#fff3cd", border: "1px solid #ffeaa7", borderRadius: 4 }}>
            <p style={{ margin: "0 0 1rem 0" }}>
              <strong>Confirm:</strong> Send train to <strong>{selectedDestination.name}</strong>?
            </p>
            <button 
              onClick={sendMoveToDestination}
              style={{
                padding: "0.5rem 1rem",
                backgroundColor: "#28a745",
                color: "white",
                border: "none",
                borderRadius: 4,
                marginRight: "0.5rem",
                cursor: "pointer"
              }}
            >
              Confirm
            </button>
            <button 
              onClick={() => setShowConfirmation(false)}
              style={{
                padding: "0.5rem 1rem",
                backgroundColor: "#6c757d",
                color: "white",
                border: "none",
                borderRadius: 4,
                cursor: "pointer"
              }}
            >
              Cancel
            </button>
          </div>
        )}

        <button 
          onClick={sendStop} 
          style={{ 
            marginLeft: 8, 
            padding: "0.75rem 1.5rem",
            backgroundColor: "#dc3545",
            color: "white",
            border: "none",
            borderRadius: 4,
            cursor: connected ? "pointer" : "not-allowed"
          }} 
          disabled={!connected}
        >
          Emergency Stop
        </button>
      </section>

      <section style={{ padding: "1rem", border: "1px solid #ddd", borderRadius: 8, marginTop: 12 }}>
        <h2>Status</h2>
        {!status ? <p>No status yet.</p> :
          <pre style={{ whiteSpace: "pre-wrap" }}>{JSON.stringify(status, null, 2)}</pre>}
      </section>
    </main>
  );
}
