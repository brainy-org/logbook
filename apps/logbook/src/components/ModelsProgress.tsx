// The transcription getting ready on this computer: downloaded once,
// then loaded. Nothing once it is ready, or with a server.
import { Download } from 'lucide-react';
import { api, size, type ModelsStatus } from '../lib/api.js';
import { mutedTextClass, secondaryButtonSmClass } from './form-styles.js';

export function ModelsProgress({ status }: { status: ModelsStatus | null }) {
  if (!status || status.state === 'ready') return null;

  if (status.state === 'failed') {
    return (
      <div className="flex items-center justify-between gap-3 rounded-lg bg-red-50 px-4 py-3 text-sm text-red-600 dark:bg-red-950/40">
        <span>The download did not finish. Check the connection and try again.</span>
        <button
          type="button"
          className={`${secondaryButtonSmClass} shrink-0 whitespace-nowrap`}
          onClick={() => void api.loadModels()}
        >
          Try again
        </button>
      </div>
    );
  }

  const share =
    status.state === 'fetching' && status.total > 0
      ? Math.min(1, status.done / status.total)
      : null;
  return (
    <div className="rounded-lg border border-neutral-200 px-4 py-3 text-sm dark:border-neutral-700">
      <div className="flex items-center gap-2">
        <Download className="size-4 shrink-0 text-accent" />
        <span className="font-medium">
          {share !== null ? 'Getting the transcription ready' : 'Almost ready…'}
        </span>
        {share !== null && (
          <span className="ml-auto font-mono text-xs tabular-nums">{Math.floor(share * 100)}%</span>
        )}
      </div>
      <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700">
        <div
          className={`h-full rounded-full bg-accent ${share === null ? 'w-full animate-pulse' : 'transition-[width] duration-300'}`}
          style={share === null ? undefined : { width: `${share * 100}%` }}
        />
      </div>
      {status.state === 'fetching' && (
        // The words stay put and the numbers keep their width: only the
        // digits change as the bytes arrive.
        <p className={`mt-1.5 flex justify-between gap-3 text-xs ${mutedTextClass}`}>
          <span>Only the first time</span>
          <span className="tabular-nums">
            {size(status.done)} of {size(status.total)}
          </span>
        </p>
      )}
    </div>
  );
}
