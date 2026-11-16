import { useCallback, useEffect, useRef, useState } from "react";
import type { CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { observeReveals } from "./motion";
import { ToastHost, useToasts } from "./Toast";
import "./App.css";

interface AiSettings {
  enabled: boolean;
  endpoint: string;
  profile_url: string;
  token: string;
  model: string;
  timeout_secs: number;
  sync_profile: boolean;
  system_prompt: string | null;
  /** Campo legado; o backend aguarda a IA sempre que ela está ativa. */
  polish_before_paste: boolean;
}

interface Settings {
  hotkey: string;
  mode: string;
  silence_ms: number;
  language: string;
  device: string | null;
  model_path: string | null;
  autostart: boolean;
  max_seconds: number;
  paste_mode: string;
  vocab_prompt: string | null;
  corrections: string | null;
  live_transcribe: boolean;
  live_model_path: string | null;
  meeting_mode: boolean;
  save_dataset: boolean;
  ai: AiSettings | null;
}

interface DatasetStats {
  total_samples: number;
  total_duration_secs: number;
  dataset_dir: string;
}

interface SettingsInfo {
  settings: Settings;
  config_path: string;
  status: "idle" | "recording" | "transcribing";
  accessibility: boolean;
  live_model_available: boolean;
  live_model_path: string;
  dataset_stats: DatasetStats;
}

const IDIOMAS = [
  ["pt", "Português"],
  ["en", "Inglês"],
  ["es", "Espanhol"],
  ["fr", "Francês"],
  ["it", "Italiano"],
  ["de", "Alemão"],
  ["auto", "Detecção automática"],
];

const STATUS_LABEL: Record<string, string> = {
  idle: "Pronto",
  recording: "Gravando…",
  transcribing: "Transcrevendo…",
};

type SaveState = "idle" | "saving" | "saved" | "error" | "invalid";

// espelha AiSettings::default() no backend (usado ao ligar a IA pela 1ª vez)
const DEFAULT_AI: AiSettings = {
  enabled: true,
  endpoint: "",
  profile_url: "",
  token: "",
  model: "llama3.2:3b",
  timeout_secs: 45,
  sync_profile: true,
  system_prompt: null,
  polish_before_paste: false,
};

// teclas que sozinhas não viram atalho (o macOS não registra modificador puro)
const MODIFIER_CODES = new Set([
  "AltLeft",
  "AltRight",
  "ControlLeft",
  "ControlRight",
  "MetaLeft",
  "MetaRight",
  "ShiftLeft",
  "ShiftRight",
]);

/** Converte o atalho salvo ("alt+space") em rótulo legível com símbolos do Mac. */
function prettyHotkey(hotkey: string): string {
  const symbols: Record<string, string> = {
    CTRL: "⌃ Ctrl",
    CONTROL: "⌃ Ctrl",
    ALT: "⌥ Option",
    OPTION: "⌥ Option",
    SHIFT: "⇧ Shift",
    CMD: "⌘ Cmd",
    COMMAND: "⌘ Cmd",
    SUPER: "⌘ Cmd",
    SPACE: "Espaço",
    ENTER: "Enter",
    TAB: "Tab",
    BACKSPACE: "⌫",
    DELETE: "⌦",
    ARROWUP: "↑",
    ARROWDOWN: "↓",
    ARROWLEFT: "←",
    ARROWRIGHT: "→",
    COMMA: ",",
    PERIOD: ".",
    MINUS: "-",
    EQUAL: "=",
    SLASH: "/",
    SEMICOLON: ";",
    QUOTE: "'",
    BACKQUOTE: "`",
    BACKSLASH: "\\",
    BRACKETLEFT: "[",
    BRACKETRIGHT: "]",
  };
  return hotkey
    .split("+")
    .map((token) => {
      const t = token.trim().toUpperCase();
      if (symbols[t]) return symbols[t];
      if (t.startsWith("KEY")) return t.slice(3);
      if (t.startsWith("DIGIT")) return t.slice(5);
      return token.trim();
    })
    .join(" + ");
}

/* glifos (stroke, não emoji) */
function Sym({ d, size = 16, sw = 1.8 }: { d: string[]; size?: number; sw?: number }) {
  return (
    <svg
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth={sw}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {d.map((p, i) => (
        <path key={i} d={p} />
      ))}
    </svg>
  );
}

const IC = {
  command: ["M15 6v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3V6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3"],
  wave: ["M2 10v3", "M6 6v11", "M10 3v18", "M14 8v7", "M18 5v13", "M22 10v3"],
  sparkles: [
    "M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z",
    "M20 3v4",
    "M22 5h-4",
  ],
  globe: ["M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0", "M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20", "M2 12h20"],
  mic: ["M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3z", "M19 10v2a7 7 0 0 1-14 0v-2", "M12 19v3"],
  cpu: ["M6 4h12a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z", "M10 9h4a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1h-4a1 1 0 0 1-1-1v-4a1 1 0 0 1 1-1z", "M9 2v2", "M15 2v2", "M9 20v2", "M15 20v2", "M2 9h2", "M2 15h2", "M20 9h2", "M20 15h2"],
  clipboard: ["M9 3h6a1 1 0 0 1 1 1v2a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z", "M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"],
  power: ["M12 2v10", "M18.4 6.6a9 9 0 1 1-12.77.04"],
  timer: ["M12 22a8 8 0 1 0 0-16 8 8 0 0 0 0 16", "M10 2h4", "M12 14l3-3"],
  alert: ["M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z", "M12 9v4", "M12 17h.01"],
  database: ["M12 8c4.97 0 9-1.34 9-3s-4.03-3-9-3-9 1.34-9 3 4.03 3 9 3z", "M21 5v6c0 1.66-4.03 3-9 3s-9-1.34-9-3V5", "M3 11v6c0 1.66 4.03 3 9 3s9-1.34 9-3v-6"],
  download: ["M12 3v12M6 11l6 6 6-6", "M5 21h14"],
  folder: ["M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"],
};

/** Campos numéricos com limites — inválidos bloqueiam o autosave. */
function collectInvalid(d: Settings): { timeout: boolean; maxSeconds: boolean } {
  const t = d.ai?.timeout_secs;
  return {
    timeout: !!d.ai?.enabled && !(t !== undefined && t >= 2 && t <= 120),
    maxSeconds: !(d.max_seconds >= 3 && d.max_seconds <= 300),
  };
}

function nowHHMM(): string {
  const n = new Date();
  return `${String(n.getHours()).padStart(2, "0")}:${String(n.getMinutes()).padStart(2, "0")}`;
}

function App() {
  const [info, setInfo] = useState<SettingsInfo | null>(null);
  const [draft, setDraft] = useState<Settings | null>(null);
  const [devices, setDevices] = useState<string[]>([]);
  // autosave: máquina de estados exibida no titlebar
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [savedAt, setSavedAt] = useState<string | null>(null);
  const [invalids, setInvalids] = useState<{ timeout: boolean; maxSeconds: boolean }>({
    timeout: false,
    maxSeconds: false,
  });
  const [banner, setBanner] = useState<{ title: string; desc: string } | null>(null);
  const [lastTranscription, setLastTranscription] = useState<string | null>(null);
  const [capturing, setCapturing] = useState(false);
  const [activeSec, setActiveSec] = useState("sec-captura");
  // download do modelo rápido em andamento: bytes recebidos (null = parado)
  const [dl, setDl] = useState<number | null>(null);
  // status do teste de conexão / sync do perfil da IA do servidor
  const [aiStatus, setAiStatus] = useState<string | null>(null);
  // conflito HFP: mic BT selecionado com fone BT na saída ("efeito copo")
  const [btConflict, setBtConflict] = useState<{ input: string; suggestion: string | null } | null>(
    null,
  );
  // versão limpa (polida pela IA) chegando async após a injeção
  const [lastClean, setLastClean] = useState<string | null>(null);
  const { items: toasts, toast, dismiss: dismissToast } = useToasts();

  const draftRef = useRef<Settings | null>(null);
  draftRef.current = draft;
  const saveTimer = useRef<number | null>(null);
  const revealedRef = useRef(false);

  const load = useCallback(async () => {
    const i = await invoke<SettingsInfo>("get_settings");
    setInfo(i);
    setDraft(i.settings);
    invoke<{ input: string; suggestion: string | null } | null>("audio_conflict")
      .then(setBtConflict)
      .catch(() => setBtConflict(null));
  }, []);

  // recarrega apenas o info (status/disponibilidades) — NÃO zera o draft:
  // o autosave chama depois de persistir para não derrubar digitação em curso
  const refresh = useCallback(async () => {
    const i = await invoke<SettingsInfo>("get_settings");
    setInfo(i);
  }, []);

  useEffect(() => {
    // Com a WebView2 quente o primeiro get_settings pode chegar antes do
    // backend concluir a inicialização; sem retry a tela ficava presa em
    // "Carregando…" até o usuário arrastar a janela (foco reinvocava load).
    let tentativas = 6;
    const tentar = () =>
      load().catch((e) => {
        if (tentativas-- > 0) {
          setTimeout(tentar, 150);
        } else {
          setBanner({ title: "Falha ao carregar as configurações", desc: String(e) });
        }
      });
    tentar();
    invoke<string[]>("list_devices")
      .then(setDevices)
      .catch(() => setDevices([]));
  }, [load]);

  // recarrega permissões/status quando a janela ganha foco
  useEffect(() => {
    const onFocus = () => load().catch(() => {});
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [load]);

  // reveals em cascata quando o conteúdo monta (uma única vez)
  useEffect(() => {
    if (draft && !revealedRef.current) {
      revealedRef.current = true;
      observeReveals();
    }
  }, [draft]);

  // resultado de cada ditado (mesmo com a janela fechada, chega ao reabrir)
  useEffect(() => {
    const un = listen<{ ok: boolean; text?: string; error?: string }>("transcription", (ev) => {
      if (ev.payload.ok && ev.payload.text) {
        setLastTranscription(ev.payload.text);
      } else if (ev.payload.error) {
        toast("err", "Falha no ditado", ev.payload.error);
      }
      load().catch(() => {});
    });
    const unClean = listen<{ id: number; text: string }>("clean-version", (ev) => {
      // O painel é a fonte ordenada; aqui mantemos apenas a última versão
      // limpa para não concatenar eventos de lote fora de ordem.
      setLastClean(ev.payload.text);
    });
    return () => {
      un.then((f) => f());
      unClean.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [load]);

  // captura do atalho: próxima combinação de teclas vira o atalho global
  useEffect(() => {
    if (!capturing) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        setCapturing(false);
        return;
      }
      if (MODIFIER_CODES.has(e.code)) return; // espera uma tecla "de verdade"
      const mods: string[] = [];
      if (e.ctrlKey) mods.push("CTRL");
      if (e.altKey) mods.push("ALT");
      if (e.shiftKey) mods.push("SHIFT");
      if (e.metaKey) mods.push("CMD");
      const combo = [...mods, e.code.toUpperCase()].join("+");
      setDraft((d) => (d ? { ...d, hotkey: combo } : d));
      setCapturing(false);
      scheduleSave();
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () => window.removeEventListener("keydown", onKey, { capture: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [capturing]);

  // ---------- autosave ----------
  async function persistDraft() {
    const d = draftRef.current;
    if (!d) return;
    try {
      await invoke("set_settings", { settings: d });
      await refresh();
      setSaveState("saved");
      setSavedAt(nowHHMM());
      setBanner(null);
    } catch (e) {
      setSaveState("error");
      setBanner({
        title: "Não foi possível salvar",
        desc: String(e),
      });
    }
  }

  /** Toda mutação do draft passa por aqui: agenda o persist debounced. */
  function scheduleSave() {
    const d = draftRef.current;
    if (!d) return;
    const bad = collectInvalid(d);
    setInvalids(bad);
    const baseOk = d.hotkey.trim().length > 1 && (d.model_path?.trim().length ?? 0) > 0;
    if (bad.timeout || bad.maxSeconds || !baseOk) {
      setSaveState("invalid");
      return;
    }
    setSaveState("saving");
    if (saveTimer.current !== null) clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => void persistDraft(), 500);
  }

  function retrySave() {
    setBanner(null);
    scheduleSave();
  }

  // progresso do download do modelo rápido ({received} durante, {done} no fim)
  useEffect(() => {
    const un = listen<{ received?: number; done?: boolean; error?: string }>(
      "model-download-progress",
      (ev) => {
        if (ev.payload.done !== undefined) {
          setDl(null);
          if (ev.payload.done) {
            toast("ok", "Modelo rápido instalado", "181 MB · pronto para o texto ao vivo");
            refresh().catch(() => {});
          } else if (ev.payload.error) {
            toast("err", "Download do modelo falhou", ev.payload.error);
          }
        } else if (typeof ev.payload.received === "number") {
          setDl(ev.payload.received);
        }
      },
    );
    return () => {
      un.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function downloadModel() {
    setDl(0);
    try {
      await invoke("download_live_model");
    } catch (e) {
      setDl(null);
      toast("err", "Download do modelo falhou", String(e));
    }
  }

  // teste da conexão com a IA do servidor (resultado chega pelo evento ai-test-result)
  async function testAi() {
    setAiStatus("Testando conexão…");
    try {
      await invoke("test_ai");
    } catch (e) {
      setAiStatus(`Falhou: ${String(e)}`);
    }
  }

  // puxa o perfil de personalização do servidor (resultado chega por profile-synced)
  async function syncProfile() {
    setAiStatus("Sincronizando…");
    try {
      await invoke("sync_profile");
    } catch (e) {
      setAiStatus(`Falhou: ${String(e)}`);
    }
  }

  // resultado do teste de conexão com o Ollama
  useEffect(() => {
    const un = listen<{ ok: boolean; models?: string[]; error?: string }>(
      "ai-test-result",
      (ev) => {
        if (ev.payload.ok) {
          const ms = ev.payload.models ?? [];
          setAiStatus(
            `Conectado ✓${ms.length ? ` — modelos: ${ms.join(", ")}` : " — nenhum modelo instalado"}`,
          );
        } else {
          setAiStatus(`Falhou: ${ev.payload.error ?? "erro desconhecido"}`);
        }
      },
    );
    return () => {
      un.then((f) => f());
    };
  }, []);

  // pull/push do perfil de personalização concluídos no backend
  useEffect(() => {
    const un = listen<{ ok: boolean; error?: string }>("profile-synced", (ev) => {
      if (ev.payload.ok) {
        setAiStatus("Personalização sincronizada ✓");
        toast("ok", "Personalização sincronizada", "Vocabulário e correções atualizados do servidor");
        load().catch(() => {});
      } else {
        setAiStatus(`Sync falhou: ${ev.payload.error ?? "erro desconhecido"}`);
        toast("err", "Sync do perfil falhou", ev.payload.error ?? "erro desconhecido");
      }
    });
    return () => {
      un.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [load]);

  async function grantAccessibility() {
    await invoke("open_accessibility_prefs");
  }

  function navTo(id: string) {
    setActiveSec(id);
    document.getElementById(id)?.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  if (!draft || !info) {
    return <main className="loading">Carregando…</main>;
  }

  const isMac = navigator.userAgent.includes("Macintosh");
  const needsAccessibility = isMac && !info.accessibility;
  const silencePct = ((draft.silence_ms - 600) / (4000 - 600)) * 100;

  function patch(p: Partial<Settings>) {
    setDraft({ ...draft!, ...p });
    scheduleSave();
  }

  // campos aninhados da IA (cria o objeto com defaults ao ligar pela 1ª vez)
  function patchAi(p: Partial<AiSettings>) {
    const ai: AiSettings = { ...DEFAULT_AI, ...(draft!.ai ?? {}), ...p };
    setDraft({ ...draft!, ai });
    scheduleSave();
  }

  function numField(e: React.ChangeEvent<HTMLInputElement>): number {
    const n = Number(e.target.value);
    return Number.isFinite(n) ? n : 0;
  }

  const NAV: { group: string; items: [string, string][] }[] = [
    { group: "captura", items: [["sec-captura", "Atalho e modo"], ["sec-mic", "Microfone"], ["sec-live", "Ao vivo"]] },
    { group: "texto", items: [["sec-texto", "Qualidade e idioma"], ["sec-entrega", "Entrega e reunião"]] },
    { group: "inteligência", items: [["sec-ia", "IA do servidor"], ["sec-dataset", "Dataset e treino"]] },
    { group: "sistema", items: [["sec-modelo", "Modelo e geral"]] },
  ];

  return (
    <main className="container">
      <header data-tauri-drag-region>
        <h1>Fala</h1>
        <p className={`status status--${info.status}`}>{STATUS_LABEL[info.status]}</p>
        <span className="sp" />
        {saveState === "saving" && (
          <span className="save-status"><span className="mini-ring" />salvando…</span>
        )}
        {saveState === "saved" && (
          <span className="save-status ok"><span className="dot-ok" />salvo às {savedAt}</span>
        )}
        {saveState === "error" && <span className="save-status err">falha ao salvar</span>}
        {saveState === "invalid" && <span className="save-status err">campo inválido</span>}
      </header>

      <div className="win-body">
        <nav className="nav">
          {NAV.map((g) => (
            <div key={g.group}>
              <div className="group">{g.group}</div>
              {g.items.map(([id, label]) => (
                <button
                  key={id}
                  className={activeSec === id ? "on" : ""}
                  onClick={() => navTo(id)}
                >
                  <span className="ndot" />
                  {label}
                </button>
              ))}
            </div>
          ))}
        </nav>

        <div className="content">
          {needsAccessibility && (
            <section className="sec reveal">
              <div className="card warn">
                <div className="row">
                  <span className="row-icon t-amb">
                    <Sym d={IC.alert} />
                  </span>
                  <div className="row-col">
                    <span className="row-label">Permissão necessária</span>
                    <span className="row-sub">
                      Para colar o texto no app em foco, ative o Fala em Ajustes → Privacidade e
                      Segurança → Acessibilidade.
                    </span>
                  </div>
                </div>
                <div className="row">
                  <button onClick={grantAccessibility}>Abrir Ajustes de Acessibilidade</button>
                </div>
              </div>
            </section>
          )}

          <section className="sec reveal stag" id="sec-captura">
            <h2>Atalho e modo</h2>
            <p className="ph">Como a captura começa e termina</p>
            <div className="card">
              <div className="row">
                <span className="row-icon t-blue">
                  <Sym d={IC.command} />
                </span>
                <div className="row-col">
                  <span className="row-label">Atalho de ditado</span>
                  <span className="row-sub">
                    Clique na caixa e pressione a combinação — Esc cancela
                  </span>
                </div>
                <span className="row-end">
                  <button
                    className={capturing ? "hotkey-box capturing" : "hotkey-box"}
                    onClick={() => setCapturing(true)}
                  >
                    {capturing ? "Pressione…" : prettyHotkey(draft.hotkey)}
                  </button>
                </span>
              </div>
              <p className="row-hint">
                No Mac, a tecla <code>alt</code> é a ⌥ Option.
              </p>
              <div className="row">
                <span className="row-icon t-rec">
                  <Sym d={IC.wave} />
                </span>
                <div className="row-col">
                  <span className="row-label">Modo</span>
                  <span className="row-sub">
                    {draft.mode === "toggle"
                      ? "Aperta uma vez; para sozinho ao silenciar"
                      : "Segure enquanto fala"}
                  </span>
                </div>
                <span className="row-end">
                  <select value={draft.mode} onChange={(e) => patch({ mode: e.target.value })}>
                    <option value="toggle">Automático</option>
                    <option value="push">Push-to-talk</option>
                  </select>
                </span>
              </div>
              {draft.mode === "toggle" && (
                <div className="row">
                  <span className="row-icon">
                    <Sym d={IC.timer} />
                  </span>
                  <div className="row-col"><span className="row-label">Encerrar após silêncio</span></div>
                  <span className="row-end">
                    <input
                      type="range"
                      min={600}
                      max={4000}
                      step={100}
                      value={draft.silence_ms}
                      style={{ "--pct": `${silencePct}%` } as CSSProperties}
                      onChange={(e) => patch({ silence_ms: Number(e.target.value) })}
                    />
                    <span className="row-value">
                      {(draft.silence_ms / 1000).toFixed(1).replace(".", ",")} s
                    </span>
                  </span>
                </div>
              )}
              <div className="row">
                <span className="row-icon t-amb">
                  <Sym d={IC.alert} />
                </span>
                <div className="row-col">
                  <span className="row-label">Limite da gravação</span>
                  <span className="row-sub">Encerra caso o “soltar” se perca</span>
                </div>
                <span className="row-end">
                  <input
                    className={`narrow${invalids.maxSeconds ? " bad" : ""}`}
                    type="number"
                    min={3}
                    max={300}
                    value={draft.max_seconds}
                    onChange={(e) => patch({ max_seconds: numField(e) })}
                  />
                  <span className="row-value">seg</span>
                </span>
              </div>
              {invalids.maxSeconds && (
                <div className="field-pad">
                  <div className="err-msg">
                    <Sym d={IC.alert} size={12} sw={2.2} />
                    Entre 3 e 300 segundos — o autosave aguarda um valor válido
                  </div>
                </div>
              )}
            </div>
          </section>

          <section className="sec reveal stag" id="sec-mic">
            <h2>Microfone</h2>
            <p className="ph">Entrada de áudio e diagnóstico</p>
            <div className="card">
              <div className="row">
                <span className="row-icon t-rec">
                  <Sym d={IC.mic} />
                </span>
                <div className="row-col">
                  <span className="row-label">Entrada de áudio</span>
                  <span className="row-sub">Padrão do sistema ou dispositivo específico</span>
                </div>
                <span className="row-end">
                  <select
                    value={draft.device ?? ""}
                    onChange={(e) => patch({ device: e.target.value || null })}
                  >
                    <option value="">Padrão do sistema</option>
                    {devices.map((d) => (
                      <option key={d} value={d}>
                        {d}
                      </option>
                    ))}
                  </select>
                </span>
              </div>
              {btConflict && (
                <div className="row">
                  <span className="row-icon t-amb">
                    <Sym d={IC.alert} />
                  </span>
                  <div className="row-col">
                    <span className="row-label">Conflito de áudio Bluetooth</span>
                    <span className="row-sub">
                      O microfone de “{btConflict.input}” força o fone para o perfil
                      de ligação e degrada a música enquanto o Fala grava
                      (“efeito copo”).
                      {btConflict.suggestion ? ` Sugestão: ${btConflict.suggestion}.` : ""}
                    </span>
                  </div>
                  {btConflict.suggestion && (
                    <span className="row-end">
                      <button onClick={() => patch({ device: btConflict.suggestion! })}>
                        Trocar microfone
                      </button>
                    </span>
                  )}
                </div>
              )}
            </div>
          </section>

          <section className="sec reveal stag" id="sec-live">
            <h2>Transcrição ao vivo</h2>
            <p className="ph">Palavras aparecendo enquanto você fala</p>
            <div className="card">
              <div className="row">
                <span className="row-icon t-blue">
                  <Sym d={IC.wave} />
                </span>
                <div className="row-col">
                  <span className="row-label">Palavras ao vivo</span>
                  <span className="row-sub">
                    Cada pausa confirma o trecho sem refazer nada. Com Clipboard ou IA,
                    os trechos ficam em memória e são entregues uma vez no final.
                  </span>
                </div>
                <span className="row-end">
                  <label className="switch">
                    <input
                      type="checkbox"
                      checked={draft.live_transcribe}
                      onChange={(e) => patch({ live_transcribe: e.target.checked })}
                    />
                    <span />
                    <span />
                  </label>
                </span>
              </div>
              <div className="row">
                <span className="row-icon t-rose">
                  <Sym d={IC.download} />
                </span>
                <div className="row-col">
                  <span className="row-label">Modelo rápido</span>
                  <span className="row-sub">
                    {info.live_model_available
                      ? "small-q5_1 disponível ✓"
                      : dl !== null
                        ? `Baixando… ${(dl / 1_000_000).toFixed(0)} MB`
                        : "Necessário para o texto ao vivo (~181 MB, download único)"}
                  </span>
                </div>
                <span className="row-end">
                  {!info.live_model_available && dl === null && (
                    <button onClick={downloadModel}>Baixar modelo</button>
                  )}
                </span>
              </div>
            </div>
          </section>

          <section className="sec reveal stag" id="sec-texto">
            <h2>Qualidade e idioma</h2>
            <p className="ph">Vocabulário, correções e idioma da fala</p>
            <div className="card">
              <div className="row">
                <span className="row-icon t-rose">
                  <Sym d={IC.sparkles} />
                </span>
                <span className="row-label">Vocabulário personalizado</span>
              </div>
              <div className="field-pad">
                <textarea
                  rows={3}
                  value={draft.vocab_prompt ?? ""}
                  onChange={(e) => patch({ vocab_prompt: e.target.value || null })}
                  placeholder="Nomes e termos técnicos — ex. João Vitor, ZCode, whisper.cpp, Tauri…"
                  spellCheck={false}
                />
              </div>
              <div className="row">
                <span className="row-label">Correções automáticas</span>
              </div>
              <div className="field-pad">
                <textarea
                  rows={3}
                  value={draft.corrections ?? ""}
                  onChange={(e) => patch({ corrections: e.target.value || null })}
                  placeholder={"Uma por linha:\ntou gravando = tô gravando\nBala cuda = Barracuda"}
                  spellCheck={false}
                />
              </div>
              <div className="row">
                <span className="row-icon t-blue">
                  <Sym d={IC.globe} />
                </span>
                <span className="row-label">Idioma da fala</span>
                <span className="row-end">
                  <select value={draft.language} onChange={(e) => patch({ language: e.target.value })}>
                    {IDIOMAS.map(([v, label]) => (
                      <option key={v} value={v}>
                        {label}
                      </option>
                    ))}
                  </select>
                </span>
              </div>
            </div>
            <p className="hint">
              {draft.ai?.enabled && draft.ai.profile_url.trim() ? (
                "Sincronizado com o servidor: salvar envia vocabulário e correções para todos os dispositivos."
              ) : (
                <>
                  Correções no formato <code>errado = correto</code>, aplicadas ao texto final.
                </>
              )}
            </p>
          </section>

          <section className="sec reveal stag" id="sec-entrega">
            <h2>Entrega e reunião</h2>
            <p className="ph">Para onde o resultado vai ao soltar o atalho</p>
            <div className="card">
              <div className="row">
                <span className="row-icon t-grn">
                  <Sym d={IC.clipboard} />
                </span>
                <div className="row-col">
                  <span className="row-label">
                    {draft.paste_mode === "clipboard" ? "Guardar no Clipboard" : "Inserir no campo focado"}
                  </span>
                  <span className="row-sub">
                    {draft.paste_mode === "clipboard"
                      ? "Deixa o resultado final no clipboard para você colar manualmente"
                      : "Digita diretamente no campo atualmente focado — mais compatível"}
                  </span>
                </div>
                <span className="row-end">
                  <select
                    value={draft.paste_mode}
                    onChange={(e) => patch({ paste_mode: e.target.value })}
                  >
                    <option value="clipboard">Guardar no Clipboard</option>
                    <option value="type">Inserir no campo</option>
                  </select>
                </span>
              </div>
              <div className="row">
                <span className="row-icon">
                  <Sym d={IC.wave} />
                </span>
                <div className="row-col">
                  <span className="row-label">Capturar sem colar (reunião)</span>
                  <span className="row-sub">
                    Nada é colado em outros apps — a transcrição acumula no painel
                    “Versão limpa” da tray, com a IA limpando cada trecho. Copie no final.
                  </span>
                </div>
                <span className="row-end">
                  <label className="switch">
                    <input
                      type="checkbox"
                      checked={draft.meeting_mode}
                      onChange={(e) => patch({ meeting_mode: e.target.checked })}
                    />
                    <span />
                    <span />
                  </label>
                </span>
              </div>
            </div>
          </section>

          <section className="sec reveal stag" id="sec-ia">
            <h2>IA do servidor</h2>
            <p className="ph">Polimento LLM — quando ativa, a entrega aguarda a versão limpa</p>
            <div className="card">
              <div className="row">
                <span className="row-icon t-rose">
                  <Sym d={IC.sparkles} />
                </span>
                <div className="row-col">
                  <span className="row-label">Polimento com LLM</span>
                  <span className="row-sub">
                    A revisão final passa pela IA servida no seu servidor — um só
                    cérebro e um só perfil para todos os dispositivos
                  </span>
                </div>
                <span className="row-end">
                  <label className="switch">
                    <input
                      type="checkbox"
                      checked={draft.ai?.enabled ?? false}
                      onChange={(e) => patchAi({ enabled: e.target.checked })}
                    />
                    <span />
                    <span />
                  </label>
                </span>
              </div>
              {draft.ai?.enabled && (
                <>
                  <div className="field-pad">
                    <input
                      value={draft.ai.endpoint}
                      onChange={(e) => patchAi({ endpoint: e.target.value })}
                      placeholder="Ollama — ex. http://fala-servidor:11434"
                      spellCheck={false}
                    />
                    <input
                      value={draft.ai.profile_url}
                      onChange={(e) => patchAi({ profile_url: e.target.value })}
                      placeholder="Personalização — ex. http://fala-servidor:8338"
                      spellCheck={false}
                    />
                    <input
                      type="password"
                      value={draft.ai.token}
                      onChange={(e) => patchAi({ token: e.target.value })}
                      placeholder="Token do serviço de personalização"
                      spellCheck={false}
                    />
                    <input
                      value={draft.ai.model}
                      onChange={(e) => patchAi({ model: e.target.value })}
                      placeholder="Modelo — ex. llama3.1:8b-instruct-q4_K_M"
                      spellCheck={false}
                    />
                  </div>
                  <div className="row">
                    <span className="row-col">
                      <span className="row-label">Sincronizar perfil ao iniciar</span>
                      <span className="row-sub">
                        Vocabulário, correções e prompt vêm do servidor (servidor vence no boot)
                      </span>
                    </span>
                    <span className="row-end">
                      <label className="switch">
                        <input
                          type="checkbox"
                          checked={draft.ai.sync_profile}
                          onChange={(e) => patchAi({ sync_profile: e.target.checked })}
                        />
                        <span />
                        <span />
                      </label>
                    </span>
                  </div>
                  <div className="row">
                    <span className="row-label">Timeout do polimento</span>
                    <span className="row-end">
                      <input
                        className={`narrow${invalids.timeout ? " bad" : ""}`}
                        type="number"
                        min={2}
                        max={120}
                        value={draft.ai.timeout_secs}
                        onChange={(e) => patchAi({ timeout_secs: numField(e) })}
                      />
                      <span className="row-value">seg</span>
                    </span>
                  </div>
                  {invalids.timeout && (
                    <div className="field-pad">
                      <div className="err-msg">
                        <Sym d={IC.alert} size={12} sw={2.2} />
                        Entre 2 e 120 segundos — o autosave aguarda um valor válido
                      </div>
                    </div>
                  )}
                  <div className="row">
                    <span className="row-col">
                      <span className="row-label">Entrega após o polimento</span>
                      <span className="row-sub">
                        Quando a IA está ativa, o Fala sempre aguarda a versão limpa
                        antes de entregar. Se o servidor falhar, usa o texto original.
                      </span>
                    </span>
                    <span className="row-end row-value">Automática</span>
                  </div>
                  <div className="row">
                    <span className="row-label">Prompt de sistema da IA</span>
                  </div>
                  <div className="field-pad">
                    <textarea
                      rows={3}
                      value={draft.ai.system_prompt ?? ""}
                      onChange={(e) => patchAi({ system_prompt: e.target.value || null })}
                      placeholder="Instruções de polimento — salvas também no servidor"
                      spellCheck={false}
                    />
                  </div>
                  <div className="row">
                    <button onClick={testAi}>Testar conexão</button>
                    <button onClick={syncProfile}>Sincronizar agora</button>
                  </div>
                  {aiStatus && <p className="row-hint">{aiStatus}</p>}
                </>
              )}
            </div>
            <p className="hint">
              Sem IA (ou servidor fora do ar), a entrega usa as correções locais;
              quando a IA está ativa, o resultado só é entregue após o polimento.
            </p>
          </section>

          <section className="sec reveal stag" id="sec-dataset">
            <h2>Dataset e treino</h2>
            <p className="ph">Coleta de áudios para fine-tuning do seu modelo</p>
            <div className="card">
              <div className="row">
                <span className="row-icon t-blue">
                  <Sym d={IC.database} />
                </span>
                <div className="row-col">
                  <span className="row-label">Coletar áudios para Fine-Tuning</span>
                  <span className="row-sub">
                    Salva os áudios e transcrições dos seus ditados reais em ~/.fala/dataset para treinar um modelo exclusivo
                  </span>
                </div>
                <span className="row-end">
                  <label className="switch">
                    <input
                      type="checkbox"
                      checked={draft.save_dataset}
                      onChange={(e) => patch({ save_dataset: e.target.checked })}
                    />
                    <span />
                    <span />
                  </label>
                </span>
              </div>
              <div className="row">
                <span className="row-icon">
                  <Sym d={IC.folder} />
                </span>
                <div className="row-col">
                  <span className="row-label">Amostras coletadas</span>
                  <span className="row-sub">
                    {info.dataset_stats && info.dataset_stats.total_samples > 0
                      ? `${info.dataset_stats.total_samples} gravações (~${(info.dataset_stats.total_duration_secs / 60).toFixed(1)} min de áudio)`
                      : "Nenhuma amostra salva ainda — ative a opção acima para começar"}
                  </span>
                </div>
                <span className="row-end">
                  <button onClick={() => invoke("open_dataset_folder").catch((e) => toast("err", "Falha ao abrir a pasta", String(e)))}>
                    Abrir pasta
                  </button>
                </span>
              </div>
            </div>
            <p className="hint">
              As gravações são salvas localmente em 16 kHz mono. Quando atingir ~100 amostras,
              execute o script em <code>training/train_lora.py</code> para gerar seu modelo customizado.
            </p>
          </section>

          <section className="sec reveal stag" id="sec-modelo">
            <h2>Modelo e geral</h2>
            <p className="ph">Modelo Whisper local e preferências do sistema</p>
            <div className="card">
              <div className="row">
                <span className="row-icon t-rose">
                  <Sym d={IC.cpu} />
                </span>
                <div className="row-col">
                  <span className="row-label">Modelo local</span>
                  <span className="row-sub">Arquivo .bin ggml — vazio usa o padrão detectado</span>
                </div>
              </div>
              <div className="field-pad">
                <input
                  value={draft.model_path ?? ""}
                  onChange={(e) => patch({ model_path: e.target.value || null })}
                  placeholder="/caminho/para/ggml-…-q5_0.bin"
                  spellCheck={false}
                />
              </div>
              <div className="row">
                <span className="row-icon">
                  <Sym d={IC.power} />
                </span>
                <div className="row-col">
                  <span className="row-label">Iniciar com o sistema</span>
                  <span className="row-sub">Abre o Fala na bandeja ao fazer login</span>
                </div>
                <span className="row-end">
                  <label className="switch">
                    <input
                      type="checkbox"
                      checked={draft.autostart}
                      onChange={(e) => patch({ autostart: e.target.checked })}
                    />
                    <span />
                    <span />
                  </label>
                </span>
              </div>
            </div>
          </section>

          {lastTranscription && (
            <section className="sec reveal">
              <h2>Última transcrição</h2>
              <p className="ph">Resultado do ditado mais recente</p>
              <div className="card">
                <blockquote>“{lastTranscription}”</blockquote>
                {lastClean && (
                  <>
                    <div className="row">
                      <span className="row-icon t-rose">
                        <Sym d={IC.sparkles} />
                      </span>
                      <span className="row-label">Versão limpa (IA)</span>
                    </div>
                    <div className="field-pad">
                      <textarea rows={3} value={lastClean} readOnly spellCheck={false} />
                    </div>
                  </>
                )}
              </div>
            </section>
          )}

          {banner && (
            <div className="banner" id="saveBanner">
              <Sym d={IC.alert} size={15} sw={2.2} />
              <div className="t">
                {banner.title}
                <div className="d">{banner.desc}</div>
              </div>
              <button onClick={retrySave}>Tentar de novo</button>
            </div>
          )}

          <p className="footer-path">{info.config_path}</p>
        </div>
      </div>

      <ToastHost items={toasts} onDismiss={dismissToast} />
    </main>
  );
}

export default App;
