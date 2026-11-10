import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./panel.css";

interface Segment {
  id: number;
  raw: string;
  clean: string | null;
}

export default function TranscriptPanel() {
  const [segments, setSegments] = useState<Segment[]>([]);
  const [toast, setToast] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const toastTimer = useRef<number | null>(null);

  useEffect(() => {
    invoke<Segment[]>("panel_state")
      .then(setSegments)
      .catch(() => {});
    const un = listen<{ segments: Segment[] }>("panel-state", (ev) => {
      setSegments(ev.payload.segments);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  // auto-scroll: acompanha o texto chegando
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  }, [segments]);

  function showToast(msg: string, ms = 1600) {
    if (toastTimer.current !== null) window.clearTimeout(toastTimer.current);
    setToast(msg);
    toastTimer.current = window.setTimeout(() => {
      setToast(null);
      toastTimer.current = null;
    }, ms);
  }

  async function copyAll() {
    try {
      const n = await invoke<number>("panel_copy");
      showToast(`Copiado ✓ (${n} caracteres)`);
    } catch (e) {
      showToast(String(e), 2400);
    }
  }

  async function clearAll() {
    try {
      await invoke("panel_clear");
    } catch (e) {
      showToast(String(e), 2400);
    }
  }

  return (
    <div className="panel">
      <header className="panel-header">
        <span className="panel-title">Versão limpa</span>
        <span className="panel-actions">
          <button onClick={copyAll} disabled={segments.length === 0}>
            Copiar
          </button>
          <button
            className="panel-danger"
            onClick={clearAll}
            disabled={segments.length === 0}
          >
            Limpar
          </button>
        </span>
      </header>
      <div className="panel-scroll" ref={scrollRef}>
        {segments.length === 0 && (
          <p className="panel-empty">
            Dite algo — os trechos aparecem aqui e a IA limpa cada um em
            segundos. Acumula entre ditados até você limpar.
          </p>
        )}
        {segments.map((s) => (
          <p key={s.id} className={`panel-seg${s.clean ? "" : " pending"}`}>
            {s.clean ?? s.raw}
          </p>
        ))}
      </div>
      {toast && <div className="panel-toast">{toast}</div>}
    </div>
  );
}
