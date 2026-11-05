import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

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
}

interface SettingsInfo {
  settings: Settings;
  config_path: string;
  status: "idle" | "recording" | "transcribing";
  accessibility: boolean;
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
  idle: "Pronto — segure o atalho para ditar",
  recording: "● Gravando…",
  transcribing: "… Transcrevendo…",
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

function App() {
  const [info, setInfo] = useState<SettingsInfo | null>(null);
  const [draft, setDraft] = useState<Settings | null>(null);
  const [devices, setDevices] = useState<string[]>([]);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastTranscription, setLastTranscription] = useState<string | null>(null);
  const [capturing, setCapturing] = useState(false);

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

  async function grantAccessibility() {
    await invoke("open_accessibility_prefs");
  }

  if (!draft || !info) {
    return <main className="loading">Carregando…</main>;
  }

  const isMac = navigator.userAgent.includes("Macintosh");
  const needsAccessibility = isMac && !info.accessibility;
  const valid = draft.hotkey.trim().length > 1 && (draft.model_path?.trim().length ?? 0) > 0;

  function patch(p: Partial<Settings>) {
    setDraft({ ...draft!, ...p });
  }

  return (
    <main className="container">
      <header>
        <h1>🎙️ Fala</h1>
        <p className={info.status === "idle" ? "status idle" : "status active"}>
          {STATUS_LABEL[info.status]}
        </p>
      </header>

      {needsAccessibility && (
        <section className="card warn">
          <h2>⚠️ Permissão necessária</h2>
          <p>
            Para colar o texto no app em foco, o Fala precisa de <b>Acessibilidade</b>. Ative o Fala
            em: Ajustes → Privacidade e Segurança → Acessibilidade. Clique abaixo para abrir e
            depois volte aqui.
          </p>
          <button onClick={grantAccessibility}>Abrir Ajustes de Acessibilidade</button>
        </section>
      )}

      <section className="card">
        <h2>Atalho global</h2>
        <p className="hint">
          Clique abaixo e pressione a combinação que quiser — no Mac, a tecla <code>alt</code> é a{" "}
          <b>⌥ Option</b>.
        </p>
        <button
          className={capturing ? "capturing" : "hotkey-box"}
          onClick={() => setCapturing(true)}
        >
          {capturing ? "⌨️ Pressione a combinação… (Esc cancela)" : prettyHotkey(draft.hotkey)}
        </button>
      </section>

      <section className="card">
        <h2>Modo de captura</h2>
        <select value={draft.mode} onChange={(e) => patch({ mode: e.target.value })}>
          <option value="toggle">Automático — aperta uma vez, para sozinho ao silenciar</option>
          <option value="push">Push-to-talk — segure enquanto fala</option>
        </select>
        {draft.mode === "toggle" && (
          <>
            <p className="hint" style={{ marginTop: 10 }}>
              Encerra sozinho após {(draft.silence_ms / 1000).toFixed(1)}s de silêncio (depois de
              começar a falar)
            </p>
            <input
              type="range"
              min={600}
              max={4000}
              step={100}
              value={draft.silence_ms}
              onChange={(e) => patch({ silence_ms: Number(e.target.value) })}
            />
          </>
        )}
      </section>

      <section className="card">
        <h2>Idioma da fala</h2>
        <select
          value={draft.language}
          onChange={(e) => patch({ language: e.target.value })}
        >
          {IDIOMAS.map(([v, label]) => (
            <option key={v} value={v}>
              {label}
            </option>
          ))}
        </select>
      </section>

      <section className="card">
        <h2>Microfone</h2>
        <select
          value={draft.device ?? ""}
          onChange={(e) => patch({ device: e.target.value || null })}
        >
          <option value="">Padrão do sistema (fone quando conectado)</option>
          {devices.map((d) => (
            <option key={d} value={d}>
              {d}
            </option>
          ))}
        </select>
      </section>

      <section className="card">
        <h2>Modelo Whisper (local)</h2>
        <p className="hint">
          Caminho do arquivo <code>.bin</code> ggml. Deixe vazio e salve para voltar ao padrão
          detectado.
        </p>
        <input
          value={draft.model_path ?? ""}
          onChange={(e) => patch({ model_path: e.target.value || null })}
          placeholder="/caminho/para/ggml-…-q5_0.bin"
          spellCheck={false}
        />
      </section>

      <section className="card">
        <h2>Inserção do texto</h2>
        <select
          value={draft.paste_mode}
          onChange={(e) => patch({ paste_mode: e.target.value })}
        >
          <option value="clipboard">Colar (clipboard) — rápido, recomendado</option>
          <option value="type">Digitar caractere a caractere — mais compatível</option>
        </select>
      </section>

      <section className="card row">
        <div>
          <h2>Iniciar com o sistema</h2>
          <p className="hint">Abre o Fala na bandeja ao fazer login.</p>
        </div>
        <label className="switch">
          <input
            type="checkbox"
            checked={draft.autostart}
            onChange={(e) => patch({ autostart: e.target.checked })}
          />
          <span />
        </label>
      </section>

      <section className="card row">
        <div>
          <h2>Limite da gravação</h2>
          <p className="hint">Segurança caso o teclado engula o “soltar”.</p>
        </div>
        <input
          className="narrow"
          type="number"
          min={3}
          max={300}
          value={draft.max_seconds}
          onChange={(e) => patch({ max_seconds: Number(e.target.value) || 120 })}
        />
      </section>

      {lastTranscription && (
        <section className="card">
          <h2>Última transcrição</h2>
          <blockquote>“{lastTranscription}”</blockquote>
        </section>
      )}

      {error && <p className="error">{error}</p>}

      <footer>
        <button className="primary" disabled={!valid || saved} onClick={save}>
          {saved ? "Salvo ✓" : "Salvar"}
        </button>
        <p className="hint">{info.config_path}</p>
      </footer>
    </main>
  );
}

export default App;
