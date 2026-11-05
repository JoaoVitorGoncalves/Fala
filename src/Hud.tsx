import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import "./Hud.css";

type HudState = "idle" | "recording" | "transcribing" | "done" | "error";

const BARS = 38;

export default function Hud() {
  const [state, setState] = useState<HudState>("idle");
  const [message, setMessage] = useState<string | null>(null);
  const [leaving, setLeaving] = useState(false);
  const targetsRef = useRef<number[]>(new Array(BARS).fill(0.06));
  const barsRef = useRef<number[]>(new Array(BARS).fill(0.06));
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  // estados vindos do Rust
  useEffect(() => {
    const un = listen<{ state: HudState; message: string | null }>("hud-state", (ev) => {
      setState(ev.payload.state);
      setMessage(ev.payload.message);
      setLeaving(false);
      // animação de saída antes de o Rust esconder a janela
      if (ev.payload.state === "done") {
        setTimeout(() => setLeaving(true), 850);
      } else if (ev.payload.state === "error") {
        setTimeout(() => setLeaving(true), 1700);
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  // níveis de áudio em tempo real → alvos das barras
  useEffect(() => {
    const un = listen<number>("hud-level", (ev) => {
      const v = Math.min(1, Math.max(0.05, Math.sqrt(Math.max(ev.payload, 0)) * 3.2));
      targetsRef.current.push(v);
      if (targetsRef.current.length > BARS) targetsRef.current.shift();
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  // loop de desenho — depende de `state`: o canvas só existe durante a gravação,
  // então o efeito precisa remontar quando ela começa
  useEffect(() => {
    if (state !== "recording") return;
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const W = 220;
    const H = 48;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = W * dpr;
    canvas.height = H * dpr;
    ctx.scale(dpr, dpr);

    let phase = 0;
    let raf = 0;
    const draw = () => {
      phase += 0.05;
      ctx.clearRect(0, 0, W, H);

      const n = barsRef.current.length;
      const barW = 3;
      const gap = (W - n * barW) / (n + 1);
      for (let i = 0; i < n; i++) {
        // easing: barra persegue o alvo (movimento macio)
        const target = targetsRef.current[i] ?? 0.06;
        barsRef.current[i] += (target - barsRef.current[i]) * 0.3;

        // respiração sutil no silêncio — a onda nunca fica "morta"
        const idle = 0.07 + 0.04 * (0.5 + 0.5 * Math.sin(phase * 2.2 + i * 0.5));
        const v = Math.max(barsRef.current[i], idle);

        const h = Math.max(4, v * H);
        const x = gap + i * (barW + gap);
        const y = (H - h) / 2; // simétrico em torno do centro
        ctx.fillStyle = "rgba(255, 255, 255, 0.95)";
        ctx.beginPath();
        ctx.roundRect(x, y, barW, h, barW / 2);
        ctx.fill();
      }
      raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, [state]);

  return (
    <div className={`hud hud-${state}${leaving ? " hud-leaving" : ""}`}>
      <div className="hud-content" key={state}>
        {state === "recording" && (
          <>
            <span className="hud-dot" />
            <canvas ref={canvasRef} className="hud-wave" width={220} height={48} />
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
            <svg
              viewBox="0 0 24 24"
              width="14"
              height="14"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
              <path d="M12 9v4" />
              <path d="M12 17h.01" />
            </svg>
            {message ?? "Erro"}
          </span>
        )}
        {state === "idle" && <span className="hud-transcribing">Fala</span>}
      </div>
    </div>
  );
}
