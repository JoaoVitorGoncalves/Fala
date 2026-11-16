import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./Hud.css";

type HudState = "idle" | "recording" | "transcribing" | "polishing" | "done" | "error";
type DoneInfo = { mode: "clipboard" | "meeting" | "direct"; chars: number | null };

const BARS = 22;

/** Mensagem do done vinda do Rust: "clipboard" | "meeting" | "direct|<chars>". */
function parseDone(message: string | null): DoneInfo {
  if (message === "clipboard") return { mode: "clipboard", chars: null };
  if (message === "meeting") return { mode: "meeting", chars: null };
  if (message?.startsWith("direct|")) {
    const n = Number(message.slice(7));
    return { mode: "direct", chars: Number.isFinite(n) ? n : null };
  }
  return { mode: "direct", chars: null };
}

export default function Hud() {
  const [state, setState] = useState<HudState>("idle");
  const [message, setMessage] = useState<string | null>(null);
  const [done, setDone] = useState<DoneInfo>({ mode: "direct", chars: null });
  const [polishSecs, setPolishSecs] = useState(0);
  const [leaving, setLeaving] = useState(false);
  const leavingTimerRef = useRef<number | null>(null);
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

  // Aviso curto quando uma entrega final chega ao clipboard ou ao campo focado.
  // A mesma instância é reutilizada para não sobrepor sons em sessões rápidas.
  useEffect(() => {
    const audio = new Audio("/sounds/completion.wav");
    audio.preload = "auto";
    audio.load();
    const un = listen("delivery-complete", () => {
      audio.currentTime = 0;
      void audio.play().catch(() => {
        // A WebView pode bloquear áudio sem gesto local; a entrega já foi concluída.
      });
    });
    return () => {
      audio.pause();
      audio.removeAttribute("src");
      audio.load();
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
      if (ev.payload.state === "done") setDone(parseDone(ev.payload.message));
      if (ev.payload.state === "polishing") setPolishSecs(0);

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

  // contador honesto do polimento (a IA pode demorar — mostre quanto)
  useEffect(() => {
    if (state !== "polishing") return;
    const t = window.setInterval(() => setPolishSecs((s) => s + 1), 1000);
    return () => window.clearInterval(t);
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

    const W = 120;
    const H = 26;
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

        const h = Math.max(3, v * H);
        const x = gap + i * (barW + gap);
        const y = (H - h) / 2; // simétrico em torno do centro
        ctx.fillStyle = "rgba(255, 92, 92, 0.95)";
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
    <div className={`hud-root${leaving ? " hud-leaving" : ""}`}>
      {chip && <span className="hud-chip">{chip}</span>}
      <div className="hud-wrap">
        {/* idle */}
        <div className={`hud${state === "idle" ? " on" : ""}`}>
          <div className="hud-state">
            <span className="hud-dot hud-dot-idle" />
            <span className="hud-lab-2">Pronto para ouvir</span>
            <span className="hud-mono">alt+space</span>
          </div>
        </div>
        {/* recording */}
        <div className={`hud hud-rec${state === "recording" ? " on" : ""}`}>
          <div className="hud-state">
            <button
              type="button"
              className="hbtn hbtn-x"
              aria-label="Cancelar e descartar a gravação"
              onClick={() => void invoke("hud_cancel")}
            >
              <svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round">
                <path d="M6 6l12 12" />
                <path d="M18 6L6 18" />
              </svg>
            </button>
            <canvas ref={canvasRef} className="hud-wave" width={120} height={26} aria-label="Nível do microfone" />
            <button
              type="button"
              className="hbtn hbtn-ok"
              aria-label="Encerrar e polir com IA"
              onClick={() => void invoke("hud_accept")}
            >
              <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round">
                <path d="M4 12.5l5 5L20 6.5" />
              </svg>
            </button>
          </div>
        </div>
        {/* transcribing */}
        <div className={`hud${state === "transcribing" ? " on" : ""}`}>
          <div className="hud-state">
            <span className="hud-ring" />
            <span className="hud-lab">Transcrevendo…</span>
            <span className="hud-mono">{message ?? ""}</span>
          </div>
        </div>
        {/* polishing */}
        <div className={`hud hud-pol${state === "polishing" ? " on" : ""}`}>
          <div className="hud-state">
            <span className="hud-ring hud-ring-pol" />
            <span className="hud-lab">Polindo…</span>
            <span className="hud-mono">{polishSecs}s</span>
          </div>
        </div>
        {/* done — contextual: destino do resultado */}
        <div className={`hud hud-ok${state === "done" ? " on" : ""}`}>
          <div className="hud-state">
            <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="var(--ok)" strokeWidth="2.8" strokeLinecap="round" strokeLinejoin="round">
              <path d="M4 12.5l5 5L20 6.5" />
            </svg>
            {done.mode === "clipboard" && (
              <>
                <span className="hud-lab hud-lab-ok">No clipboard</span>
                <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="var(--t3)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <rect x="8" y="2" width="8" height="4" rx="1" />
                  <path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2" />
                </svg>
                <span className="hud-mono">Ctrl+V</span>
              </>
            )}
            {done.mode === "meeting" && (
              <>
                <span className="hud-lab hud-lab-ok">No painel</span>
                <span className="hud-mono">versão limpa</span>
              </>
            )}
            {done.mode === "direct" && (
              <>
                <span className="hud-lab hud-lab-ok">Entregue</span>
                {done.chars !== null && <span className="hud-mono">{done.chars} chars</span>}
              </>
            )}
          </div>
        </div>
        {/* error */}
        <div className={`hud hud-err${state === "error" ? " on" : ""}`}>
          <div className="hud-state">
            <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="var(--err)" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
              <path d="M12 9v4" />
              <path d="M12 17h.01" />
            </svg>
            <span className="hud-lab hud-lab-trunc" title={message ?? ""}>
              {message ?? "Erro"}
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
