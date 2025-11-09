"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import { connect, IClientOptions, MqttClient } from "mqtt";

type Status = {
  v: number; seq: number; ts_ms: number;
  ack_id?: string;
  motor: { speed: number; direction: "forward"|"reverse"; state: "running"|"stopped"|"error" };
  battery_pct?: number;
  last_tag?: { id: string; ts_ms: number; rssi?: number };
  error?: string | null;
};

const trainId = process.env.NEXT_PUBLIC_TRAIN_ID!;
const base = `shotexpress/${trainId}`;

export default function Page() {
  const [connected, setConnected] = useState(false);
  const [presence, setPresence] = useState<string>("offline");
  const [status, setStatus] = useState<Status | null>(null);
  const [speed, setSpeed] = useState(0.6);
  const [direction, setDirection] = useState<"forward"|"reverse">("forward");
  const [target, setTarget] = useState("raucherecke");
  const [stopOn, setStopOn] = useState("tag_06");
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
    const c = connect(url, opts);
    clientRef.current = c;

    c.on("connect", () => setConnected(true));
    c.on("reconnect", () => setConnected(false));
    c.on("close", () => setConnected(false));
    c.on("error", () => setConnected(false));

    c.subscribe(`${base}/presence`, { qos: 1 });
    c.subscribe(`${base}/status`, { qos: 0 });

    c.on("message", (topic, payload) => {
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
    c.publish(`${base}/cmd`, JSON.stringify(cmd), { qos: 1 });
  }

  function sendMove() {
    const now = Date.now();
    const cmd = {
      v: 1,
      id: `cmd_${now}`,
      seq: now,    // simple unique monotonic for demo
      ts_ms: now,
      type: "move_to",
      params: {
        target,
        speed,
        direction,
        expected_tags: ["tag_03", "tag_04", "tag_05"],
        stop_on_tag: stopOn,
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
  }

  function sendStop() {
    const now = Date.now();
    publishCmd({
      v: 1, id: `cmd_${now}`, seq: now, ts_ms: now,
      type: "stop", params: {}
    });
  }

  return (
    <main style={{ maxWidth: 720, margin: "2rem auto", fontFamily: "system-ui, sans-serif" }}>
      <h1>ShotExpress</h1>
      <p>Broker: {connected ? "connected" : "disconnected"} · Train: {presence}</p>

      <section style={{ padding: "1rem", border: "1px solid #ddd", borderRadius: 8, marginTop: 12 }}>
        <h2>Move to</h2>
        <label>Target&nbsp;
          <input value={target} onChange={e => setTarget(e.target.value)} />
        </label>
        <br />
        <label>Stop on tag&nbsp;
          <input value={stopOn} onChange={e => setStopOn(e.target.value)} />
        </label>
        <br />
        <label>Direction&nbsp;
          <select value={direction} onChange={e => setDirection(e.target.value as any)}>
            <option value="forward">forward</option>
            <option value="reverse">reverse</option>
          </select>
        </label>
        <br />
        <label>Speed {speed.toFixed(2)}
          <input type="range" min="0" max="1" step="0.01"
                 value={speed}
                 onChange={e => setSpeed(parseFloat(e.target.value))} />
        </label>
        <br />
        <button onClick={sendMove} disabled={!connected}>Send move_to</button>
        <button onClick={sendStop} style={{ marginLeft: 8 }} disabled={!connected}>Stop</button>
      </section>

      <section style={{ padding: "1rem", border: "1px solid #ddd", borderRadius: 8, marginTop: 12 }}>
        <h2>Status</h2>
        {!status ? <p>No status yet.</p> :
          <pre style={{ whiteSpace: "pre-wrap" }}>{JSON.stringify(status, null, 2)}</pre>}
      </section>
    </main>
  );
}
