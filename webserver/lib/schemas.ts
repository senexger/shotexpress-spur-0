import { z } from "zod";
import { MQTT_TOPICS } from "./mqttTopics";

export const envelopeSchema = z.object({
    msg_id: z.uuidv4(),
    cmd_id: z.union([z.uuidv4(), z.literal("")]),
    seq: z.number().int().nonnegative(),
    ts_ms: z.number().int().nonnegative(),
});

// Train Messages

export const execAcceptedSchema = z.object({
    ...envelopeSchema.shape,
    exec_type: z.literal("accepted"),
});

export const execStartedSchema = z.object({
    ...envelopeSchema.shape,
    exec_type: z.literal("started"),
});

export const execProgressSchema = z.object({
    ...envelopeSchema.shape,
    exec_type: z.literal("progress"),
    direction: z.enum(["forward", "reverse"]),
    progress: z.object({
        last_tag: z.string().optional(),
        distance_m: z.number().nonnegative().optional(),
    }).optional()
});

export const execCompletedSchema = z.object({
    ...envelopeSchema.shape,
    exec_type: z.literal("completed"),
});

export const execFailedSchema = z.object({
    ...envelopeSchema.shape,
    exec_type: z.literal("failed"),
    error: z.object({
        code: z.string(),
        reason: z.string(),
    }),
});

export const execCancelledSchema = z.object({
    ...envelopeSchema.shape,
    exec_type: z.literal("cancelled"),
});

export const execExpiredSchema = z.object({
    ...envelopeSchema.shape,
    exec_type: z.literal("expired"),
});

export const eventExecSchema = z.discriminatedUnion("exec_type", [
    execAcceptedSchema,
    execStartedSchema,
    execProgressSchema,
    execCompletedSchema,
    execFailedSchema,
    execCancelledSchema,
    execExpiredSchema,
]);

export type ExecEvent = z.infer<typeof eventExecSchema>;

// status messages
export const trainStateSchema = z.enum([
  "IDLE",
  "RUNNING",
  "APPROACH",
  "DWELL",
  "SAFE_STOP",
  "ERROR",
]);

export type TrainState = z.infer<typeof trainStateSchema>;

export const statusSchema = z.object({
  ...envelopeSchema.shape,
  state: trainStateSchema,
  battery_pct: z.number().int().min(0).max(100),
  status_uptime_ms: z.number().int().nonnegative(),
});

export type StatusMessage = z.infer<typeof statusSchema>;


// Server Messages

export const offlinePlanSchema = z.object({
    approach_slowdown_ms: z.number().nonnegative(),
    max_run_ms_without_tag: z.number().nonnegative(),
    crawl_speed: z.number().min(0).max(1),
    dwell_ms: z.number().nonnegative()
})

export const moveCommandParametersSchema = z.object({
    target: z.enum(["bar", "schachbrett", "forgot_name", "raucherecke"]),
    speed: z.number().min(0).max(1),
    direction: z.enum(["forward", "reverse"]),
    expected_tags: z.array(z.string()),
    stop_on_tag: z.string(),
    offline_plan: offlinePlanSchema
});

export const stopCommandSchema = z.object({
    ...envelopeSchema.shape,
    cmd_type: z.literal("stop")
});

export const waitForLoadCommandSchema = z.object({
    ...envelopeSchema.shape,
    cmd_type: z.literal("wait_for_load"),
    wait_time_ms: z.number().nonnegative(),
    ttl_ms: z.number().int().nonnegative(),
});

export const moveCommandSchema = z.object({
    ...envelopeSchema.shape,
    cmd_type: z.literal("move_to"),
    params: moveCommandParametersSchema,
    ttl_ms: z.number().int().nonnegative(),
});

export const continueCommandSchema = z.object({
    ...moveCommandSchema.shape,
    cmd_type: z.literal("continue"),
    ttl_ms: z.number().int().nonnegative(),
});

export const commandSchema = z.discriminatedUnion("cmd_type", [
    stopCommandSchema,
    waitForLoadCommandSchema,
    moveCommandSchema,
    continueCommandSchema
]);

export type PublishableMessage = {
  topic: (typeof MQTT_TOPICS)[keyof typeof MQTT_TOPICS];
  payload: string;
};

export type CommandType = z.infer<typeof commandSchema>["cmd_type"];
export type Envelope = z.infer<typeof envelopeSchema>;
export type OfflinePlan = z.infer<typeof offlinePlanSchema>;
export type MoveCommandParameters = z.infer<typeof moveCommandParametersSchema>;
export type StopCommand = z.infer<typeof stopCommandSchema>;
export type WaitForLoadCommand = z.infer<typeof waitForLoadCommandSchema>;
export type MoveCommand = z.infer<typeof moveCommandSchema>;
export type ContinueCommand = z.infer<typeof continueCommandSchema>;
export type Command = z.infer<typeof commandSchema>;

export type BaseMessage = Envelope & Record<string, unknown>;