import { useCallback, useEffect, useRef, useState } from "react";
import mqtt from "mqtt";
import { MQTT_TOPICS } from "../lib/mqttTopics";

const MQTT_URL = process.env.NEXT_PUBLIC_MQTT_URL;
const MQTT_USERNAME = process.env.NEXT_PUBLIC_MQTT_USER;
const MQTT_PASSWORD = process.env.NEXT_PUBLIC_MQTT_PASS;

export type ConnectionState = "connecting" | "connected" | "reconnecting" | "disconnected" | "error";

export type ControllerState = {
  connection: ConnectionState;
  sendCommand: (command: 0 | 1 | 2) => void;
  error: string | null;
};

export function useTrainController(): ControllerState {
  const [connection, setConnection] = useState<ConnectionState>("connecting");
  const [error, setError] = useState<string | null>(null);

  const clientRef = useRef<mqtt.MqttClient | null>(null);

  useEffect(() => {
    if (!MQTT_URL) {
      setConnection("error");
      setError("Missing NEXT_PUBLIC_MQTT_URL environment variable");
      return;
    }

    const client = mqtt.connect(MQTT_URL, {
      clean: true,
      reconnectPeriod: 2_000,
      username: MQTT_USERNAME,
      password: MQTT_PASSWORD,
    });

    clientRef.current = client;

    const handleConnect = () => {
      setConnection("connected");
      setError(null);
    };
    const handleReconnect = () => {
      setConnection("reconnecting");
      setError(null);
    };
    const handleClose = () => setConnection("disconnected");
    const handleError = (err: Error) => {
      setConnection("error");
      setError(err.message);
    };

    client.on("connect", handleConnect);
    client.on("reconnect", handleReconnect);
    client.on("close", handleClose);
    client.on("error", handleError);

    return () => {
      client.off("connect", handleConnect);
      client.off("reconnect", handleReconnect);
      client.off("close", handleClose);
      client.off("error", handleError);
      client.end(true);
      clientRef.current = null;
    };
  }, []);

  const sendCommand = useCallback((command: 0 | 1 | 2) => {
    const client = clientRef.current;
    if (!client) {
      setError("MQTT client not ready");
      return;
    }

    try {
      client.publish(MQTT_TOPICS.command, command.toString(), { qos: 1 });
      setError(null);
    } catch (err) {
      const reason = err instanceof Error ? err.message : "Failed to publish command";
      setError(reason);
    }
  }, []);

  return {
    connection,
    sendCommand,
    error,
  };
}
