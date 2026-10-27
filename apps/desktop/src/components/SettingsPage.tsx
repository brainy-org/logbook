// The settings: where meetings are transcribed, what is recorded, whether
// they go to brainy, and the language model that cleans them up and
// writes the report; then the voices known by name. Secrets (the
// server's and brainy's tokens, the model's key) are written to the keychain and never read
// back: the page only knows whether they are set.
import { useEffect, useState, type ReactNode } from 'react';
import { Check, Laptop, Server, Trash2 } from 'lucide-react';
import { ask } from '@tauri-apps/plugin-dialog';
import { api, type Devices, type Settings, type SettingsView, type Voice } from '../lib/api.js';
import {
  fieldClass,
  mutedTextClass,
  primaryButtonClass,
  quietDangerButtonClass,
  secondaryButtonSmClass,
} from './form-styles.js';

/** Hosted models the report and the clean-up were tried with. */
const PRESETS = [
  { label: 'Groq · Qwen3.8 27B', url: 'https://api.groq.com/openai/v1', name: 'qwen/qwen3.8-27b' },
  { label: 'Ollama (this computer)', url: 'http://localhost:11434/v1', name: '' },
];

type Check =
  | { state: 'idle' }
  | { state: 'checking' }
  | { state: 'ok' }
  | { state: 'failed'; message: string };

