import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

interface Settings {
  hotkey: string;
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

function App() {
  const [info, setInfo] = useState<SettingsInfo | null>(null);
  const [draft, setDraft] = useState<Settings | null>(null);
  const [devices, setDevices] = useState<string[]>([]);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastTranscription, setLastTranscription] = useState<string | null>(null);

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
        <h2>Atalho global (push-to-talk)</h2>
        <p className="hint">
          Segure para gravar, solte para transcrever e colar. Formato: modificadores + tecla, ex.{" "}
          <code>alt+space</code>, <code>ctrl+shift+d</code>.
        </p>
        <input
          value={draft.hotkey}
          onChange={(e) => patch({ hotkey: e.target.value })}
          placeholder="alt+space"
          spellCheck={false}
        />
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
