import type { IClientOptions, MqttClient } from "mqtt";
import { createTrainMqttClient, type TrainMqttClient } from "./mqttClient";
import { createEnvelopeFactory, createEventExecMessage, createStatusMessage, createStatus } from "./messages";
import type { Command, MoveCommand, ExecEvent, TrainState } from "./schemas";
import {trainStateSchema } from "./schemas";
import dotenv from "dotenv";
dotenv.config({ path: ".env.local" });

type FakeExpressOptions = {
  url?: string;
  username?: string;
  password?: string;
  heartbeatIntervalMs?: number;
  clientOptions?: IClientOptions;
  logger?: Pick<Console, "info" | "warn" | "error">;
};

export type FakeExpressController = {
  stop(): Promise<void>;
};

type CommandQueueHandle = NodeJS.Timeout;

const DEFAULT_HEARTBEAT_MS = 1_000;

export async function startFakeExpress(options: FakeExpressOptions = {}): Promise<FakeExpressController> {
  const logger = options.logger ?? console;
  const url =
    options.url ?? process.env.FAKE_EXPRESS_MQTT_URL ?? process.env.NEXT_PUBLIC_MQTT_URL ?? process.env.MQTT_URL;

  if (!url) {
    throw new Error("FakeExpress requires an MQTT URL. Provide options.url or set FAKE_EXPRESS_MQTT_URL.");
  }

  const mqttClient = createTrainMqttClient(url, {
    username: options.username ?? process.env.FAKE_EXPRESS_MQTT_USER ?? process.env.NEXT_PUBLIC_MQTT_USER,
    password: options.password ?? process.env.FAKE_EXPRESS_MQTT_PASS ?? process.env.NEXT_PUBLIC_MQTT_PASS,
    autoSubscribe: false,
    ...options.clientOptions,
  });

  await waitForConnect(mqttClient.client);
  logger.info?.(`[fake_express] connected to ${url}`);

  const simulation = new FakeExpressRuntime(mqttClient, {
    heartbeatIntervalMs: options.heartbeatIntervalMs ?? DEFAULT_HEARTBEAT_MS,
    logger,
  });

  simulation.start();

  return {
    stop: () => simulation.stop(),
  };
}

class FakeExpressRuntime {
  private readonly envelopeFactory = createEnvelopeFactory({});
  private readonly seenMessageIds = new Set<string>();
  private readonly timeouts = new Set<CommandQueueHandle>();
  private heartbeatTimer: NodeJS.Timeout | null = null;
  private currentState: TrainState = "IDLE";
  private stateSince = Date.now();
  private currentCommandId: string = '';
  private readonly logger: Pick<Console, "info" | "warn" | "error">;
  private unsubscribe: (() => void) | null = null;
  private batteryPct = 86;

  constructor(
    private readonly client: TrainMqttClient,
    private readonly options: { heartbeatIntervalMs: number; logger: Pick<Console, "info" | "warn" | "error"> },
  ) {
    this.logger = options.logger;
  }

  start() {
    this.unsubscribe = this.client.onCommand((command) => this.handleCommand(command));
    this.publishStatus();
    this.startHeartbeat();
  }

  async stop() {
    this.unsubscribe?.();
    this.unsubscribe = null;
    this.clearScheduled();
    this.stopHeartbeat();
    await this.client.disconnect(true);
    this.logger.info?.("[fake_express] stopped");
  }

