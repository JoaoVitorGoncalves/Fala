import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import "./Hud.css";

type HudState = "idle" | "recording" | "transcribing" | "done" | "error";

const BARS = 38;

function formatElapsed(ms: number) {
  const seconds = Math.floor(ms / 1000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

export default function Hud() {
  const [state, setState] = useState<HudState>("idle");
  const [message, setMessage] = useState<string | null>(null);
  const [leaving, setLeaving] = useState(false);
  const leavingTimerRef = useRef<number | null>(null);
  const recordingStartedRef = useRef<number | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const targetsRef = useRef<number[]>(new Array(BARS).fill(0.06));
  const barsRef = useRef<number[]>(new Array(BARS).fill(0.06));
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  // chip de feedback (comandos de voz / avisos) sem trocar de estado
  const [chip, setChip] = useState<string | null>(null);
  const chipTimer = useRef<number | null>(null);

  // chips: mensagem transitória sobre o HUD atual
  useEffect(() => {
    const un = listen<{ message: string }>("hud-chip", (ev) => {
      if (chipTimer.current !== null) window.clearTimeout(chipTimer.current);
      setChip(ev.payload.message);
      chipTimer.current = window.setTimeout(() => {
        setChip(null);
        chipTimer.current = null;
      }, 1500);
    });
    return () => {
      if (chipTimer.current !== null) window.clearTimeout(chipTimer.current);
      un.then((f) => f());
    };
  }, []);

  // estados vindos do Rust
  useEffect(() => {
    const un = listen<{ state: HudState; message: string | null }>("hud-state", (ev) => {
      if (leavingTimerRef.current !== null) {
        window.clearTimeout(leavingTimerRef.current);
        leavingTimerRef.current = null;
      }
      setLeaving(false);
      setState(ev.payload.state);
      setMessage(ev.payload.message);
      if (ev.payload.state === "recording") {
        recordingStartedRef.current = performance.now();
        setElapsed(0);
      } else {
        recordingStartedRef.current = null;
      }

      // A saída atua somente na opacidade do shell. O timeout é cancelado
      // quando uma nova gravação/transcrição começa.
      const delay = ev.payload.state === "done" ? 850 : ev.payload.state === "error" ? 1700 : null;
      if (delay !== null) {
        leavingTimerRef.current = window.setTimeout(() => {
          leavingTimerRef.current = null;
          setLeaving(true);
        }, delay);
      }
    });
    return () => {
      if (leavingTimerRef.current !== null) {
        window.clearTimeout(leavingTimerRef.current);
      }
      un.then((f) => f());
    };
  }, []);

  // Timer visível apenas durante a gravação — reforça que o mic está ativo.
  useEffect(() => {
    if (state !== "recording") return;
    const timer = window.setInterval(() => {
      if (recordingStartedRef.current !== null) {
        setElapsed(performance.now() - recordingStartedRef.current);
      }
    }, 250);
    return () => window.clearInterval(timer);
  }, [state]);

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
      {chip && <span className="hud-chip">{chip}</span>}
      <div className="hud-content" key={state}>
        {state === "recording" && (
          <div className="hud-recording-content">
            <span className="hud-dot" aria-hidden="true" />
            <canvas ref={canvasRef} className="hud-wave" width={220} height={48} aria-label="Nível do microfone" />
            <span className="hud-timer" aria-label="Tempo de gravação">
              {formatElapsed(elapsed)}
            </span>
          </div>
        )}
        {state === "transcribing" && (
          <span className="hud-transcribing">
            <span className="hud-spinner" /> {message ?? "Transcrevendo…"}
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
        {state === "idle" && <span className="hud-transcribing">Pronto para ouvir</span>}
      </div>
    </div>
  );
}
