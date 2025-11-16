import { startFakeExpress } from "../lib/fakeExpress";

async function main() {
  const controller = await startFakeExpress();
  console.log("[fake_express] simulator running – press Ctrl+C to stop.");

  const shutdown = async () => {
    console.log("\n[fake_express] shutting down…");
    await controller.stop();
    process.exit(0);
  };

  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
}

main().catch((error) => {
  console.error("[fake_express] failed to start", error);
  process.exit(1);
});
