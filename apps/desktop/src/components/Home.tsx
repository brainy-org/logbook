// Before a meeting: its title, what it is (a call, people in the room, or
// both: its channels, kept for the next), the start button, and what will happen to
// it (where it is transcribed, whether it goes to brainy, who writes the
// report), as the settings say. Meetings left pending are offered here.
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
} from '../lib/api.js';
import { mutedTextClass, primaryButtonClass, secondaryButtonSmClass } from './form-styles.js';
import { ModelsProgress } from './ModelsProgress.js';

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
  const [finishing, setFinishing] = useState(false);

  useEffect(() => {
    api.getSettings().then((view) => {
      setSettings(view);
      setMode(view.settings.audio.mode);
    });
  }, []);

  const start = async (e: FormEvent) => {
    e.preventDefault();
    setStarting(true);
    setError(null);
    try {
      onStarted(await api.startMeeting(title, mode));
    } catch (err) {
      setError(String(err));
    } finally {
      setStarting(false);
    }
  };

  const s = settings?.settings;
  const local = s?.transcription === 'local';
  const model = s && (s.model.polish || s.model.report);
  const lastWarning = [...logs].reverse().find((l) => l.warning);
  const current = MODES.find((m) => m.mode === mode)!;

  return (
    <div className="flex h-full flex-col">
      <div data-tauri-drag-region className="h-11 shrink-0" />
      <div className="mx-auto flex w-full max-w-xl flex-1 flex-col justify-center px-8 pb-16">
        <div className="mb-8 flex size-12 items-center justify-center rounded-2xl bg-accent-fill text-white">
          <AudioLines className="size-6" />
        </div>
        <form onSubmit={start}>
          <input
            autoFocus
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="Meeting title (optional)"
            className="mb-6 w-full border-none bg-transparent text-3xl font-semibold tracking-tight outline-none placeholder:text-neutral-300 dark:placeholder:text-neutral-500"
          />
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
                : `Transcribed by logbook-server at ${s.serverUrl}`}
            </li>
            <li className="flex items-center gap-2">
              <Cloud className="size-4" />
              {s.brainy.enabled
                ? `Written to brainy${s.brainy.parent ? `, under ${s.brainy.parent}` : ''}`
                : 'Kept on this computer only (brainy is off)'}
            </li>
            <li className="flex items-center gap-2">
              <FileText className="size-4" />
              {!model
                ? 'No language model: the transcript stays as heard, no report'
                : `${s.model.report ? 'Report' : ''}${s.model.report && s.model.polish ? ' and ' : ''}${s.model.polish ? 'clean-up' : ''} by ${local ? s.model.name || 'the model in settings' : "the server's model"}`}
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

        {pending.length > 0 && (
          <div className="mt-8 flex items-center justify-between gap-3 rounded-lg border border-amber-300 bg-amber-50 px-4 py-3 text-sm dark:border-amber-700 dark:bg-amber-950/30">
            <span>
              {pending.length === 1
                ? 'A meeting is waiting to be finished'
                : `${pending.length} meetings are waiting to be finished`}{' '}
              (the server or brainy could not be reached).
            </span>
            <button
              type="button"
              disabled={finishing}
              className={`${secondaryButtonSmClass} shrink-0 whitespace-nowrap`}
              onClick={() => {
                setFinishing(true);
                onFinishPending();
              }}
            >
              {finishing ? 'Finishing…' : 'Finish now'}
            </button>
          </div>
        )}
        {lastWarning && !failure && models?.state !== 'failed' && (
          <p className={`mt-6 text-xs ${mutedTextClass}`}>Last message: {lastWarning.message}</p>
        )}
      </div>
    </div>
  );
}