export function SettingsPage({ recording }: { recording: boolean }) {
  const [view, setView] = useState<SettingsView | null>(null);
  const [s, setS] = useState<Settings | null>(null);
  const [devices, setDevices] = useState<Devices>({ inputs: [], outputs: [] });
  const [voices, setVoices] = useState<Voice[]>([]);
  const [token, setToken] = useState<string | undefined>(undefined);
  const [key, setKey] = useState<string | undefined>(undefined);
  const [serverToken, setServerToken] = useState<string | undefined>(undefined);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [serverCheck, setServerCheck] = useState<Check>({ state: 'idle' });
  const [brainyCheck, setBrainyCheck] = useState<Check>({ state: 'idle' });

  useEffect(() => {
    api.getSettings().then((v) => {
      setView(v);
      setS(v.settings);
    });
    api.listDevices().then(setDevices, () => undefined);
    api.listVoices().then(setVoices, () => undefined);
  }, []);

  if (!s || !view) return null;

  const update = (change: (s: Settings) => void) => {
    const next = structuredClone(s);
    change(next);
    setS(next);
    setSaved(false);
  };

  const save = async () => {
    setError(null);
    try {
      const v = await api.saveSettings(s, token, key, serverToken);
      setView(v);
      setS(v.settings);
      setToken(undefined);
      setKey(undefined);
      setServerToken(undefined);
      setSaved(true);
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  };

  const run = async (set: (c: Check) => void, check: () => Promise<void>) => {
    set({ state: 'checking' });
    try {
      await check();
      set({ state: 'ok' });
    } catch (e) {
      set({ state: 'failed', message: String(e) });
    }
  };

  const local = s.transcription === 'local';

  return (
    <div className="flex h-full flex-col">
      <div data-tauri-drag-region className="h-11 shrink-0" />
      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto max-w-2xl space-y-10 px-10 pb-24">
          <h1 className="text-2xl font-semibold tracking-tight">Settings</h1>
          {recording && (
            <p className="rounded-md bg-amber-50 px-3 py-2 text-sm dark:bg-amber-950/30">
              A meeting is being recorded: changes apply to the next one.
            </p>
          )}

          <Section title="Transcription">
            <div className="grid grid-cols-2 gap-3">
              <Choice
                selected={local}
                onClick={() => update((n) => (n.transcription = 'local'))}
                icon={<Laptop className="size-5" />}
                title="On this computer"
                text="The models (about 1 GB) are fetched once and run here. Nothing leaves the computer for the transcript."
              />
              <Choice
                selected={!local}
                onClick={() => update((n) => (n.transcription = 'server'))}
                icon={<Server className="size-5" />}
                title="logbook-server"
                text="A server holds the models for everyone: for computers that cannot keep up."
              />
            </div>
            {!local && (
              <>
                <Field label="Server address">
                  <div className="flex gap-2">
                    <input
                      value={s.serverUrl}
                      onChange={(e) => update((n) => (n.serverUrl = e.target.value))}
                      placeholder="http://127.0.0.1:3300"
                      className={`${fieldClass} flex-1`}
                    />
                    <CheckButton
                      check={serverCheck}
                      onClick={() =>
                        void run(setServerCheck, () =>
                          api.checkServer(s.serverUrl, serverToken || undefined),
                        )
                      }
                    />
                  </div>
                  <CheckResult check={serverCheck} ok="The server answers and takes the token." />
                </Field>
                <Field
                  label="Your token"
                  help={`Made for you on the server's admin page (${s.serverUrl.replace(/\/+$/, '') || 'http://…'}). Kept in the keychain.${view.secrets.serverToken ? ' One is set: type a new one to replace it.' : ''}`}
                >
                  <input
                    type="password"
                    value={serverToken ?? ''}
                    onChange={(e) => {
                      setServerToken(e.target.value);
                      setSaved(false);
                      setServerCheck({ state: 'idle' });
                    }}
                    placeholder={view.secrets.serverToken ? '••••••••' : 'lbk_…'}
                    className={`${fieldClass} w-full`}
                  />
                </Field>
              </>
            )}
          </Section>

          <Section title="Audio">
            <Field label="Your name" help="What the microphone hears is written under it.">
              <input
                value={s.me}
                onChange={(e) => update((n) => (n.me = e.target.value))}
                className={`${fieldClass} w-full`}
              />
            </Field>
            <Field label="Microphone">
              <DeviceSelect
                value={s.audio.micDevice}
                options={devices.inputs}
                onChange={(v) => update((n) => (n.audio.micDevice = v))}
              />
            </Field>
            <Toggle
              checked={s.audio.system}
              onChange={(v) => update((n) => (n.audio.system = v))}
              label="Record the call"
              help="What the computer plays (Meet, Zoom, Teams) as a second channel, whose speakers are told apart. macOS asks once for permission."
            />
            {s.audio.system && (
              <>
                <Field label="Output to record">
                  <DeviceSelect
                    value={s.audio.systemDevice}
                    options={devices.outputs}
                    onChange={(v) => update((n) => (n.audio.systemDevice = v))}
                  />
                </Field>
                <Toggle
                  checked={s.audio.headphones}
                  onChange={(v) => update((n) => (n.audio.headphones = v))}
                  label="I use headphones"
                  help="Without them the call's echo is cancelled from the microphone; with them nothing is."
                />
              </>
            )}
            <Toggle
              checked={s.audio.micGainAuto}
              onChange={(v) => update((n) => (n.audio.micGainAuto = v))}
              label="Raise a quiet microphone"
              help="For a quiet microphone or voices far from it."
            />
            <Toggle
              checked={s.audio.diarizeMic}
              onChange={(v) => update((n) => (n.audio.diarizeMic = v))}
              label="Several people around this computer"
              help="Tell apart the voices the microphone hears (a meeting room)."
            />
          </Section>

          <Section title="brainy">
            <Toggle
              checked={s.brainy.enabled}
              onChange={(v) => update((n) => (n.brainy.enabled = v))}
              label="Write each meeting as a page in brainy"
              help="The page is created when the meeting starts and the transcript is added a minute or so behind it; at the end the notes and the report go on top."
            />
            {s.brainy.enabled && (
              <>
                <Field label="brainy's address">
                  <input
                    value={s.brainy.url}
                    onChange={(e) => update((n) => (n.brainy.url = e.target.value))}
                    placeholder="https://brainy.example.com"
                    className={`${fieldClass} w-full`}
                  />
                </Field>
                <Field
                  label="Personal token"
                  help={`brainy: Settings → Connections, with write access. Kept in the keychain.${view.secrets.brainyToken ? ' One is set: type a new one to replace it.' : ''}`}
                >
                  <input
                    type="password"
                    value={token ?? ''}
                    onChange={(e) => {
                      setToken(e.target.value);
                      setSaved(false);
                    }}
                    placeholder={view.secrets.brainyToken ? '••••••••' : 'brainy_…'}
                    className={`${fieldClass} w-full`}
                  />
                </Field>
                <Field
                  label="Under the page"
                  help='Its address, or its path of titles ("Engineering/Meetings"); empty for the top of your workspace.'
                >
                  <input
                    value={s.brainy.parent}
                    onChange={(e) => update((n) => (n.brainy.parent = e.target.value))}
                    placeholder="Meetings"
                    className={`${fieldClass} w-full`}
                  />
                </Field>
                <div>
                  <CheckButton
                    check={brainyCheck}
                    onClick={() =>
                      void run(setBrainyCheck, async () => {
                        if (!(await save())) throw new Error('the settings could not be saved');
                        await api.checkBrainy();
                      })
                    }
                    label="Save and check"
                  />
                  <CheckResult check={brainyCheck} ok="brainy answers, and the page is there." />
                </div>
              </>
            )}
          </Section>

          <Section title="Language model">
            <p className={`text-sm ${mutedTextClass}`}>
              It reads the meeting's text: to clean the transcript up and to write the report
              (summary, your notes completed, decisions, action items).{' '}
              {local
                ? 'Without a server, the model below is used.'
                : "With logbook-server, the server's model is used (its --polish-url): nothing below."}
            </p>
            <Toggle
              checked={s.model.report}
              onChange={(v) => update((n) => (n.model.report = v))}
              label="Write the report"
            />
            <Toggle
              checked={s.model.polish}
              onChange={(v) => update((n) => (n.model.polish = v))}
              label="Clean the transcript up"
              help="Fillers, false starts, punctuation. Without it the transcript stays as heard."
            />
            {local && (
              <>
                <div className="flex flex-wrap gap-2">
                  {PRESETS.map((p) => (
                    <button
                      key={p.label}
                      type="button"
                      className={secondaryButtonSmClass}
                      onClick={() =>
                        update((n) => {
                          n.model.url = p.url;
                          n.model.name = p.name;
                        })
                      }
                    >
                      {p.label}
                    </button>
                  ))}
                </div>
                <Field label="API address" help="OpenAI-compatible, up to /v1.">
                  <input
                    value={s.model.url}
                    onChange={(e) => update((n) => (n.model.url = e.target.value))}
                    className={`${fieldClass} w-full`}
                  />
                </Field>
                <Field label="Model">
                  <input
                    value={s.model.name}
                    onChange={(e) => update((n) => (n.model.name = e.target.value))}
                    className={`${fieldClass} w-full`}
                  />
                </Field>
                <Field
                  label="API key"
                  help={`Kept in the keychain; none for a model on this computer.${view.secrets.modelKey ? ' One is set: type a new one to replace it.' : ''}`}
                >
                  <input
                    type="password"
                    value={key ?? ''}
                    onChange={(e) => {
                      setKey(e.target.value);
                      setSaved(false);
                    }}
                    placeholder={view.secrets.modelKey ? '••••••••' : 'gsk_…'}
                    className={`${fieldClass} w-full`}
                  />
                </Field>
                {s.model.url.includes('groq.com') && (
                  <p className="rounded-md bg-amber-50 px-3 py-2 text-sm dark:bg-amber-950/30">
                    Groq reads the text of every meeting it cleans up or reports on: fine for
                    testing, not for meetings that must stay in the company.
                  </p>
                )}
              </>
            )}
          </Section>

          <Section title="Voices known by name">
            <p className={`text-sm ${mutedTextClass}`}>
              Name a speaker in a meeting and their voice is kept on this computer only, to name
              them in the next meetings. Ask before you name someone: a voiceprint is personal data.
            </p>
            {voices.length === 0 ? (
              <p className="text-sm text-neutral-500">No voice yet.</p>
            ) : (
              <ul className="divide-y divide-neutral-200 rounded-md border border-neutral-200 dark:divide-neutral-800 dark:border-neutral-800">
                {voices.map((v) => (
                  <li key={v.name} className="flex items-center justify-between px-3 py-2 text-sm">
                    <span>
                      {v.name}{' '}
                      <span className="text-neutral-500">
                        · {Math.round(v.seconds)} s from {v.prints} meeting
                        {v.prints === 1 ? '' : 's'}
                      </span>
                    </span>
                    <button
                      type="button"
                      title={`Forget ${v.name}'s voice`}
                      className={quietDangerButtonClass}
                      onClick={async () => {
                        if (
                          !(await ask(`${v.name} will no longer be recognized.`, {
                            title: `Forget ${v.name}'s voice?`,
                            kind: 'warning',
                          }))
                        )
                          return;
                        await api.forgetVoice(v.name);
                        setVoices(await api.listVoices());
                      }}
                    >
                      <Trash2 className="size-4" />
                    </button>
                  </li>
                ))}
              </ul>
            )}
            <p className="text-xs text-neutral-500">
              Data folder, shared with the logbook CLI: {view.data}
            </p>
          </Section>
        </div>
      </div>
      <footer className="flex items-center justify-end gap-3 border-t border-neutral-200 px-10 py-3 dark:border-neutral-800">
        {error && <span className="text-sm text-red-600">{error}</span>}
        {saved && (
          <span className="flex items-center gap-1 text-sm text-neutral-500">
            <Check className="size-4" /> Saved
          </span>
        )}
        <button type="button" onClick={() => void save()} className={primaryButtonClass}>
          Save
        </button>
      </footer>
    </div>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="space-y-4">
      <h2 className="text-sm font-semibold tracking-wide text-neutral-500 uppercase">{title}</h2>
      {children}
    </section>
  );
}

function Field({ label, help, children }: { label: string; help?: string; children: ReactNode }) {
  return (
    <label className="block space-y-1.5">
      <span className="block text-sm font-medium">{label}</span>
      {children}
      {help && <span className={`block text-xs ${mutedTextClass}`}>{help}</span>}
    </label>
  );
}

function Toggle({
  checked,
  onChange,
  label,
  help,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  help?: string;
}) {
  return (
    <label className="flex cursor-pointer items-start gap-3">
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        onClick={() => onChange(!checked)}
        className={`relative mt-0.5 h-5 w-9 shrink-0 rounded-full transition-colors ${
          checked ? 'bg-accent-fill' : 'bg-neutral-300 dark:bg-neutral-600'
        }`}
      >
        <span
          className={`absolute top-0.5 left-0 size-4 rounded-full bg-white shadow transition-transform ${
            checked ? 'translate-x-4' : 'translate-x-0.5'
          }`}
        />
      </button>
      <span>
        <span className="block text-sm font-medium">{label}</span>
        {help && <span className={`block text-xs ${mutedTextClass}`}>{help}</span>}
      </span>
    </label>
  );
}

function Choice({
  selected,
  onClick,
  icon,
  title,
  text,
}: {
  selected: boolean;
  onClick: () => void;
  icon: ReactNode;
  title: string;
  text: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`rounded-lg border p-4 text-left transition-colors ${
        selected
          ? 'border-accent bg-accent/5 ring-1 ring-accent'
          : 'border-neutral-200 hover:border-neutral-400 dark:border-neutral-700'
      }`}
    >
      <span className={`mb-2 block ${selected ? 'text-accent' : 'text-neutral-500'}`}>{icon}</span>
      <span className="block text-sm font-medium">{title}</span>
      <span className={`mt-1 block text-xs ${mutedTextClass}`}>{text}</span>
    </button>
  );
}

function DeviceSelect({
  value,
  options,
  onChange,
}: {
  value: string | null;
  options: string[];
  onChange: (v: string | null) => void;
}) {
  return (
    <select
      value={value ?? ''}
      onChange={(e) => onChange(e.target.value || null)}
      className={`${fieldClass} w-full`}
    >
      <option value="">The system's default</option>
      {options.map((o) => (
        <option key={o} value={o}>
          {o}
        </option>
      ))}
      {value && !options.includes(value) && <option value={value}>{value} (not connected)</option>}
    </select>
  );
}

function CheckButton({
  check,
  onClick,
  label = 'Check',
}: {
  check: Check;
  onClick: () => void;
  label?: string;
}) {
  return (
    <button
      type="button"
      disabled={check.state === 'checking'}
      onClick={onClick}
      className={`${secondaryButtonSmClass} !h-8`}
    >
      {check.state === 'checking' ? 'Checking…' : label}
    </button>
  );
}

function CheckResult({ check, ok }: { check: Check; ok: string }) {
  if (check.state === 'ok') return <span className="block text-xs text-accent">{ok}</span>;
  if (check.state === 'failed')
    return <span className="block text-xs text-red-600">{check.message}</span>;
  return null;
}