  private handleCommand(command: Command) {
    if (this.seenMessageIds.has(command.msg_id)) {
      this.logger.info?.(`[fake_express] ignoring duplicate command ${command.msg_id}`);
      return;
    }

    this.seenMessageIds.add(command.msg_id);

    if (command.cmd_type !== "move_to") {
      this.publishExec({
        ...this.envelopeFactory(command.cmd_id),
        exec_type: "failed",
        error: {
          code: "UNSUPPORTED_COMMAND",
          reason: `Command ${command.cmd_type} is not supported by FakeExpress`,
        },
      });
      return;
    }

    const now = Date.now();
    if (now > command.ts_ms + command.ttl_ms) {
      this.publishExec({
        ...this.envelopeFactory(command.cmd_id),
        exec_type: "expired",
      });
      return;
    }

    if (command.params.target.toLowerCase() !== "raucherecke") {
      this.publishExec({
        ...this.envelopeFactory(command.cmd_id),
        exec_type: "failed",
        error: {
          code: "UNROUTABLE",
          reason: `FakeExpress only knows how to reach raucherecke, received ${command.params.target}`,
        },
      });
      return;
    }

    this.logger.info?.(`[fake_express] executing command ${command.msg_id} to raucherecke`);
    this.executeMoveToRaucherecke(command);
  }

  private executeMoveToRaucherecke(command: MoveCommand) {
    this.clearScheduled();
    this.currentCommandId = command.cmd_id;

    this.publishExec({
      ...this.envelopeFactory(command.cmd_id),
      exec_type: "accepted",
    });

    this.queue(() => {
      this.publishExec({
        ...this.envelopeFactory(command.cmd_id),
        exec_type: "started",
      });
      this.updateState("RUNNING");
    }, 300);

    this.queue(() => {
      this.publishExec({
        ...this.envelopeFactory(command.cmd_id),
        exec_type: "progress",
        direction: "forward",  // hardcode for now
        progress: {
          last_tag: "tag_03",
          distance_m: 2.1,
        },
      });
    }, 3_400);

    this.queue(() => {
      this.updateState("APPROACH");
      this.publishExec({
        ...this.envelopeFactory(command.cmd_id),
        exec_type: "progress",
        direction: "forward",  // hardcode for now
        progress: {
          last_tag: "tag_06",
          distance_m: 4.2,
        },
      });
    }, 5_600);

    this.queue(() => {
      this.updateState("DWELL");
      this.publishExec({
        ...this.envelopeFactory(command.cmd_id),
        exec_type: "completed",
      });
    }, 5_600);

    this.queue(() => {
      this.updateState("IDLE");
      this.currentCommandId = '';
    }, 5_600 + command.params.offline_plan.dwell_ms);
  }

  private publishExec(execEvent: ExecEvent) {
    const message = createEventExecMessage(execEvent);
    this.client.publish(message);
  }

  private updateState(newState: TrainState) {
    if (!trainStateSchema.safeParse(newState).success) return;
    this.currentState = newState;
    this.stateSince = Date.now();
    this.publishStatus();
  }

  private publishStatus() {
    const uptime = Date.now() - this.stateSince;
    const status = createStatus(
      this.envelopeFactory(this.currentCommandId),
      {
        state: this.currentState,
        battery_pct: this.batteryPct,
        status_uptime_ms: uptime,
      }
    );
    this.client.publish(createStatusMessage(status));
  }

  private startHeartbeat() {
    if (this.heartbeatTimer) return;
    this.heartbeatTimer = setInterval(() => this.publishStatus(), this.options.heartbeatIntervalMs);
  }

  private stopHeartbeat() {
    if (this.heartbeatTimer) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }

  private queue(callback: () => void, delayMs: number) {
    const timeout = setTimeout(() => {
      this.timeouts.delete(timeout);
      callback();
    }, delayMs);
    this.timeouts.add(timeout);
  }

  private clearScheduled() {
    this.timeouts.forEach((timeout) => clearTimeout(timeout));
    this.timeouts.clear();
  }
}

function waitForConnect(client: MqttClient): Promise<void> {
  if (client.connected) return Promise.resolve();
  return new Promise((resolve, reject) => {
    const onConnect = () => {
      cleanup();
      resolve();
    };
    const onError = (err: Error) => {
      cleanup();
      reject(err);
    };
    const cleanup = () => {
      client.off("connect", onConnect);
      client.off("error", onError);
    };
    client.once("connect", onConnect);
    client.once("error", onError);
  });
}
