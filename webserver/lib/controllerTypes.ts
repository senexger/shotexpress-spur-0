import type { ExecEvent, StatusMessage } from "./schemas";

export type ConnectionState = "connecting" | "connected" | "reconnecting" | "disconnected" | "error";

export type ControllerSnapshot = {
  connection: ConnectionState;
  lastStatus: StatusMessage | null;
  events: ExecEvent[];
  isBusy: boolean;
  error: string | null;
};
