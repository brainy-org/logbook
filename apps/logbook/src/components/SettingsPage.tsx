// The settings: where meetings are transcribed, what is recorded, whether
// they go to brainy, and the
// language model that writes the report; then how many voices are known by
// name, and the way to their page (VoicesPage.tsx). Secrets (the
// server's and brainy's tokens, the model's key) are kept on this
// computer (secrets.json, this user only) and never read back: the page
// only knows whether they are set. On Android: a server always, the
// microphone only, and whether the recording may go on in the background.
import { useEffect, useState, type ReactNode } from 'react';
import { Check, ChevronRight, Laptop, Server } from 'lucide-react';
import { android, macos } from '../lib/platform.js';
import { api, type Devices, type Settings, type SettingsView, type Voice } from '../lib/api.js';
import {
  fieldClass,
  mutedTextClass,
  primaryButtonClass,
  secondaryButtonSmClass,
} from './form-styles.js';

/**
 * Hosted providers with an OpenAI-compatible API, and any other at an
 * address given. The one chosen is read back from the address kept.
 */
const OTHER = { id: 'other', label: 'Other (OpenAI-compatible)', url: '', key: '' };
const PROVIDERS = [
  { id: 'groq', label: 'Groq', url: 'https://api.groq.com/openai/v1', key: 'gsk_…' },
  { id: 'openai', label: 'OpenAI', url: 'https://api.openai.com/v1', key: 'sk-…' },
  { id: 'mistral', label: 'Mistral', url: 'https://api.mistral.ai/v1', key: '' },
  OTHER,
];

const providerOf = (url: string) =>
  PROVIDERS.find((p) => p.url && p.url === url.trim().replace(/\/+$/, '')) ?? OTHER;

type Check =
  | { state: 'idle' }
  | { state: 'checking' }
  | { state: 'ok' }
  | { state: 'failed'; message: string };

/** The models the API offers, once asked for. */
type Listed =
  | { state: 'idle' }
  | { state: 'listing' }
  | { state: 'listed'; names: string[] }
  | { state: 'failed'; message: string };

