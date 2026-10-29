// The meeting being recorded, as Granola has it: a page for your notes,
// the live transcript beside it, and a pill at the bottom with the levels,
// the time and the stop button. The notes go to the app as they are typed
// (`set_notes`), and to the report when the meeting ends. At phone width
// the live transcript is a panel over the notes, closed at first. On the
// desktop a screenshot (the buttons, or the shortcuts from any app: the
// screen under the pointer, or on macOS an area to select) goes in
// the notes as a line of its own where the cursor is, shown under them.
import { useEffect, useRef, useState } from 'react';
import { Camera, PanelRightClose, PanelRightOpen, Scan, Square } from 'lucide-react';
import {
  api,
  clock,
  events,
  liveLabel,
  subscribe,
  type LiveUtterance,
  type LogLine,
  type ModelsStatus,
  type RecorderState,
  type Shot,
} from '../lib/api.js';
import { android } from '../lib/platform.js';
import { AREA, AREA_KEYS, SCREEN_KEYS, shotsIn, withShot } from '../lib/shots.js';
import { Shots } from './Shots.js';
import { mutedTextClass } from './form-styles.js';
import { ModelsProgress } from './ModelsProgress.js';

interface Props {
  state: RecorderState;
  utterances: LiveUtterance[];
  levels: number[];
  logs: LogLine[];
  models: ModelsStatus | null;
}

/** How long typing pauses before the notes are sent to the app. */
const NOTES_DELAY_MS = 400;

