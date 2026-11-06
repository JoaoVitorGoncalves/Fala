import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import Hud from "./Hud";

const label = getCurrentWindow().label;
if (label === "hud") {
  document.body.classList.add("hud-body");
}
// O vibrancy/Liquid Glass é exclusivo do macOS; no Windows as janelas
// transparentes viram vidro total e o conteúdo fica ilegível. A classe
// .win permite o CSS dar um material quase opaco só nesta plataforma.
if (navigator.userAgent.includes("Windows NT")) {
  document.documentElement.classList.add("win");
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{label === "hud" ? <Hud /> : <App />}</React.StrictMode>,
);
