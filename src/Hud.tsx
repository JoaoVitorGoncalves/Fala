import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import "./Hud.css";

type HudState = "idle" | "recording" | "transcribing" | "done" | "error";

export default function Hud() {
  const [state, setState] = useState<HudState>("idle");
  const [message, setMessage] = useState<string | null>(null);
  const [leaving, setLeaving] = useState(false);
  const levelRef = useRef(0.12); // alvo vindo do Rust (RMS mapeado)
  const smoothRef = useRef(0.12); // nível suavizado usado no desenho
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

  // nível de áudio em tempo real → alvo do ribbon
  useEffect(() => {
    const un = listen<number>("hud-level", (ev) => {
      const v = Math.min(1, Math.max(0.04, Math.sqrt(Math.max(ev.payload, 0)) * 3.2));
      levelRef.current = v;
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  // loop de desenho — o canvas só existe durante a gravação,
  // então o efeito precisa remontar quando ela começa
  useEffect(() => {
    if (state !== "recording") return;
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const dpr = Math.min(2, window.devicePixelRatio || 1);
    const W = Math.max(160, canvas.clientWidth || 204);
    const H = Math.max(32, canvas.clientHeight || 40);
    canvas.width = W * dpr;
    canvas.height = H * dpr;
    ctx.scale(dpr, dpr);

    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

    // Catmull-Rom → bezier: spline suave pelos pontos
    const spline = (p: number[][]) => {
      ctx.moveTo(p[0][0], p[0][1]);
      for (let i = 0; i < p.length - 1; i++) {
        const p0 = p[Math.max(0, i - 1)];
        const p1 = p[i];
        const p2 = p[i + 1];
        const p3 = p[Math.min(p.length - 1, i + 2)];
        ctx.bezierCurveTo(
          p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6,
          p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6,
          p2[0], p2[1],
        );
      }
    };

    const draw = (t: number, lvl: number) => {
      ctx.clearRect(0, 0, W, H);
      const cy = H / 2;
      const n = 60;
      const amp = 4.5 + lvl * 17;
      const pts: number[][] = [];
      for (let i = 0; i <= n; i++) {
        const u = i / n;
        // envelope afina as pontas — o ribbon "pinça" nas extremidades
        const env = Math.pow(Math.sin(Math.PI * u), 0.85);
        const w =
          Math.sin(u * 7.1 + t * 2.7) * 0.55 +
          Math.sin(u * 2.9 - t * 1.9) * 0.3 +
          Math.sin(u * 11.3 + t * 4.3) * 0.15;
        pts.push([u * W, cy + w * env * amp]);
      }
      ctx.lineJoin = "round";
      ctx.lineCap = "round";

      // aura: traço largo e translúcido sob o ribbon
      ctx.beginPath();
      spline(pts);
      ctx.strokeStyle = "rgba(123, 91, 255, 0.30)";
      ctx.lineWidth = 11;
      ctx.stroke();

      // núcleo: gradiente da Siri com glow
      const g = ctx.createLinearGradient(0, 0, W, 0);
      g.addColorStop(0, "#45A6FF");
      g.addColorStop(0.48, "#7B5BFF");
      g.addColorStop(1, "#F151C8");
      ctx.shadowColor = "rgba(123, 91, 255, 0.60)";
      ctx.shadowBlur = 12;
      ctx.strokeStyle = g;
      ctx.lineWidth = 3.6;
      ctx.stroke();
      ctx.shadowBlur = 0;
    };

    if (reduced) {
      draw(2.0, 0.5);
      return;
    }

    let raf = 0;
    const loop = (now: number) => {
      // o nível persegue o alvo; no silêncio o ribbon respira — nunca morre
      const breathe = 0.05 + 0.03 * (0.5 + 0.5 * Math.sin((now / 1000) * 1.6));
      const target = Math.max(levelRef.current, breathe);
      smoothRef.current += (target - smoothRef.current) * 0.12;
      draw(now / 1000, smoothRef.current);
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, [state]);

  return (
    <div className={`hud hud-${state}${leaving ? " hud-leaving" : ""}`}>
      <div className="hud-content" key={state}>
        {state === "recording" && (
          <div className="hud-row hud-row-rec">
            <span className="hud-dot" />
            <canvas ref={canvasRef} className="hud-wave" />
          </div>
        )}
        {state === "transcribing" && (
          <div className="hud-row hud-row-center">
            <span className="hud-spinner" /> Transcrevendo…
          </div>
        )}
        {state === "done" && (
          <div className="hud-row hud-row-center hud-done">
            <svg viewBox="0 0 24 24" width="17" height="17">
              <path
                d="M4 12.5l5 5L20 6.5"
                fill="none"
                stroke="currentColor"
                strokeWidth="2.6"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
            Entendido
          </div>
        )}
        {state === "error" && (
          <div className="hud-row hud-row-center hud-error" title={message ?? ""}>
            <svg
              viewBox="0 0 24 24"
              width="15"
              height="15"
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
            <span className="hud-err-text">{message ?? "Erro"}</span>
          </div>
        )}
        {state === "idle" && (
          <div className="hud-row hud-row-center hud-idle-label">Fala</div>
        )}
      </div>
    </div>
  );
}
