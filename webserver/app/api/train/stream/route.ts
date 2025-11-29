import { getTrainMqttService, type TrainServiceEvent } from "../../../../lib/server/trainMqttService";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export function GET(request: Request) {
  const service = getTrainMqttService();
  const encoder = new TextEncoder();

  let cleanedUp = false;
  let cleanup = () => {
    cleanedUp = true;
  };

  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      const send = (event: TrainServiceEvent) => {
        controller.enqueue(encoder.encode(`data: ${JSON.stringify(event)}\n\n`));
      };

      send({ type: "snapshot", payload: service.getSnapshot() });

      const unsubscribe = service.subscribe(send);
      const heartbeat = setInterval(() => {
        controller.enqueue(encoder.encode(`event: heartbeat\ndata: ${Date.now()}\n\n`));
      }, 25_000);

      cleanup = () => {
        if (cleanedUp) return;
        cleanedUp = true;
        clearInterval(heartbeat);
        unsubscribe();
        request.signal.removeEventListener("abort", abortListener);
      };

      const abortListener = () => {
        cleanup();
        controller.close();
      };

      if (request.signal.aborted) {
        abortListener();
        return;
      }

      request.signal.addEventListener("abort", abortListener);
    },
    cancel() {
      cleanup();
    },
  });

  return new Response(stream, {
    headers: {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache, no-transform",
      Connection: "keep-alive",
    },
  });
}
