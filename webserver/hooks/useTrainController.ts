import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createTrainMqttClient, type TrainMqttClient } from "../lib/mqttClient";
import { createEnvelopeFactory, createCommandMessage, createMoveCommand } from "../lib/messages";
import type { ExecEvent, MoveCommandParameters, StatusMessage } from "../lib/schemas";
import { v4 as uuidv4 } from "uuid";

const MQTT_URL = process.env.NEXT_PUBLIC_MQTT_URL;
const MQTT_USERNAME = process.env.NEXT_PUBLIC_MQTT_USER;
const MQTT_PASSWORD = process.env.NEXT_PUBLIC_MQTT_PASS;

export type ConnectionState = "connecting" | "connected" | "reconnecting" | "disconnected" | "error";

export type ControllerState = {
  connection: ConnectionState;
  lastStatus: StatusMessage | null;
  events: ExecEvent[];
  sendTrainToRaucherecke: () => void;
  isBusy: boolean;
  error: string | null;
};

const FINAL_EVENT_TYPES: ExecEvent["exec_type"][] = ["completed", "failed", "cancelled", "expired"];

export function useTrainController(): ControllerState {
  const [connection, setConnection] = useState<ConnectionState>("connecting");
  const [lastStatus, setLastStatus] = useState<StatusMessage | null>(null);
  const [events, setEvents] = useState<ExecEvent[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [isBusy, setIsBusy] = useState<boolean>(false);

  const clientRef = useRef<TrainMqttClient | null>(null);
  const disconnectRef = useRef<(() => void) | null>(null);
  const currentCommandIdRef = useRef<string | null>(null);
  const seenExecMsgIds = useRef<Set<string>>(new Set());
  const envelopeFactory = useMemo(() => createEnvelopeFactory({}), []);

  useEffect(() => {
    if (!MQTT_URL) {
      setConnection("error");
      setError("Missing NEXT_PUBLIC_MQTT_URL environment variable");
      return;
    }

    const client = createTrainMqttClient(MQTT_URL, {
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

    client.client.on("connect", handleConnect);
    client.client.on("reconnect", handleReconnect);
    client.client.on("close", handleClose);
    client.client.on("error", handleError);

    const unsubscribeStatus = client.onStatus((status) => {
      setLastStatus(status);
    });

    const unsubscribeExec = client.onExecEvent((event) => {
      if (seenExecMsgIds.current.has(event.msg_id)) return;
      seenExecMsgIds.current.add(event.msg_id);

      if (currentCommandIdRef.current && event.cmd_id !== currentCommandIdRef.current) {
        return;
      }

      setEvents((prev) => [...prev, event]);

      if (FINAL_EVENT_TYPES.includes(event.exec_type)) {
        setIsBusy(false);
        currentCommandIdRef.current = null;
      }
    });

    disconnectRef.current = () => {
      unsubscribeStatus();
      unsubscribeExec();
      client.client.off("connect", handleConnect);
      client.client.off("reconnect", handleReconnect);
      client.client.off("close", handleClose);
      client.client.off("error", handleError);
      client.disconnect(true);
    };

    return () => {
      disconnectRef.current?.();
      disconnectRef.current = null;
      clientRef.current = null;
    };
  }, []);

  const sendTrainToRaucherecke = useCallback(() => {
    const client = clientRef.current;
    if (!client) {
      setError("MQTT client not ready");
      return;
    }

    const cmd_id = uuidv4();
    const envelope = envelopeFactory(cmd_id);

    const params: MoveCommandParameters = {
      target: "raucherecke",
      speed: 0.6,
      direction: "forward",
      expected_tags: ["tag_02", "tag_03", "tag_04", "tag_05", "tag_06"],
      stop_on_tag: "tag_07",
      offline_plan: {
        approach_slowdown_ms: 2_000,
        max_run_ms_without_tag: 7_000,
        crawl_speed: 0.15,
        dwell_ms: 1_500,
      },
    };

    const command = createMoveCommand(envelope, params, 60_000);
    const message = createCommandMessage(command);

    try {
      client.publish(message);
      currentCommandIdRef.current = cmd_id;
      setEvents([]);
      setIsBusy(true);
      setError(null);
    } catch (err) {
      const reason = err instanceof Error ? err.message : "Failed to publish command";
      setError(reason);
    }
  }, [envelopeFactory]);

  return {
    connection,
    lastStatus,
    events,
    sendTrainToRaucherecke,
    isBusy,
    error,
  };
}
