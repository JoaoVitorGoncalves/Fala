import { useCallback, useRef, useState } from "react";
import type { ReactNode } from "react";
import "./toast.css";

export interface ToastItem {
  id: number;
  kind: "ok" | "warn" | "err";
  title: string;
  desc?: string;
  /** marcado durante a animação de saída */
  bye?: boolean;
}

/** Fila de toasts com auto-dismiss — useToasts() no componente dono. */
export function useToasts() {
  const [items, setItems] = useState<ToastItem[]>([]);
  const nextId = useRef(0);
  const dismiss = useCallback((id: number) => {
    // anima a saída antes de remover do estado
    setItems((list) => list.map((t) => (t.id === id ? { ...t, bye: true } : t)));
    setTimeout(() => setItems((list) => list.filter((t) => t.id !== id)), 260);
  }, []);
  const toast = useCallback(
    (kind: ToastItem["kind"], title: string, desc?: string) => {
      const id = ++nextId.current;
      setItems((list) => [...list.slice(-3), { id, kind, title, desc }]);
      setTimeout(() => dismiss(id), 4200);
    },
    [dismiss],
  );
  return { items, toast, dismiss };
}

const GLYPHS: Record<ToastItem["kind"], ReactNode> = {
  ok: (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" strokeLinejoin="round"><path d="M4 12.5l5 5L20 6.5" /></svg>
  ),
  warn: (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" /><path d="M12 9v4M12 17h.01" /></svg>
  ),
  err: (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round"><circle cx="12" cy="12" r="9" /><path d="M12 8v4M12 16h.01" /></svg>
  ),
};

export function ToastHost({
  items,
  onDismiss,
}: {
  items: (ToastItem & { bye?: boolean })[];
  onDismiss: (id: number) => void;
}) {
  if (items.length === 0) return null;
  return (
    <div className="toasts">
      {items.map((t) => (
        <div
          key={t.id}
          className={`toast ${t.kind}${t.bye ? " bye" : ""}`}
          onClick={() => onDismiss(t.id)}
          role="status"
        >
          <span className="t-ic">{GLYPHS[t.kind]}</span>
          <div className="t-body">
            {t.title}
            {t.desc && <div className="t-desc">{t.desc}</div>}
          </div>
        </div>
      ))}
    </div>
  );
}
