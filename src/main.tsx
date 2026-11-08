import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import Hud from "./Hud";
import TranscriptPanel from "./TranscriptPanel";

const label = getCurrentWindow().label;
if (label === "hud") {
  document.body.classList.add("hud-body");
}
if (label === "panel") {
  document.body.classList.add("panel-body");
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {label === "hud" ? <Hud /> : label === "panel" ? <TranscriptPanel /> : <App />}
  </React.StrictMode>,
);
