import { useCallback, useEffect, useRef, useState } from "react";
import type { ExecEvent, StatusMessage } from "../lib/schemas";
import type { ConnectionState, ControllerSnapshot } from "../lib/controllerTypes";

export type ControllerState = {
  connection: ConnectionState;
  lastStatus: StatusMessage | null;
  events: ExecEvent[];
  sendTrainToRaucherecke: () => void;
  isBusy: boolean;
  error: string | null;
};

type StreamEvent =
  | { type: "snapshot"; payload: ControllerSnapshot }
  | { type: "connection"; payload: ConnectionState }
  | { type: "status"; payload: StatusMessage }
  | { type: "events"; payload: ExecEvent[] }
  | { type: "busy"; payload: boolean }
  | { type: "error"; payload: string | null };

const SNAPSHOT_URL = "/api/train/state";
const COMMAND_URL = "/api/train/command";
const STREAM_URL = "/api/train/stream";

export function useTrainController(): ControllerState {
  const [connection, setConnection] = useState<ConnectionState>("connecting");
  const [lastStatus, setLastStatus] = useState<StatusMessage | null>(null);
  const [events, setEvents] = useState<ExecEvent[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [isBusy, setIsBusy] = useState<boolean>(false);

  const eventSourceRef = useRef<EventSource | null>(null);
  const isMountedRef = useRef(true);

  useEffect(() => {
    return () => {
      isMountedRef.current = false;
      eventSourceRef.current?.close();
      eventSourceRef.current = null;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;

    const loadSnapshot = async () => {
      try {
        const response = await fetch(SNAPSHOT_URL, { cache: "no-store" });
        if (!response.ok) {
          throw new Error(`Failed to load train state (${response.status})`);
        }
        const snapshot = (await response.json()) as ControllerSnapshot;
        if (cancelled || !isMountedRef.current) return;
        applySnapshot(snapshot);
      } catch (err) {
        if (cancelled || !isMountedRef.current) return;
        const message = err instanceof Error ? err.message : "Failed to load train state";
        setConnection("error");
        setError(message);
      }
    };

    const applySnapshot = (snapshot: ControllerSnapshot) => {
      setConnection(snapshot.connection);
      setLastStatus(snapshot.lastStatus);
      setEvents(snapshot.events);
      setIsBusy(snapshot.isBusy);
      setError(snapshot.error);
    };

    loadSnapshot();

    return () => {
      cancelled = true;
    };
  }, []);

  const handleStreamEvent = useCallback((event: StreamEvent) => {
    switch (event.type) {
      case "snapshot":
        setConnection(event.payload.connection);
        setLastStatus(event.payload.lastStatus);
        setEvents(event.payload.events);
        setIsBusy(event.payload.isBusy);
        setError(event.payload.error);
        break;
      case "connection":
        setConnection(event.payload);
        break;
      case "status":
        setLastStatus(event.payload);
        break;
      case "events":
        setEvents(event.payload);
        break;
      case "busy":
        setIsBusy(event.payload);
        break;
      case "error":
        setError(event.payload);
        break;
      default:
        break;
    }
  }, []);

  useEffect(() => {
    const source = new EventSource(STREAM_URL);
    eventSourceRef.current = source;

    source.onmessage = (event) => {
      try {
        const parsed = JSON.parse(event.data) as StreamEvent;
        handleStreamEvent(parsed);
      } catch (err) {
        console.warn("[train] failed to parse stream event", err);
      }
    };

    source.addEventListener("heartbeat", () => {
      // heartbeat ensures the connection stays warm; no action needed
    });

    source.onerror = () => {
      setError((prev) => prev ?? "Lost connection to train event stream");
    };

    return () => {
      source.close();
      if (eventSourceRef.current === source) {
        eventSourceRef.current = null;
      }
    };
  }, [handleStreamEvent]);

  const sendTrainToRaucherecke = useCallback(async () => {
    try {
      const response = await fetch(COMMAND_URL, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({}),
      });

      if (!response.ok) {
        const data = (await response.json().catch(() => null)) as { error?: string } | null;
        const message = data?.error ?? `Command failed (${response.status})`;
        throw new Error(message);
      }

      setEvents([]);
      setIsBusy(true);
    } catch (err) {
      const message = err instanceof Error ? err.message : "Failed to send command";
      setError(message);
      return;
    }
  }, []);

  return {
    connection,
    lastStatus,
    events,
    sendTrainToRaucherecke,
    isBusy,
    error,
  };
}
