import React from "react";
import { TrainDashboard } from "../components/TrainDashboard";

export default function Page() {
  return (
    <main className="control-page">
      <header className="page-header">
        <h1 className="page-title">ShotExpress Control</h1>
        <p className="page-subtitle">
          Dispatch the train to the Raucherecke and track its progress in real time.
        </p>
      </header>
      <TrainDashboard />
    </main>
  );
}
