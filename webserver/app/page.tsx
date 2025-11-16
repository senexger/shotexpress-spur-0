import { TrainDashboard } from "../components/TrainDashboard";

export default function Page() {
  return (
    <main style={{ maxWidth: 720, margin: "2.5rem auto", fontFamily: "system-ui, sans-serif", padding: "0 1rem" }}>
      <header style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ fontSize: "2rem", fontWeight: 600, margin: 0 }}>ShotExpress Control</h1>
        <p style={{ color: "#475569", marginTop: "0.5rem", fontSize: "0.95rem" }}>
          Dispatch the train to the Raucherecke and track its progress in real time.
        </p>
      </header>
      <TrainDashboard />
    </main>
  );
}
