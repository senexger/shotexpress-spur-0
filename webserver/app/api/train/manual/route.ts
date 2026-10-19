import { NextResponse } from "next/server";
import { z } from "zod";
import { getTrainMqttService, MANUAL_ACTIONS } from "../../../../lib/server/trainMqttService";

export const runtime = "nodejs";

const bodySchema = z.object({
  action: z.enum(Object.keys(MANUAL_ACTIONS) as [keyof typeof MANUAL_ACTIONS]),
});

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
    return NextResponse.json({ error: "Invalid manual command", details: parsed.error.flatten() }, { status: 400 });
  }

  try {
    service.sendManualCommand(parsed.data.action);
    return NextResponse.json({ ok: true });
  } catch (err) {
    const message = err instanceof Error ? err.message : "Failed to dispatch manual command";
    return NextResponse.json({ error: message }, { status: 503 });
  }
}
