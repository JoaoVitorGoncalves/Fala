import { useCallback, useEffect, useState } from "react";
import type { CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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
  ai: AiSettings | null;
}

interface SettingsInfo {
  settings: Settings;
  config_path: string;
  status: "idle" | "recording" | "transcribing";
  accessibility: boolean;
  live_model_available: boolean;
  live_model_path: string;
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

/* glifos estilo SF Symbols (stroke, não emoji) */
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
};

function App() {
  const [info, setInfo] = useState<SettingsInfo | null>(null);
  const [draft, setDraft] = useState<Settings | null>(null);
  const [devices, setDevices] = useState<string[]>([]);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastTranscription, setLastTranscription] = useState<string | null>(null);
  const [capturing, setCapturing] = useState(false);
  // download do modelo rápido em andamento: bytes recebidos (null = parado)
  const [dl, setDl] = useState<number | null>(null);
  // status do teste de conexão / sync do perfil da IA do servidor
  const [aiStatus, setAiStatus] = useState<string | null>(null);

  const load = useCallback(async () => {
    const i = await invoke<SettingsInfo>("get_settings");
    setInfo(i);
    setDraft(i.settings);
  }, []);

  useEffect(() => {
    load().catch((e) => setError(String(e)));
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

  // resultado de cada ditado (mesmo com a janela fechada, chega ao reabrir)
  useEffect(() => {
    const un = listen<{ ok: boolean; text?: string; error?: string }>("transcription", (ev) => {
      if (ev.payload.ok && ev.payload.text) {
        setLastTranscription(ev.payload.text);
      } else if (ev.payload.error) {
        setError(ev.payload.error);
      }
      load().catch(() => {});
    });
    return () => {
      un.then((f) => f());
    };
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
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () => window.removeEventListener("keydown", onKey, { capture: true });
  }, [capturing]);

  async function save() {
    if (!draft) return;
    setError(null);
    try {
      await invoke("set_settings", { settings: draft });
      await load();
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch (e) {
      setError(String(e));
    }
  }

  // progresso do download do modelo rápido ({received} durante, {done} no fim)
  useEffect(() => {
    const un = listen<{ received?: number; done?: boolean; error?: string }>(
      "model-download-progress",
      (ev) => {
        if (ev.payload.done !== undefined) {
          setDl(null);
          if (ev.payload.done) {
            load().catch(() => {});
          } else if (ev.payload.error) {
            setError(ev.payload.error);
          }
        } else if (typeof ev.payload.received === "number") {
          setDl(ev.payload.received);
        }
      },
    );
    return () => {
      un.then((f) => f());
    };
  }, [load]);

  async function downloadModel() {
    setError(null);
    setDl(0);
    try {
      await invoke("download_live_model");
    } catch (e) {
      setDl(null);
      setError(String(e));
    }
  }

  // teste da conexão com a IA do servidor (resultado chega pelo evento ai-test-result)
  async function testAi() {
    setError(null);
    setAiStatus("Testando conexão…");
    try {
      await invoke("test_ai");
    } catch (e) {
      setAiStatus(`Falhou: ${String(e)}`);
    }
  }

  // puxa o perfil de personalização do servidor (resultado chega por profile-synced)
  async function syncProfile() {
    setError(null);
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
        load().catch(() => {});
      } else {
        setAiStatus(`Sync falhou: ${ev.payload.error ?? "erro desconhecido"}`);
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [load]);

  async function grantAccessibility() {
    await invoke("open_accessibility_prefs");
  }

  if (!draft || !info) {
    return <main className="loading">Carregando…</main>;
  }

  const isMac = navigator.userAgent.includes("Macintosh");
  const needsAccessibility = isMac && !info.accessibility;
  const valid = draft.hotkey.trim().length > 1 && (draft.model_path?.trim().length ?? 0) > 0;
  const silencePct = ((draft.silence_ms - 600) / (4000 - 600)) * 100;

  function patch(p: Partial<Settings>) {
    setDraft({ ...draft!, ...p });
  }

  // campos aninhados da IA (cria o objeto com defaults ao ligar pela 1ª vez)
  function patchAi(p: Partial<AiSettings>) {
    const ai: AiSettings = { ...DEFAULT_AI, ...(draft!.ai ?? {}), ...p };
    setDraft({ ...draft!, ai });
  }

  return (
    <main className="container">
      <header data-tauri-drag-region>
        <h1>Fala</h1>
        <p className={`status status--${info.status}`}>{STATUS_LABEL[info.status]}</p>
      </header>

      {needsAccessibility && (
        <section className="group">
          <div className="card warn">
            <div className="row">
              <span className="row-icon t-orange">
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

      <section className="group">
        <h2>Atalho global</h2>
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
        </div>
      </section>

      <section className="group">
        <h2>Modo de captura</h2>
        <div className="card">
          <div className="row">
            <span className="row-icon t-indigo">
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
              <span className="row-label">Encerrar após silêncio</span>
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
        </div>
      </section>

      <section className="group">
        <h2>Qualidade da transcrição</h2>
        <div className="card">
          <div className="row">
            <span className="row-icon t-pink">
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

      <section className="group">
        <h2>IA do servidor</h2>
        <div className="card">
          <div className="row">
            <span className="row-icon t-purple">
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
                  </label>
                </span>
              </div>
              <div className="row">
                <span className="row-label">Timeout do polimento</span>
                <span className="row-end">
                  <input
                    className="narrow"
                    type="number"
                    min={2}
                    max={120}
                    value={draft.ai.timeout_secs}
                    onChange={(e) => patchAi({ timeout_secs: Number(e.target.value) || 20 })}
                  />
                  <span className="row-value">seg</span>
                </span>
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
          Sem IA (ou servidor fora do ar), o texto entra só com as correções
          locais — o ditado nunca depende da rede.
        </p>
      </section>

      <section className="group">
        <h2>Transcrição em tempo real</h2>
        <div className="card">
          <div className="row">
            <span className="row-icon t-indigo">
              <Sym d={IC.wave} />
            </span>
            <div className="row-col">
              <span className="row-label">Palavras ao vivo</span>
              <span className="row-sub">
                Cada pausa confirma o trecho falado direto no input — sem refazer
                nada; só o final recebe uma revisão rápida
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
              </label>
            </span>
          </div>
          <div className="row">
            <span className="row-icon t-purple">
              <Sym d={IC.cpu} />
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
        <p className="hint">
          Os trechos ao vivo usam um modelo pequeno e rápido; a revisão do
          trecho final usa o modelo Whisper configurado acima.
        </p>
      </section>

      <section className="group">
        <h2>Idioma</h2>
        <div className="card">
          <div className="row">
            <span className="row-icon t-teal">
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
      </section>

      <section className="group">
        <h2>Microfone</h2>
        <div className="card">
          <div className="row">
            <span className="row-icon t-red">
              <Sym d={IC.mic} />
            </span>
            <span className="row-label">Entrada de áudio</span>
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
        </div>
      </section>

      <section className="group">
        <h2>Modelo Whisper</h2>
        <div className="card">
          <div className="row">
            <span className="row-icon t-purple">
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
        </div>
      </section>

      <section className="group">
        <h2>Inserção do texto</h2>
        <div className="card">
          <div className="row">
            <span className="row-icon t-green">
              <Sym d={IC.clipboard} />
            </span>
            <div className="row-col">
              <span className="row-label">Entrega do texto</span>
              <span className="row-sub">
                {draft.paste_mode === "clipboard"
                  ? "Cola via clipboard — rápido"
                  : "Digita caractere a caractere — mais compatível"}
              </span>
            </div>
            <span className="row-end">
              <select
                value={draft.paste_mode}
                onChange={(e) => patch({ paste_mode: e.target.value })}
              >
                <option value="clipboard">Colar</option>
                <option value="type">Digitar</option>
              </select>
            </span>
          </div>
        </div>
      </section>

      <section className="group">
        <h2>Geral</h2>
        <div className="card">
          <div className="row">
            <span className="row-icon t-gray">
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
              </label>
            </span>
          </div>
          <div className="row">
            <span className="row-icon t-orange">
              <Sym d={IC.timer} />
            </span>
            <div className="row-col">
              <span className="row-label">Limite da gravação</span>
              <span className="row-sub">Encerra caso o “soltar” se perca</span>
            </div>
            <span className="row-end">
              <input
                className="narrow"
                type="number"
                min={3}
                max={300}
                value={draft.max_seconds}
                onChange={(e) => patch({ max_seconds: Number(e.target.value) || 120 })}
              />
              <span className="row-value">seg</span>
            </span>
          </div>
        </div>
      </section>

      {lastTranscription && (
        <section className="group">
          <h2>Última transcrição</h2>
          <div className="card">
            <blockquote>“{lastTranscription}”</blockquote>
          </div>
        </section>
      )}

      {error && <p className="error">{error}</p>}

      <footer>
        <p className="footer-path">{info.config_path}</p>
        <button className="primary" disabled={!valid || saved} onClick={save}>
          {saved ? "Salvo ✓" : "Salvar"}
        </button>
      </footer>
    </main>
  );
}

export default App;