export function SettingsPage({
  recording,
  onOpenVoices,
}: {
  recording: boolean;
  onOpenVoices: () => void;
}) {
  const [view, setView] = useState<SettingsView | null>(null);
  const [s, setS] = useState<Settings | null>(null);
  const [devices, setDevices] = useState<Devices>({ inputs: [], outputs: [] });
  const [voices, setVoices] = useState<Voice[] | null>(null);
  const [token, setToken] = useState<string | undefined>(undefined);
  const [key, setKey] = useState<string | undefined>(undefined);
  const [serverToken, setServerToken] = useState<string | undefined>(undefined);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [serverCheck, setServerCheck] = useState<Check>({ state: 'idle' });
  const [brainyCheck, setBrainyCheck] = useState<Check>({ state: 'idle' });
  const [listed, setListed] = useState<Listed>({ state: 'idle' });
  const [unrestricted, setUnrestricted] = useState(true);

  const listModels = async (url: string, typedKey?: string) => {
    if (!url.trim()) return;
    setListed({ state: 'listing' });
    try {
      setListed({ state: 'listed', names: await api.listModels(url, typedKey || undefined) });
    } catch (e) {
      setListed({ state: 'failed', message: String(e) });
    }
  };

  useEffect(() => {
    api.getSettings().then((v) => {
      setView(v);
      setS(v.settings);
      if (v.settings.transcription === 'local') void listModels(v.settings.model.url);
    });
    if (!android) api.listDevices().then(setDevices, () => undefined);
    api.listVoices().then(setVoices, () => undefined);
  }, []);

  useEffect(() => {
    if (!android) return;
    // Read again when the app comes back from Android's dialog.
    const check = () => void api.backgroundState().then(setUnrestricted, () => undefined);
    check();
    document.addEventListener('visibilitychange', check);
    return () => document.removeEventListener('visibilitychange', check);
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

  const local = !android && s.transcription === 'local';
  const here = android ? 'this phone' : 'this computer';
  const provider = providerOf(s.model.url);

  return (
    <div className="flex h-full flex-col">
      <div data-tauri-drag-region className="h-11 shrink-0 max-sm:hidden" />
      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto max-w-2xl space-y-10 px-10 pb-24 max-sm:px-4 max-sm:pt-4">
          <h1 className="text-2xl font-semibold tracking-tight">Settings</h1>
          {recording && (
            <p className="rounded-md bg-amber-50 px-3 py-2 text-sm dark:bg-amber-950/30">
              A meeting is being recorded: changes apply to the next one.
            </p>
          )}

          <Section title="Transcription">
            {android ? (
              <p className={`text-sm ${mutedTextClass}`}>
                This phone records and streams the audio to a server, which transcribes it.
              </p>
            ) : (
              <div className="grid grid-cols-2 gap-3">
                <Choice
                  selected={local}
                  onClick={() => update((n) => (n.transcription = 'local'))}
                  icon={<Laptop className="size-5" />}
                  title="On this computer"
                  text="The audio stays on this computer. The models (about 1 GB) download once; a laptop without a GPU may fall behind."
                />
                <Choice
                  selected={!local}
                  onClick={() => update((n) => (n.transcription = 'server'))}
                  icon={<Server className="size-5" />}
                  title="On a server"
                  text="The audio is streamed to a server, which transcribes it and keeps it only until the meeting ends. Needs its address and your token."
                />
              </div>
            )}
            {!local && (
              <>
                <Field label="Server address">
                  <div className="flex gap-2 max-sm:flex-col">
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
                  help={`Made for you on the server's admin page (${s.serverUrl.replace(/\/+$/, '') || 'http://…'}). Kept on ${here}, readable by this app only.${view.secrets.serverToken ? ' One is set: type a new one to replace it.' : ''}`}
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
            {!android && (
              <>
                <Field label="Microphone">
                  <DeviceSelect
                    value={s.audio.micDevice}
                    options={devices.inputs}
                    onChange={(v) => update((n) => (n.audio.micDevice = v))}
                  />
                </Field>
                <Field
                  label="Output to record"
                  help="The call, in a meeting on a call (chosen on the home page). macOS asks once for permission."
                >
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
                  help="On a call, without them the call's echo is cancelled from the microphone; with them nothing is."
                />
              </>
            )}
            <Toggle
              checked={s.audio.micGainAuto}
              onChange={(v) => update((n) => (n.audio.micGainAuto = v))}
              label={android ? 'Raise faint voices' : 'Raise a quiet microphone'}
              help={
                android
                  ? 'As far as it goes, for voices across the room; background noise rises too.'
                  : 'For a quiet microphone or voices far from it.'
              }
            />
          </Section>

          {macos && (
            <Section title="Calls">
              <Toggle
                checked={s.calls.notice}
                onChange={(v) => update((n) => (n.calls.notice = v))}
                label="Notice when a call starts"
                help="When Teams, Zoom, Webex, Slack, Discord or WhatsApp opens the microphone, a notification offers to record the call, until it ends. Calls in a browser are not noticed. Needs macOS 14.4."
              />
              {s.calls.notice && (
                <Toggle
                  checked={s.calls.record}
                  onChange={(v) => update((n) => (n.calls.record = v))}
                  label="Record it at once"
                  help="Without asking: the notification says it is being recorded. Stop it as any meeting."
                />
              )}
            </Section>
          )}

          {android && (
            <Section title="Background recording">
              <p className={`text-sm ${mutedTextClass}`}>
                While recording, a notification stays on and the phone keeps the app awake with the
                screen off. Android's battery optimization can still stop it: exempt the app.
              </p>
              {unrestricted ? (
                <p className="flex items-center gap-1 text-sm text-accent">
                  <Check className="size-4" /> Battery optimization leaves the app alone.
                </p>
              ) : (
                <button
                  type="button"
                  onClick={() => void api.askBackground()}
                  className={secondaryButtonSmClass}
                >
                  Allow in background
                </button>
              )}
            </Section>
          )}

          <Section title="brainy">
            <Toggle
              checked={s.brainy.enabled}
              onChange={(v) => update((n) => (n.brainy.enabled = v))}
              label="Write each meeting as a page in brainy"
              help="The page is created when the meeting ends, with the report (or the notes without one). The transcript stays on this computer."
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
                  help={`brainy: Settings → Connections, with write access. Kept on ${here}, readable by ${android ? 'this app' : 'you'} only.${view.secrets.brainyToken ? ' One is set: type a new one to replace it.' : ''}`}
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
              It reads the meeting's text to write the report (summary, your notes completed,
              decisions, action items).{' '}
              {local
                ? 'Without a server, the model below is used.'
                : "On a server, the server's model is used (its --llm-url): nothing below."}
            </p>
            <Toggle
              checked={s.model.report}
              onChange={(v) => update((n) => (n.model.report = v))}
              label="Write the report"
            />
            {local && (
              <>
                <Field label="Provider">
                  <select
                    value={provider.id}
                    onChange={(e) => {
                      const p = PROVIDERS.find((p) => p.id === e.target.value) ?? provider;
                      update((n) => {
                        n.model.url = p.url;
                        n.model.name = '';
                      });
                      setListed({ state: 'idle' });
                      if (p.url) void listModels(p.url, key);
                    }}
                    className={`${fieldClass} w-full`}
                  >
                    {PROVIDERS.map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.label}
                      </option>
                    ))}
                  </select>
                </Field>
                {provider.id === 'other' && (
                  <Field label="API address" help="OpenAI-compatible, up to /v1.">
                    <input
                      value={s.model.url}
                      onChange={(e) => {
                        update((n) => (n.model.url = e.target.value));
                        setListed({ state: 'idle' });
                      }}
                      placeholder="https://models.example.com/v1"
                      className={`${fieldClass} w-full`}
                    />
                  </Field>
                )}
                <Field
                  label="Model"
                  help={
                    listed.state === 'listed'
                      ? undefined
                      : "Its name, or the list of those the provider offers (with the provider's key)."
                  }
                >
                  <div className="flex gap-2">
                    {listed.state === 'listed' ? (
                      <ModelSelect
                        value={s.model.name}
                        names={listed.names}
                        onChange={(v) => update((n) => (n.model.name = v))}
                      />
                    ) : (
                      <input
                        value={s.model.name}
                        onChange={(e) => update((n) => (n.model.name = e.target.value))}
                        className={`${fieldClass} flex-1`}
                      />
                    )}
                    <button
                      type="button"
                      disabled={listed.state === 'listing' || !s.model.url.trim()}
                      onClick={() => void listModels(s.model.url, key)}
                      className={`${secondaryButtonSmClass} !h-8`}
                    >
                      {listed.state === 'listing'
                        ? 'Listing…'
                        : listed.state === 'listed'
                          ? 'List again'
                          : 'List models'}
                    </button>
                  </div>
                  {listed.state === 'failed' && (
                    <span className="block text-xs text-red-600">{listed.message}</span>
                  )}
                </Field>
                <Field
                  label="API key"
                  help={`The provider's, kept on this computer, readable by you only.${view.secrets.modelKey ? ' One is set: type a new one to replace it.' : ''}`}
                >
                  <input
                    type="password"
                    value={key ?? ''}
                    onChange={(e) => {
                      setKey(e.target.value);
                      setSaved(false);
                    }}
                    placeholder={view.secrets.modelKey ? '••••••••' : provider.key}
                    className={`${fieldClass} w-full`}
                  />
                </Field>
                {provider.id !== 'other' && (
                  <p className="rounded-md bg-amber-50 px-3 py-2 text-sm dark:bg-amber-950/30">
                    {provider.label} reads the text of every meeting it reports on: fine for
                    testing, not for meetings that must stay in the company.
                  </p>
                )}
              </>
            )}
          </Section>

          <Section title="Voices known by name">
            <div className="flex items-center justify-between gap-3">
              <p className={`text-sm ${mutedTextClass}`}>
                {voices === null
                  ? 'Kept on ' + here + ' only.'
                  : `${voices.length === 0 ? 'None yet' : `${voices.length} ${voices.length === 1 ? 'person' : 'people'}`}, kept on ${here} only.`}
              </p>
              <button
                type="button"
                onClick={onOpenVoices}
                className={`${secondaryButtonSmClass} flex shrink-0 items-center gap-1`}
              >
                Manage voices <ChevronRight className="size-3.5" />
              </button>
            </div>
            <p className="text-xs text-neutral-500">
              {android ? 'Data folder' : 'Data folder, shared with the logbook CLI'}: {view.data}
            </p>
          </Section>
        </div>
      </div>
      <footer className="flex items-center justify-end gap-3 border-t border-neutral-200 px-10 py-3 max-sm:px-4 dark:border-neutral-800">
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
      className={`flex flex-col items-start rounded-lg border p-4 text-left transition-colors ${
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

function ModelSelect({
  value,
  names,
  onChange,
}: {
  value: string;
  names: string[];
  onChange: (v: string) => void;
}) {
  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className={`${fieldClass} flex-1`}
    >
      {!value && <option value="">Choose a model</option>}
      {value && !names.includes(value) && <option value={value}>{value} (not offered)</option>}
      {names.map((n) => (
        <option key={n} value={n}>
          {n}
        </option>
      ))}
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
