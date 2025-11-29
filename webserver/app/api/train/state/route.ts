import { NextResponse } from "next/server";
import { getTrainMqttService } from "../../../../lib/server/trainMqttService";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET() {
  const service = getTrainMqttService();
  return NextResponse.json(service.getSnapshot());
}
