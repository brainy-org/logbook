// Before a meeting: its title, what it is (a call, people in the room, or
// both: its channels, kept for the next), who is expected among the voices
// known by name (its speakers are named from them only), the start button, and what will happen to
// it (where it is transcribed, whether it goes to brainy, who writes the
// report), as the settings say. Meetings left pending are shown here as
// they are finished (PendingMeetings.tsx).
// On Android the phone's microphone is the only channel: the people around
// it, told apart; no mode to choose.
import { useEffect, useState, type FormEvent } from 'react';
import {
  AudioLines,
  Cloud,
  FileText,
  Laptop,
  Mic,
  Monitor,
  MonitorSpeaker,
  NotebookPen,
  Server,
  Users,
  X,
  type LucideIcon,
} from 'lucide-react';
import {
  api,
  type LogLine,
  type Mode,
  type ModelsStatus,
  type PendingMeeting,
  type RecorderState,
  type SettingsView,
  type Voice,
} from '../lib/api.js';
import { android } from '../lib/platform.js';
import {
  compactFieldClass,
  mutedTextClass,
  primaryButtonClass,
  secondaryButtonSmClass,
} from './form-styles.js';
import { ModelsProgress } from './ModelsProgress.js';
import { PendingMeetings } from './PendingMeetings.js';

const MODES: { mode: Mode; label: string; icon: LucideIcon; hears: string }[] = [
  { mode: 'call', label: 'Call', icon: Monitor, hears: 'Your microphone and the call' },
  {
    mode: 'room',
    label: 'In the room',
    icon: Users,
    hears: 'The voices around this computer, told apart',
  },
  {
    mode: 'roomCall',
    label: 'Room + call',
    icon: MonitorSpeaker,
    hears: 'The voices around this computer, told apart, and the call',
  },
];

interface Props {
  pending: PendingMeeting[];
  failure: string | null;
  logs: LogLine[];
  models: ModelsStatus | null;
  onStarted: (state: RecorderState) => void;
  onOpenSettings: () => void;
  onFinishPending: () => void;
}