export function Recorder({ state, utterances, levels, logs, models }: Props) {
  const [notes, setNotes] = useState(state.notes);
  const [showTranscript, setShowTranscript] = useState(
    () => !window.matchMedia('(max-width: 639px)').matches,
  );
  const [now, setNow] = useState(Date.now());
  const [shotError, setShotError] = useState<string | null>(null);
  const [shooting, setShooting] = useState(false);
  const area = useRef<HTMLTextAreaElement>(null);
  // The notes as last typed, for a screenshot that comes by an event.
  const current = useRef(state.notes);
  const timer = useRef<number | undefined>(undefined);
  // Notes typed and not sent yet: sent with this meeting's number, so that
  // a cut between typing and sending leaves them with this meeting.
  const unsent = useRef<string | undefined>(undefined);
  const part = state.part;

  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(t);
  }, []);
  const send = () => {
    window.clearTimeout(timer.current);
    const text = unsent.current;
    unsent.current = undefined;
    return text === undefined ? Promise.resolve() : api.setNotes(part, text);
  };
  // A new meeting of the recording draws this again with empty notes (its
  // `key`): what this one had not sent yet goes to its own meeting now.
  useEffect(() => () => void send(), []);

  const edit = (text: string) => {
    current.current = text;
    setNotes(text);
    unsent.current = text;
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => void send(), NOTES_DELAY_MS);
  };
  // The screenshot's line where the cursor is, or at the end.
  const insert = (shot: Shot) => {
    const el = area.current;
    const text = current.current;
    const at = el && document.activeElement === el ? el.selectionStart : text.length;
    const next = withShot(text, at, shot);
    edit(next.text);
    window.requestAnimationFrame(() => el?.setSelectionRange(next.caret, next.caret));
    setShotError(null);
  };
  const failed = (message: string) => {
    setShotError(message);
    window.setTimeout(() => setShotError(null), 8000);
  };
  useEffect(() => {
    if (android) return;
    return subscribe(events.screenshot(insert), events.screenshotFailed(failed));
  }, []);
  const screenshot = async (kind: 'screen' | 'area') => {
    setShooting(true);
    try {
      const shot = await api.takeScreenshot(kind);
      if (shot) insert(shot);
    } catch (e) {
      failed(String(e));
    } finally {
      setShooting(false);
    }
  };
  const shots = shotsIn(notes);

  // The last keystrokes are not lost when the meeting stops.
  const stop = async () => {
    await send();
    await api.stopMeeting();
  };

  const elapsed = state.started ? (now - state.started) / 1000 : 0;
  const recording = state.phase === 'recording';

  return (
    <div className="relative flex h-full">
      <div className="relative flex min-w-0 flex-1 flex-col">
        <header
          data-tauri-drag-region
          className="flex h-11 shrink-0 items-center justify-end gap-2 px-4"
        >
          {!android && recording && (
            <button
              type="button"
              disabled={shooting}
              title={`The screen under the pointer, in the notes (${SCREEN_KEYS}, from any app)`}
              onClick={() => void screenshot('screen')}
              className={shotButtonClass}
            >
              <Camera className="size-4" />
              <span className="font-mono text-xs">{SCREEN_KEYS}</span>
            </button>
          )}
          {!android && AREA && recording && (
            <button
              type="button"
              disabled={shooting}
              title={`An area of the screen, in the notes (${AREA_KEYS}, from any app; Esc to give up)`}
              onClick={() => void screenshot('area')}
              className={shotButtonClass}
            >
              <Scan className="size-4" />
              <span className="font-mono text-xs">{AREA_KEYS}</span>
            </button>
          )}
          <button
            type="button"
            title={showTranscript ? 'Hide the transcript' : 'Show the transcript'}
            onClick={() => setShowTranscript((v) => !v)}
            className="rounded-md p-1.5 text-neutral-500 hover:bg-neutral-100 dark:hover:bg-neutral-800"
          >
            {showTranscript ? (
              <PanelRightClose className="size-4" />
            ) : (
              <PanelRightOpen className="size-4" />
            )}
          </button>
        </header>
        <div className="mx-auto flex w-full max-w-2xl flex-1 flex-col overflow-y-auto px-10 pb-28 max-sm:px-4">
          <h1 className="text-3xl font-semibold tracking-tight">
            {state.title || 'Untitled meeting'}
          </h1>
          <p className={`mt-1 mb-6 text-sm ${mutedTextClass}`}>
            {state.started
              ? new Date(state.started).toLocaleString(undefined, {
                  weekday: 'long',
                  hour: '2-digit',
                  minute: '2-digit',
                })
              : 'Getting ready…'}
          </p>
          {shotError && <p className="mb-3 text-sm text-red-600">{shotError}</p>}
          <textarea
            ref={area}
            autoFocus={!android}
            readOnly={state.phase === 'finishing'}
            value={notes}
            onChange={(e) => edit(e.target.value)}
            placeholder={
              'Write notes…\n\nA line per point is enough: the report completes each one from what was said.'
            }
            className="min-h-[50vh] w-full flex-1 resize-none border-none bg-transparent text-[15px] leading-relaxed outline-none placeholder:text-neutral-400"
          />
          {shots.length > 0 && <Shots folder="screenshots" shots={shots} />}
        </div>

        <Pill
          phase={state.phase}
          channels={state.channels}
          levels={levels}
          elapsed={elapsed}
          lastLog={logs[logs.length - 1]}
          onStop={() => void stop()}
          canStop={recording || state.phase === 'preparing'}
        />
      </div>

      {showTranscript && (
        <LiveTranscript
          utterances={utterances}
          me={state.me}
          phase={state.phase}
          ready={state.ready}
          models={models}
        />
      )}
    </div>
  );
}

function Pill({
  phase,
  channels,
  levels,
  elapsed,
  lastLog,
  onStop,
  canStop,
}: {
  phase: RecorderState['phase'];
  channels: string[];
  levels: number[];
  elapsed: number;
  lastLog: LogLine | undefined;
  onStop: () => void;
  canStop: boolean;
}) {
  return (
    <div className="pointer-events-none absolute inset-x-0 bottom-6 z-20 flex justify-center px-6 max-sm:bottom-3 max-sm:px-3">
      <div className="pointer-events-auto flex max-w-full items-center gap-4 rounded-full border border-neutral-200 bg-white/95 py-2 pr-2 pl-5 shadow-xl backdrop-blur dark:border-neutral-700 dark:bg-neutral-900/95">
        {phase === 'recording' ? (
          <>
            {channels.map((c, i) => (
              <Meter key={c} label={c === 'mic' ? 'You' : 'Call'} level={levels[i] ?? 0} />
            ))}
            <span className="font-mono text-sm tabular-nums">{clock(elapsed)}</span>
          </>
        ) : (
          <span className="max-w-md truncate text-sm text-neutral-600 dark:text-neutral-300">
            {phase === 'preparing' ? 'Getting ready' : 'Finishing'}
            {lastLog ? ` · ${lastLog.message}` : '…'}
          </span>
        )}
        {canStop ? (
          <button
            type="button"
            onClick={onStop}
            className="flex h-9 items-center gap-2 rounded-full bg-red-700 px-4 text-sm font-medium text-white hover:bg-red-800"
          >
            <Square className="size-3.5 fill-current" />
            Stop
          </button>
        ) : (
          <span className="mr-3 size-4 animate-spin rounded-full border-2 border-neutral-300 border-t-accent" />
        )}
      </div>
    </div>
  );
}

