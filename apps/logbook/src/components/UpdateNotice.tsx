// A newer version of the app, found on GitHub (src-tauri updater.rs): the
// person installs it when they choose, never while a meeting is recorded,
// since installing restarts the app. Closed, it comes back at the next
// look (every few hours) or the next start.
import { useEffect, useState } from 'react';
import { X } from 'lucide-react';
import { api, events, subscribe, type UpdateInfo, type UpdateProgress } from '../lib/api.js';

type Install =
  | { state: 'idle' }
  | { state: 'installing'; progress: UpdateProgress | null }
  | { state: 'failed'; message: string };

export function UpdateNotice({
  update,
  recording,
  onClose,
}: {
  update: UpdateInfo;
  recording: boolean;
  onClose: () => void;
}) {
  const [install, setInstall] = useState<Install>({ state: 'idle' });

  useEffect(
    () =>
      subscribe(
        events.updateProgress((progress) =>
          setInstall((i) => (i.state === 'installing' ? { state: 'installing', progress } : i)),
        ),
      ),
    [],
  );

  const run = async () => {
    setInstall({ state: 'installing', progress: null });
    try {
      // On success the app restarts with the new version.
      await api.installUpdate();
    } catch (e) {
      setInstall({ state: 'failed', message: String(e) });
    }
  };

  const progress = install.state === 'installing' ? install.progress : null;
  const percent = progress?.total
    ? Math.min(100, Math.round((progress.done / progress.total) * 100))
    : null;

  return (
    <div
      role="status"
      className="fixed right-3 bottom-3 z-50 flex w-72 items-start gap-2 rounded-lg border border-neutral-200 bg-white p-3 text-sm shadow-lg max-sm:inset-x-3 max-sm:w-auto dark:border-neutral-700 dark:bg-neutral-900"
    >
      <div className="min-w-0 flex-1">
        <p>Logbook {update.version} is available.</p>
        <p className="text-xs text-neutral-500">
          {install.state === 'installing'
            ? percent === null
              ? 'Downloading…'
              : `Downloading… ${percent}%`
            : recording
              ? 'It can be installed once the meeting is finished.'
              : `You have ${update.current}. Installing restarts Logbook.`}
        </p>
        {install.state === 'failed' && (
          <p className="mt-1 text-xs text-red-600">{install.message}</p>
        )}
      </div>
      {install.state !== 'installing' && (
        <>
          <button
            type="button"
            disabled={recording}
            onClick={() => void run()}
            className="rounded-md px-2 py-1 font-medium text-accent hover:bg-neutral-100 disabled:opacity-50 dark:hover:bg-neutral-800"
          >
            Install
          </button>
          <button
            type="button"
            aria-label="Later"
            onClick={onClose}
            className="rounded-md px-1.5 py-1 text-neutral-500 hover:bg-neutral-100 dark:hover:bg-neutral-800"
          >
            <X className="size-4" />
          </button>
        </>
      )}
    </div>
  );
}
