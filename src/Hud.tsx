import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import "./Hud.css";

type HudState = "idle" | "recording" | "transcribing" | "done" | "error";

const BARS = 28;

export default function Hud() {
  const [state, setState] = useState<HudState>("idle");
  const [message, setMessage] = useState<string | null>(null);
  const barsRef = useRef<number[]>(new Array(BARS).fill(0.05));
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const rafRef = useRef<number>(0);

  // estados vindos do Rust
  useEffect(() => {
    const un = listen<{ state: HudState; message: string | null }>("hud-state", (ev) => {
      setState(ev.payload.state);
      setMessage(ev.payload.message);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  // níveis de áudio em tempo real → barras rolantes
  useEffect(() => {
    const un = listen<number>("hud-level", (ev) => {
      const v = Math.min(1, Math.max(0.05, Math.sqrt(ev.payload) * 3.2));
      barsRef.current.push(v);
      if (barsRef.current.length > BARS) barsRef.current.shift();
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  // desenha o waveform continuamente
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = 180 * dpr;
    canvas.height = 44 * dpr;
    ctx.scale(dpr, dpr);

    const draw = () => {
      ctx.clearRect(0, 0, 180, 44);
      const n = barsRef.current.length;
      const barW = 3.5;
      const gap = (180 - n * barW) / (n + 1);
      barsRef.current.forEach((v, i) => {
        const h = Math.max(3, v * 40);
        const x = gap + i * (barW + gap);
        const y = (44 - h) / 2;
        const grad = ctx.createLinearGradient(0, y, 0, y + h);
        grad.addColorStop(0, "rgba(120, 220, 160, 0.95)");
        grad.addColorStop(1, "rgba(120, 220, 160, 0.45)");
        ctx.fillStyle = grad;
        ctx.beginPath();
        ctx.roundRect(x, y, barW, h, 2);
        ctx.fill();
      });
      rafRef.current = requestAnimationFrame(draw);
    };
    rafRef.current = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(rafRef.current);
  }, []);

  return (
    <div className={`hud hud-${state}`}>
      {state === "recording" && (
        <>
          <span className="hud-dot" />
          <canvas ref={canvasRef} className="hud-wave" width={180} height={44} />
        </>
      )}
      {state === "transcribing" && (
        <span className="hud-transcribing">
          <span className="hud-spinner" /> Transcrevendo…
        </span>
      )}
      {state === "done" && (
        <span className="hud-done">
          <svg viewBox="0 0 24 24" width="20" height="20">
            <path
              d="M4 12.5l5 5L20 6.5"
              fill="none"
              stroke="currentColor"
              strokeWidth="3"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
          Entendido
        </span>
      )}
      {state === "error" && (
        <span className="hud-error" title={message ?? ""}>
          ⚠ {message ?? "Erro"}
        </span>
      )}
      {state === "idle" && <span className="hud-transcribing">Fala</span>}
    </div>
  );
}