const shotButtonClass =
  'flex items-center gap-1.5 rounded-md px-2 py-1.5 text-sm text-neutral-500 hover:bg-neutral-100 disabled:opacity-50 dark:hover:bg-neutral-800';

/** Five bars that follow a channel's level. */
function Meter({ label, level }: { label: string; level: number }) {
  // Speech peaks are far below full scale: a log scale shows a voice.
  const db = 20 * Math.log10(Math.max(level, 1e-4));
  const lit = Math.max(0, Math.min(1, (db + 50) / 44));
  const shape = [0.45, 0.75, 1, 0.75, 0.45];
  return (
    <span className="flex items-center gap-1.5" title={label}>
      <span className="flex h-5 items-center gap-[3px]">
        {shape.map((s, i) => (
          <span
            key={i}
            className="meter-bar w-[3px] rounded-full bg-accent"
            style={{ height: `${Math.max(3, lit * s * 20)}px` }}
          />
        ))}
      </span>
      <span className="text-xs text-neutral-500">{label}</span>
    </span>
  );
}

function LiveTranscript({
  utterances,
  me,
  phase,
  ready,
  models,
}: {
  utterances: LiveUtterance[];
  me: string;
  phase: RecorderState['phase'];
  ready: boolean;
  models: ModelsStatus | null;
}) {
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => {
    end.current?.scrollIntoView({ block: 'end' });
  }, [utterances.length]);

  return (
    <aside className="flex w-80 shrink-0 flex-col border-l border-neutral-200 bg-sidebar max-sm:absolute max-sm:inset-x-0 max-sm:top-11 max-sm:bottom-0 max-sm:z-10 max-sm:w-auto max-sm:border-l-0 max-sm:border-t dark:border-neutral-800">
      <div
        data-tauri-drag-region
        className="flex h-11 shrink-0 items-center px-4 text-xs font-medium text-neutral-500"
      >
        Live transcript
      </div>
      <div className="flex-1 space-y-3 overflow-y-auto px-4 pb-6 max-sm:pb-24">
        {!ready && phase !== 'finishing' && (
          <div className="pt-4">
            <ModelsProgress status={models} />
          </div>
        )}
        {utterances.length === 0 && (
          <p className="pt-8 text-center text-sm text-neutral-500">
            {phase === 'preparing'
              ? 'Loading…'
              : phase === 'finishing'
                ? 'Nothing was heard.'
                : ready
                  ? 'Listening…'
                  : 'Recording has started: what is said now appears once the transcription is ready.'}
          </p>
        )}
        {utterances.map((u) => {
          const mine = u.channel === 'mic' && u.speaker === null;
          return (
            <div
              key={`${u.channel}:${u.start}`}
              className={`flex flex-col ${mine ? 'items-end' : 'items-start'}`}
            >
              <span className="mb-0.5 text-xs text-neutral-500">
                {liveLabel(u, me)} · {clock(u.start)}
              </span>
              <p
                className={`max-w-[90%] rounded-2xl px-3 py-1.5 text-sm ${
                  mine
                    ? 'rounded-br-sm bg-accent-fill text-white'
                    : 'rounded-bl-sm bg-white text-neutral-800 shadow-sm dark:bg-neutral-800 dark:text-neutral-200'
                }`}
              >
                {u.text}
              </p>
            </div>
          );
        })}
        <div ref={end} />
      </div>
      <p
        className={`border-t border-neutral-200 px-4 py-3 text-sm max-sm:hidden dark:border-neutral-800 ${mutedTextClass}`}
      >
        Speakers are numbered as heard live; the final transcript names them once for the whole
        meeting.
      </p>
    </aside>
  );
}
