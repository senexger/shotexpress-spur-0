import { v4 as uuidv4 } from "uuid";
import { MQTT_TOPICS } from "./mqttTopics";
import type {Command, BaseMessage, PublishableMessage, Envelope } from "./schemas";
import { StatusMessage, StopCommand, WaitForLoadCommand, MoveCommandParameters, MoveCommand, ContinueCommand, ExecEvent } from "./schemas";

type EnvelopeFactoryOptions = {
  initialSeq?: number;
};

function stringifyMessage<T extends BaseMessage>(message: T, topic: PublishableMessage["topic"]): PublishableMessage {
  return {
    topic,
    payload: JSON.stringify(message),
  };
}

export function createEnvelopeFactory({ initialSeq = 0 }: EnvelopeFactoryOptions) {
  let seq = initialSeq;
  return (cmd_id: string) => ({
    msg_id: uuidv4(),
    cmd_id,
    seq: ++seq,
    ts_ms: Date.now(),
  } satisfies Envelope);
};

export function createStopCommand(envelope: Envelope): StopCommand {
  return {
    ...envelope,
    cmd_type: "stop",
  } satisfies StopCommand;
};

export function createWaitCommand(envelope: Envelope, wait_time_ms: number, ttl_ms: number): WaitForLoadCommand {
  return {
    ...envelope,
    cmd_type: "wait_for_load",
    wait_time_ms,
    ttl_ms,
  } satisfies WaitForLoadCommand;
};

export function createMoveCommand(envelope: Envelope, moveParameters: MoveCommandParameters, ttl_ms: number): MoveCommand {
  return {
    ...envelope,
    cmd_type: "move_to",
    params: moveParameters,
    ttl_ms,
  } satisfies MoveCommand;
};

export function createContinueCommand(envelope: Envelope, moveParameters: MoveCommandParameters, ttl_ms: number): ContinueCommand {
  return {
    ...envelope,
    cmd_type: "continue",
    params: moveParameters,
    ttl_ms,
  } satisfies ContinueCommand;
};

export function createCommandMessage(command: Command): PublishableMessage {
  return stringifyMessage(command, MQTT_TOPICS.command);
}

export function createEventExecMessage(event: ExecEvent): PublishableMessage {
  return stringifyMessage(event, MQTT_TOPICS.eventExec);
}

type StatusOptions = {
  state: StatusMessage["state"];
  battery_pct: number;
  status_uptime_ms: number;
};

export function createStatus(envelope: Envelope, options: StatusOptions): StatusMessage {
  return {
    ...envelope,
    state: options.state,
    battery_pct: options.battery_pct,
    status_uptime_ms: options.status_uptime_ms,
  } satisfies ContinueCommand;
};

export function createStatusMessage(status: StatusMessage): PublishableMessage {
  return stringifyMessage(status, MQTT_TOPICS.status);
}
