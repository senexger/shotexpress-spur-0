import { NextResponse } from "next/server";
import { z } from "zod";
import { getTrainMqttService, getDefaultMoveParameters, getDefaultTtlMs } from "../../../../lib/server/trainMqttService";
import { moveCommandParametersSchema, type MoveCommandParameters } from "../../../../lib/schemas";

export const runtime = "nodejs";

const bodySchema = z
  .object({
    ttl_ms: z.number().int().positive().optional(),
    parameters: moveCommandParametersSchema.partial().optional(),
  })
  .optional();

export async function POST(request: Request) {
  const service = getTrainMqttService();

  let payload: unknown;
  try {
    payload = await request.json();
  } catch {
    payload = undefined;
  }

  const parsed = bodySchema.safeParse(payload);
  if (!parsed.success) {
    return NextResponse.json({ error: "Invalid command payload", details: parsed.error.flatten() }, { status: 400 });
  }

  const defaults = getDefaultMoveParameters();
  const overrides = parsed.data?.parameters ?? {};
  const parameters: MoveCommandParameters = {
    ...defaults,
    ...overrides,
  };

  const ttlMs = parsed.data?.ttl_ms ?? getDefaultTtlMs();

  try {
    await service.sendMoveCommand(parameters, ttlMs);
    return NextResponse.json({ ok: true });
  } catch (err) {
    const message = err instanceof Error ? err.message : "Failed to dispatch command";
    const status = message.includes("busy") ? 409 : 503;
    return NextResponse.json({ error: message }, { status });
  }
}
