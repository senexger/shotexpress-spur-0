export const MQTT_BASE_TOPIC = "shotexpress";

export const MQTT_TOPICS = {
  command: `${MQTT_BASE_TOPIC}/command`,
  status: `${MQTT_BASE_TOPIC}/status`,
  eventExec: `${MQTT_BASE_TOPIC}/event/exec`,
} as const;

export const MQTT_QOS = {
  command: 1,
  status: 0,
  eventExec: 1,
} as const;

type Values<T> = T[keyof T];

export type MqttTopic = Values<typeof MQTT_TOPICS>;
