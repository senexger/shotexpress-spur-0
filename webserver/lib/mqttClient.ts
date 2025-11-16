import mqtt, { IClientOptions, IClientPublishOptions, MqttClient } from "mqtt";
import { statusSchema, type StatusMessage, commandSchema, type Command, eventExecSchema, type ExecEvent} from "./schemas";
import { MQTT_QOS, MQTT_TOPICS, type MqttTopic } from "./mqttTopics";
import type { PublishableMessage } from "./schemas";

type Handler<T> = (message: T) => void;

type HandlerMap = {
  [MQTT_TOPICS.command]: Set<Handler<Command>>;
  [MQTT_TOPICS.eventExec]: Set<Handler<ExecEvent>>;
  [MQTT_TOPICS.status]: Set<Handler<StatusMessage>>;
};

const schemaByTopic = {
  [MQTT_TOPICS.command]: commandSchema,
  [MQTT_TOPICS.eventExec]: eventExecSchema,
  [MQTT_TOPICS.status]: statusSchema,
} as const;

type SchemaByTopic = typeof schemaByTopic;

export type TrainMqttClient = {
  client: MqttClient;
  publish(message: PublishableMessage, options?: IClientPublishOptions): void;
  onCommand(handler: Handler<Command>): () => void;
  onExecEvent(handler: Handler<ExecEvent>): () => void;
  onStatus(handler: Handler<StatusMessage>): () => void;
  disconnect(force?: boolean): Promise<void>;
};

type CreateTrainMqttClientOptions = IClientOptions & {
  autoSubscribe?: boolean;
};

export function createTrainMqttClient(url: string, options: CreateTrainMqttClientOptions = {}): TrainMqttClient {
  const { autoSubscribe = true, ...clientOptions } = options;
  const client = mqtt.connect(url, clientOptions);

  const handlers: HandlerMap = {
    [MQTT_TOPICS.command]: new Set(),
    [MQTT_TOPICS.eventExec]: new Set(),
    [MQTT_TOPICS.status]: new Set(),
  };

  const subscribeToTopic = (topic: MqttTopic) => {
    client.subscribe(topic, { qos: MQTT_QOS[getTopicKey(topic)] });
  };

  if (autoSubscribe) {
    (Object.values(MQTT_TOPICS) as MqttTopic[]).forEach(subscribeToTopic);
  }

  client.on("message", (topic, payload) => {
    if (!(topic in schemaByTopic)) return;
    const schema = schemaByTopic[topic as keyof SchemaByTopic];
    try {
      const parsed = schema.parse(JSON.parse(payload.toString())) as Command | ExecEvent | StatusMessage;
      const topicHandlers = handlers[topic as keyof HandlerMap];
      topicHandlers.forEach((handler) => {
        handler(parsed as never);
      });
    } catch (err) {
      console.warn(`[mqtt] failed to parse payload for ${topic}:`, err);
    }
  });

  const addCommandHandler = (handler: Handler<Command>) => {
    const set = handlers[MQTT_TOPICS.command];
    set.add(handler);
    if (!autoSubscribe && set.size === 1) {
      subscribeToTopic(MQTT_TOPICS.command);
    }
    return () => set.delete(handler);
  };

  const addExecHandler = (handler: Handler<ExecEvent>) => {
    const set = handlers[MQTT_TOPICS.eventExec];
    set.add(handler);
    if (!autoSubscribe && set.size === 1) {
      subscribeToTopic(MQTT_TOPICS.eventExec);
    }
    return () => set.delete(handler);
  };

  const addStatusHandler = (handler: Handler<StatusMessage>) => {
    const set = handlers[MQTT_TOPICS.status];
    set.add(handler);
    if (!autoSubscribe && set.size === 1) {
      subscribeToTopic(MQTT_TOPICS.status);
    }
    return () => set.delete(handler);
  };

  const publish = (message: PublishableMessage, publishOptions: IClientPublishOptions = {}) => {
    const topicKey = getTopicKey(message.topic);
    const qos = MQTT_QOS[topicKey];
    client.publish(message.topic, message.payload, { qos, ...publishOptions });
  };

  const disconnect = (force = false) =>
    new Promise<void>((resolve) => {
      client.end(force, {}, () => resolve());
    });

  return {
    client,
    publish,
    onCommand: addCommandHandler,
    onExecEvent: addExecHandler,
    onStatus: addStatusHandler,
    disconnect,
  };
}

function getTopicKey(topic: MqttTopic) {
  const entries = Object.entries(MQTT_TOPICS) as [keyof typeof MQTT_TOPICS, MqttTopic][];
  return entries.find(([, value]) => value === topic)?.[0] ?? "status";
}