export function Home({
  pending,
  failure,
  logs,
  models,
  onStarted,
  onOpenSettings,
  onFinishPending,
}: Props) {
  const [title, setTitle] = useState('');
  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [mode, setMode] = useState<Mode>('call');
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [voices, setVoices] = useState<Voice[]>([]);
  const [expected, setExpected] = useState<string[]>([]);
  // Android: whether battery optimization leaves the recording alone.
  const [unrestricted, setUnrestricted] = useState(true);

  useEffect(() => {
    api.listVoices().then(setVoices, () => undefined);
    api.getSettings().then((view) => {
      setSettings(view);
      setMode(android ? 'room' : view.settings.audio.mode);
    });
    if (!android) return;
    // Read again when the app comes back from Android's dialog.
    const check = () => void api.backgroundState().then(setUnrestricted, () => undefined);
    check();
    document.addEventListener('visibilitychange', check);
    return () => document.removeEventListener('visibilitychange', check);
  }, []);

  const start = async (e: FormEvent) => {
    e.preventDefault();
    setStarting(true);
    setError(null);
    try {
      onStarted(await api.startMeeting(title, mode, expected));
    } catch (err) {
      setError(String(err));
    } finally {
      setStarting(false);
    }
  };

  const s = settings?.settings;
  const local = !android && s?.transcription === 'local';
  const here = android ? 'this phone' : 'this computer';
  const report = s?.model.report;
  const lastWarning = [...logs].reverse().find((l) => l.warning);
  const current = MODES.find((m) => m.mode === mode)!;

  return (
    <div className="flex h-full flex-col">
      <div data-tauri-drag-region className="h-11 shrink-0 max-sm:hidden" />
      <div className="mx-auto flex w-full max-w-xl flex-1 flex-col justify-center-safe overflow-y-auto px-8 pb-16 max-sm:justify-start max-sm:px-4 max-sm:pt-6 max-sm:pb-8">
        <div className="mb-8 flex size-12 items-center justify-center rounded-2xl bg-accent-fill text-white">
          <AudioLines className="size-6" />
        </div>
        <form onSubmit={start}>
          <input
            // A phone would open its keyboard over the page.
            autoFocus={!android}
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="Meeting title (optional)"
            className="mb-6 w-full border-none bg-transparent text-3xl font-semibold tracking-tight outline-none placeholder:text-neutral-300 dark:placeholder:text-neutral-500"
          />
          {android ? (
            <p className={`text-sm ${mutedTextClass}`}>
              The voices around this phone, told apart. It keeps recording with the screen off.
            </p>
          ) : (
            <>
              <div
                role="radiogroup"
                aria-label="Meeting"
                className="inline-flex h-8 items-center gap-0.5 rounded-md border border-neutral-200 bg-neutral-100 p-0.5 dark:border-neutral-700 dark:bg-neutral-800/60"
              >
                {MODES.map(({ mode: m, label, icon: Icon }) => (
                  <button
                    key={m}
                    type="button"
                    role="radio"
                    aria-checked={mode === m}
                    onClick={() => setMode(m)}
                    className={`flex h-full items-center gap-1.5 rounded px-2.5 text-xs font-medium transition-colors focus-visible:outline-2 focus-visible:outline-accent ${
                      mode === m
                        ? 'bg-white text-neutral-900 shadow-sm ring-1 ring-neutral-200 dark:bg-neutral-600 dark:text-white dark:ring-white/10'
                        : `${mutedTextClass} hover:text-neutral-900 dark:hover:text-neutral-100`
                    }`}
                  >
                    <Icon className="size-3.5" />
                    {label}
                  </button>
                ))}
              </div>
              <p className={`mt-2 text-xs ${mutedTextClass}`}>{current.hears}</p>
            </>
          )}
          {voices.length > 0 && (
            <Expected
              voices={voices.map((v) => v.name)}
              expected={expected}
              onChange={setExpected}
            />
          )}
          <button
            type="submit"
            disabled={starting || !s}
            className={`${primaryButtonClass} mt-6 !h-9 px-4`}
          >
            <span className="flex items-center gap-2">
              <Mic className="size-4" />
              {starting ? 'Starting…' : 'Start recording'}
            </span>
          </button>
        </form>
        {local && (
          <div className="mt-6">
            <ModelsProgress status={models} />
          </div>
        )}
        {android && !unrestricted && (
          <div className="mt-4 flex items-center justify-between gap-3 rounded-md bg-amber-50 px-3 py-2 text-sm max-sm:flex-col max-sm:items-start dark:bg-amber-950/30">
            <span>
              Android's battery optimization may stop a long recording with the screen off.
            </span>
            <button
              type="button"
              className={`${secondaryButtonSmClass} shrink-0 whitespace-nowrap`}
              onClick={() => void api.askBackground()}
            >
              Allow in background
            </button>
          </div>
        )}
        {(error || failure) && (
          <p className="mt-4 rounded-md bg-red-50 px-3 py-2 text-sm text-red-600 dark:bg-red-950/40">
            {error || failure}
          </p>
        )}

        {s && (
          <ul className={`mt-10 space-y-2 text-sm ${mutedTextClass}`}>
            <li className="flex items-center gap-2">
              {local ? <Laptop className="size-4" /> : <Server className="size-4" />}
              {local
                ? 'Transcribed on this computer'
                : `Transcribed on the server at ${s.serverUrl}`}
            </li>
            <li className="flex items-center gap-2">
              <Cloud className="size-4" />
              {s.brainy.enabled
                ? `Written to brainy${s.brainy.parent ? `, under ${s.brainy.parent}` : ''}`
                : `Kept on ${here} only (brainy is off)`}
            </li>
            <li className="flex items-center gap-2">
              <FileText className="size-4" />
              {!report
                ? 'No report'
                : `Report by ${local ? s.model.name || 'the model in settings' : "the server's model"}`}
            </li>
            <li className="flex items-center gap-2">
              <NotebookPen className="size-4" />
              Write notes as you go: the report builds on them
            </li>
            <li>
              <button
                type="button"
                onClick={onOpenSettings}
                className="text-accent hover:underline"
              >
                Change in settings
              </button>
            </li>
          </ul>
        )}

        <PendingMeetings
          pending={pending}
          device={here}
          server={s?.serverUrl ?? 'the server'}
          local={local}
          onFinish={onFinishPending}
          onOpenSettings={onOpenSettings}
        />
        {lastWarning && !failure && models?.state !== 'failed' && (
          <p className={`mt-6 text-xs ${mutedTextClass}`}>Last message: {lastWarning.message}</p>
        )}
      </div>
    </div>
  );
}

