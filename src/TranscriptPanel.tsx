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
  const [hl, setHl] = useState<number[]>([]);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const toastTimer = useRef<number | null>(null);
  const cleanMap = useRef<Map<number, boolean>>(new Map());

  useEffect(() => {
    invoke<Segment[]>("panel_state")
      .then((segs) => {
        setSegments(segs);
        segs.forEach((s) => cleanMap.current.set(s.id, s.clean !== null));
      })
      .catch(() => {});
    const un = listen<{ segments: Segment[] }>("panel-state", (ev) => {
      // ids que acabaram de sair de CRU → LIMPO ganham o realce verde
      const fresh: number[] = [];
      ev.payload.segments.forEach((s) => {
        const was = cleanMap.current.get(s.id) ?? false;
        const now = s.clean !== null;
        if (!was && now) fresh.push(s.id);
        cleanMap.current.set(s.id, now);
      });
      setSegments(ev.payload.segments);
      if (fresh.length > 0) {
        setHl((l) => [...l, ...fresh]);
        window.setTimeout(() => setHl((l) => l.filter((id) => !fresh.includes(id))), 1400);
      }
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
        <svg className="panel-glyph" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="var(--t2)" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
          <rect x="8" y="2" width="8" height="4" rx="1" />
          <path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2" />
        </svg>
        <span className="panel-title">Versão limpa</span>
        {segments.length > 0 && <span className="panel-cnt">{segments.length} segmentos</span>}
        <span className="panel-sp" />
        <span className="panel-actions">
          <button className="panel-btn panel-primary" onClick={copyAll} disabled={segments.length === 0}>
            Copiar tudo
          </button>
          <button className="panel-btn" onClick={clearAll} disabled={segments.length === 0}>
            Limpar
          </button>
        </span>
      </header>
      <div className="panel-scroll" ref={scrollRef}>
        {segments.length === 0 && (
          <div className="panel-empty">
            <div className="panel-empty-ic">
              <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
                <path d="M2 10v3M6 6v11M10 3v18M14 8v7M18 5v13M22 10v3" />
              </svg>
            </div>
            <div className="panel-empty-t">Nada capturado ainda</div>
            <div className="panel-empty-s">
              Segure <kbd>alt+space</kbd>, fale, e solte.<br />
              O ditado aparece aqui — cru primeiro, limpo em segundos.
            </div>
          </div>
        )}
        {segments.map((s) => {
          const cleaned = s.clean !== null;
          return (
            <div key={s.id} className={`panel-seg${cleaned ? "" : " pending"}${hl.includes(s.id) ? " hl" : ""}`}>
              <div className="panel-txt">{s.clean ?? s.raw}</div>
              <div className="panel-meta">
                <span className={`panel-tag ${cleaned ? "clean" : "raw"}`}>
                  {cleaned ? "LIMPO" : "CRU · polindo…"}
                </span>
                <span className="panel-chars">{(s.clean ?? s.raw).length} chars</span>
              </div>
            </div>
          );
        })}
      </div>
      {toast && <div className="panel-toast">{toast}</div>}
    </div>
  );
}
