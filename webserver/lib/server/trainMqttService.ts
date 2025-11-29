import { v4 as uuidv4 } from "uuid";
import { createTrainMqttClient, type TrainMqttClient } from "../mqttClient";
import { createCommandMessage, createEnvelopeFactory, createMoveCommand } from "../messages";
import type { ExecEvent, MoveCommandParameters, StatusMessage } from "../schemas";
import type { ConnectionState, ControllerSnapshot } from "../controllerTypes";

const FINAL_EVENT_TYPES: ExecEvent["exec_type"][] = ["completed", "failed", "cancelled", "expired"];
const DEFAULT_TTL_MS = 60_000;

const DEFAULT_MOVE_PARAMETERS: MoveCommandParameters = {
  target: "raucherecke",
  speed: 0.6,
  direction: "forward",
  expected_tags: ["tag_02", "tag_03", "tag_04", "tag_05", "tag_06"],
  stop_on_tag: "tag_07",
  max_run_ms_without_tag: 7_000,
};

type TrainServiceSubscriber = (event: TrainServiceEvent) => void;

export type TrainServiceEvent =
  | { type: "connection"; payload: ConnectionState }
  | { type: "status"; payload: StatusMessage }
  | { type: "events"; payload: ExecEvent[] }
  | { type: "busy"; payload: boolean }
  | { type: "error"; payload: string | null }
  | { type: "snapshot"; payload: ControllerSnapshot };

class TrainMqttService {
  private readonly client: TrainMqttClient;
  private readonly envelopeFactory = createEnvelopeFactory({});
  private readonly subscribers = new Set<TrainServiceSubscriber>();
  private readonly seenExecMsgIds = new Set<string>();

  private connection: ConnectionState = "connecting";
  private lastStatus: StatusMessage | null = null;
  private events: ExecEvent[] = [];
  private isBusy = false;
  private error: string | null = null;
  private currentCommandId: string | null = null;

  constructor() {
    const url = resolveRequiredEnv("MQTT_URL", "NEXT_PUBLIC_MQTT_URL");
    const username = resolveOptionalEnv("MQTT_USERNAME", "NEXT_PUBLIC_MQTT_USER");
    const password = resolveOptionalEnv("MQTT_PASSWORD", "NEXT_PUBLIC_MQTT_PASS");

    this.client = createTrainMqttClient(url, {
      clean: true,
      reconnectPeriod: 2_000,
      username,
      password,
    });

    this.attachClientListeners();
  }

  getSnapshot(): ControllerSnapshot {
    return {
      connection: this.connection,
      lastStatus: this.lastStatus,
      events: this.events,
      isBusy: this.isBusy,
      error: this.error,
    };
  }

  subscribe(subscriber: TrainServiceSubscriber): () => void {
    this.subscribers.add(subscriber);
    return () => {
      this.subscribers.delete(subscriber);
    };
  }

  async sendMoveCommand(
    parameters: MoveCommandParameters = DEFAULT_MOVE_PARAMETERS,
    ttlMs: number = DEFAULT_TTL_MS,
  ): Promise<void> {
    if (this.isBusy) {
      throw new Error("Train is busy processing another command");
    }

    if (this.connection !== "connected") {
      throw new Error("MQTT connection not ready");
    }

    const cmdId = uuidv4();
    const envelope = this.envelopeFactory(cmdId);
    const command = createMoveCommand(envelope, parameters, ttlMs);
    const message = createCommandMessage(command);

    try {
      this.client.publish(message);
      this.currentCommandId = cmdId;
      this.events = [];
      this.seenExecMsgIds.clear();
      this.updateEvents();
      this.setBusy(true);
      this.setError(null);
    } catch (err) {
      const reason = err instanceof Error ? err.message : "Failed to publish command";
      this.setError(reason);
      this.setBusy(false);
      throw err;
    }
  }

  private attachClientListeners() {
    const { client } = this.client;

    client.on("connect", this.handleConnect);
    client.on("reconnect", this.handleReconnect);
    client.on("close", this.handleClose);
    client.on("error", this.handleError);

    this.client.onStatus(this.handleStatus);
    this.client.onExecEvent(this.handleExecEvent);
  }

  private handleConnect = () => {
    this.updateConnection("connected");
    this.setError(null);
  };

  private handleReconnect = () => {
    this.updateConnection("reconnecting");
  };

  private handleClose = () => {
    this.updateConnection("disconnected");
  };

  private handleError = (error: Error) => {
    this.updateConnection("error");
    this.setError(error.message);
  };

  private handleStatus = (status: StatusMessage) => {
    this.lastStatus = status;
    this.emit({ type: "status", payload: status });
  };

  private handleExecEvent = (event: ExecEvent) => {
    if (this.seenExecMsgIds.has(event.msg_id)) return;
    this.seenExecMsgIds.add(event.msg_id);

    if (this.currentCommandId && event.cmd_id !== this.currentCommandId) {
      return;
    }

    this.events = [...this.events, event];
    this.updateEvents();

    if (FINAL_EVENT_TYPES.includes(event.exec_type)) {
      this.currentCommandId = null;
      this.setBusy(false);
    }
  };

  private updateConnection(connection: ConnectionState) {
    if (this.connection === connection) return;
    this.connection = connection;
    this.emit({ type: "connection", payload: this.connection });
  }

  private updateEvents() {
    this.emit({ type: "events", payload: this.events });
  }

  private setBusy(isBusy: boolean) {
    if (this.isBusy === isBusy) return;
    this.isBusy = isBusy;
    this.emit({ type: "busy", payload: this.isBusy });
  }

  private setError(message: string | null) {
    if (this.error === message) return;
    this.error = message;
    this.emit({ type: "error", payload: this.error });
  }

  private emit(event: TrainServiceEvent) {
    for (const subscriber of this.subscribers) {
      subscriber(event);
    }
  }
}

function resolveRequiredEnv(name: string, fallbackName?: string): string {
  const value = process.env[name] ?? (fallbackName ? process.env[fallbackName] : undefined);
  if (value) {
    if (!process.env[name] && fallbackName && process.env[fallbackName]) {
      console.warn(
        `[train] Using fallback environment variable ${fallbackName}. Please set ${name} to avoid exposing credentials to the client runtime.`,
      );
    }
    return value;
  }
  throw new Error(`Missing required environment variable ${name}`);
}

function resolveOptionalEnv(name: string, fallbackName?: string): string | undefined {
  const value = process.env[name] ?? (fallbackName ? process.env[fallbackName] : undefined);
  if (value && !process.env[name] && fallbackName && process.env[fallbackName]) {
    console.warn(
      `[train] Using fallback environment variable ${fallbackName}. Please set ${name} to avoid exposing credentials to the client runtime.`,
    );
  }
  return value;
}

const globalForTrainService = globalThis as unknown as {
  trainMqttService?: TrainMqttService;
};

export function getTrainMqttService(): TrainMqttService {
  if (!globalForTrainService.trainMqttService) {
    globalForTrainService.trainMqttService = new TrainMqttService();
  }
  return globalForTrainService.trainMqttService;
}

export function getDefaultMoveParameters(): MoveCommandParameters {
  return {
    ...DEFAULT_MOVE_PARAMETERS,
    expected_tags: [...DEFAULT_MOVE_PARAMETERS.expected_tags],
  };
}

export function getDefaultTtlMs() {
  return DEFAULT_TTL_MS;
}