/** The names offered at once while choosing who is expected. */
const SUGGESTED = 6;

/**
 * Who is expected, among the voices known by name: none chosen, the
 * speakers are named from every voice; some chosen, from them only, which
 * with many voices known gives fewer wrong names. A field that offers the
 * names it matches: only the people chosen take room, however many voices
 * are known.
 */
function Expected({
  voices,
  expected,
  onChange,
}: {
  voices: string[];
  expected: string[];
  onChange: (names: string[]) => void;
}) {
  const [query, setQuery] = useState('');
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const wanted = query.trim().toLowerCase();
  const matches = voices
    .filter((v) => !expected.includes(v) && v.toLowerCase().includes(wanted))
    .slice(0, SUGGESTED);
  const add = (name: string) => {
    onChange([...expected, name]);
    setQuery('');
    setActive(0);
  };
  const remove = (name: string) => onChange(expected.filter((n) => n !== name));

  return (
    <div className="mt-6">
      <p className="text-sm font-medium">Who is expected?</p>
      <p className={`mt-0.5 text-xs ${mutedTextClass}`}>
        {expected.length === 0
          ? 'Optional: speakers are named from every voice known.'
          : 'Speakers are named from these voices only.'}
      </p>
      <div className="relative mt-2 flex flex-wrap items-center gap-1.5">
        {expected.map((name) => (
          <span
            key={name}
            className="flex items-center gap-1 rounded-full border border-accent bg-accent/10 py-0.5 pr-1 pl-2.5 text-xs text-accent"
          >
            {name}
            <button
              type="button"
              title={`Not ${name}`}
              onClick={() => remove(name)}
              className="rounded-full p-0.5 hover:bg-accent/20"
            >
              <X className="size-3" />
            </button>
          </span>
        ))}
        <div className="relative">
          <input
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setActive(0);
              setOpen(true);
            }}
            onFocus={() => setOpen(true)}
            onBlur={() => setOpen(false)}
            onKeyDown={(e) => {
              if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                e.preventDefault();
                const step = e.key === 'ArrowDown' ? 1 : -1;
                setActive((i) => (i + step + matches.length) % Math.max(matches.length, 1));
              } else if (e.key === 'Enter' && query.trim()) {
                // A name, not the form: Enter on a name typed does not start the meeting.
                e.preventDefault();
                const name = matches[active];
                if (name !== undefined) add(name);
              } else if (e.key === 'Backspace' && !query && expected.length > 0) {
                remove(expected.at(-1)!);
              } else if (e.key === 'Escape') {
                setOpen(false);
              }
            }}
            placeholder={expected.length === 0 ? 'Add a name' : 'Add another'}
            className={`${compactFieldClass} w-44`}
          />
          {open && matches.length > 0 && (
            <ul className="absolute top-full left-0 z-10 mt-1 w-60 overflow-hidden rounded-md border border-neutral-200 bg-white py-1 text-sm shadow-lg dark:border-neutral-700 dark:bg-neutral-900">
              {matches.map((name, i) => (
                <li key={name}>
                  <button
                    type="button"
                    // Before the field's blur closes the list.
                    onMouseDown={(e) => {
                      e.preventDefault();
                      add(name);
                    }}
                    onMouseEnter={() => setActive(i)}
                    className={`w-full px-3 py-1.5 text-left ${
                      i === active ? 'bg-neutral-100 dark:bg-neutral-800' : ''
                    }`}
                  >
                    {name}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}
