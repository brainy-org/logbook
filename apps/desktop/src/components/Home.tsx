// Before a meeting: its title, the start button, and what will happen to
// it (where it is transcribed, whether it goes to brainy, who writes the
// report), as the settings say. Meetings left pending are offered here.
import { useEffect, useState, type FormEvent } from 'react';
import { AudioLines, Cloud, FileText, Laptop, Mic, NotebookPen, Server } from 'lucide-react';
import {
  api,
  type LogLine,
  type ModelsStatus,
  type PendingMeeting,
  type RecorderState,
  type SettingsView,
} from '../lib/api.js';
import { mutedTextClass, primaryButtonClass, secondaryButtonSmClass } from './form-styles.js';
import { ModelsProgress } from './ModelsProgress.js';

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
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [finishing, setFinishing] = useState(false);

  useEffect(() => {
    api.getSettings().then(setSettings);
  }, []);

  const start = async (e: FormEvent) => {
    e.preventDefault();
    setStarting(true);
    setError(null);
    try {
      onStarted(await api.startMeeting(title));
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

  return (
    <div className="flex h-full flex-col">
      <div data-tauri-drag-region className="h-11 shrink-0" />
      <div className="mx-auto flex w-full max-w-xl flex-1 flex-col justify-center px-8 pb-16">
        <div className="mb-8 flex size-12 items-center justify-center rounded-2xl bg-accent-fill text-white">
          <AudioLines className="size-6" />
        </div>
        <form onSubmit={start} className="space-y-4">
          <input
            autoFocus
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="Meeting title (optional)"
            className="w-full border-none bg-transparent text-3xl font-semibold tracking-tight outline-none placeholder:text-neutral-300 dark:placeholder:text-neutral-600"
          />
          <button
            type="submit"
            disabled={starting || !s}
            className={`${primaryButtonClass} !h-10 px-5`}
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
              {s.audio.system ? ', your microphone and the call' : ', your microphone'}
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
