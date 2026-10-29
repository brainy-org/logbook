// The app's frame: the meetings on the left, and on the right the meeting
// being recorded, a past meeting, or the settings. The recorder's state
// and the live transcript live here, so that they survive moving between
// views while a meeting is recorded.
import { useCallback, useEffect, useRef, useState } from 'react';
import { Menu } from 'lucide-react';
import {
  api,
  events,
  subscribe,
  type LiveUtterance,
  type LogLine,
  type MeetingSummary,
  type ModelsStatus,
  type PendingMeeting,
  type RecorderState,
} from './lib/api.js';
import { Sidebar } from './components/Sidebar.js';
import { Recorder } from './components/Recorder.js';
import { MeetingPage } from './components/MeetingPage.js';
import { SettingsPage } from './components/SettingsPage.js';
import { Home } from './components/Home.js';

export type View = { kind: 'home' } | { kind: 'meeting'; id: string } | { kind: 'settings' };

/** How long a deleted meeting can be brought back. */
const UNDO_MS = 8000;

/** Log lines kept in the page. */
const LOG_LINES = 200;
/**
 * Phrases the live transcript keeps: the latest. A meeting of hours has
 * tens of thousands, and the page slows down holding them all; the whole
 * transcript is the final one, on the meeting's page.
 */
const LIVE_PHRASES = 300;

export function App() {
  const [view, setView] = useState<View>({ kind: 'home' });
  const [recorder, setRecorder] = useState<RecorderState | null>(null);
  const [utterances, setUtterances] = useState<LiveUtterance[]>([]);
  const [levels, setLevels] = useState<number[]>([]);
  const [logs, setLogs] = useState<LogLine[]>([]);
  const [models, setModels] = useState<ModelsStatus | null>(null);
  const [meetings, setMeetings] = useState<MeetingSummary[]>([]);
  const [pending, setPending] = useState<PendingMeeting[]>([]);
  const [failure, setFailure] = useState<string | null>(null);
  // At phone width the sidebar is a drawer, open over the page.
  const [drawer, setDrawer] = useState(false);
  const show = (next: View) => {
    setView(next);
    setDrawer(false);
  };

  const refresh = useCallback(() => {
    api.listMeetings().then(setMeetings, () => setMeetings([]));
    api.listPending().then(setPending, () => setPending([]));
  }, []);

  useEffect(() => {
    refresh();
    api.meetingState().then(setRecorder);
    api.getLogs().then(setLogs);
    api.modelsState().then(setModels);
    return subscribe(
      events.models(setModels),
      events.state((s) => {
        setRecorder(s);
        if (s?.phase === 'preparing') {
          setUtterances([]);
          setFailure(null);
        }
      }),
      events.utterance((u) => setUtterances((all) => [...all.slice(-(LIVE_PHRASES - 1)), u])),
      events.levels(setLevels),
      events.log((l) => setLogs((all) => [...all.slice(-(LOG_LINES - 1)), l])),
      events.done((outcome) => {
        refresh();
        if (outcome) setView({ kind: 'meeting', id: outcome.id });
      }),
      events.failed((message) => {
        setFailure(message);
        refresh();
      }),
      events.meetingsChanged(refresh),
    );
  }, [refresh]);

  // A meeting deleted at once, with a notice to undo it: it waits in the
  // trash until the notice goes (or the app's next start).
  const [deleted, setDeleted] = useState<{ id: string; title: string } | null>(null);
  const undoTimer = useRef<number | undefined>(undefined);
  const purge = (id: string) => void api.purgeMeeting(id).catch(() => undefined);
  const expire = (id: string) => {
    window.clearTimeout(undoTimer.current);
    undoTimer.current = window.setTimeout(() => {
      purge(id);
      setDeleted(null);
    }, UNDO_MS);
  };
  const deleteMeeting = async (id: string) => {
    const i = meetings.findIndex((m) => m.id === id);
    const m = meetings[i];
    if (!m) return;
    try {
      await api.deleteMeeting(id);
    } catch (e) {
      setFailure(String(e));
      return;
    }
    // The one deleted before is gone for good: one undo at a time.
    if (deleted) purge(deleted.id);
    setDeleted({ id, title: m.title });
    expire(id);
    if (view.kind === 'meeting' && view.id === id) {
      const next = meetings[i + 1] ?? meetings[i - 1];
      setView(next ? { kind: 'meeting', id: next.id } : { kind: 'home' });
    }
    refresh();
  };
  const undo = async () => {
    if (!deleted) return;
    window.clearTimeout(undoTimer.current);
    setDeleted(null);
    await api.restoreMeeting(deleted.id);
    refresh();
    setView({ kind: 'meeting', id: deleted.id });
  };
  // ⌘⌫ deletes the meeting shown, ⌘Z undoes it: not while typing.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target?.closest('input, textarea, [contenteditable="true"]')) return;
      if (!(e.metaKey || e.ctrlKey)) return;
      if (e.key === 'Backspace' && view.kind === 'meeting') {
        e.preventDefault();
        void deleteMeeting(view.id);
      } else if (e.key.toLowerCase() === 'z' && deleted) {
        e.preventDefault();
        void undo();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  const recording = recorder !== null;
  let main;
  if (view.kind === 'settings') {
    main = <SettingsPage recording={recording} />;
  } else if (view.kind === 'meeting') {
    main = (
      <MeetingPage
        key={view.id}
        id={view.id}
        onDelete={() => void deleteMeeting(view.id)}
        onChanged={refresh}
      />
    );
  } else if (recorder) {
    main = (
      <Recorder
        key={recorder.part}
        state={recorder}
        utterances={utterances}
        levels={levels}
        logs={logs}
        models={models}
      />
    );
  } else {
    main = (
      <Home
        pending={pending}
        failure={failure}
        logs={logs}
        models={models}
        onStarted={setRecorder}
        onOpenSettings={() => show({ kind: 'settings' })}
        onFinishPending={() => api.finishPending()}
      />
    );
  }

  return (
    <div className="flex h-full max-sm:flex-col">
      <div className="flex h-12 shrink-0 items-center gap-2 border-b border-neutral-200 px-2 sm:hidden dark:border-neutral-800">
        <button
          type="button"
          aria-label="Meetings and settings"
          onClick={() => setDrawer(true)}
          className="rounded-md p-2 text-neutral-500 hover:bg-neutral-100 dark:hover:bg-neutral-800"
        >
          <Menu className="size-5" />
        </button>
        <span className="truncate text-sm font-medium">
          {recorder
            ? recorder.phase === 'finishing'
              ? 'Finishing…'
              : recorder.title || 'Recording'
            : 'Logbook'}
        </span>
      </div>
      {drawer && (
        <div
          className="fixed inset-0 z-30 bg-black/30 sm:hidden"
          onClick={() => setDrawer(false)}
        />
      )}
      <div
        className={`flex max-sm:fixed max-sm:inset-y-0 max-sm:left-0 max-sm:z-40 max-sm:shadow-xl ${drawer ? '' : 'max-sm:hidden'}`}
      >
        <Sidebar
          view={view}
          recorder={recorder}
          meetings={meetings}
          onView={show}
          onDelete={(id) => void deleteMeeting(id)}
        />
      </div>
      {deleted && (
        <div
          role="status"
          onMouseEnter={() => window.clearTimeout(undoTimer.current)}
          onMouseLeave={() => expire(deleted.id)}
          className="fixed bottom-14 left-2 z-50 flex w-56 items-start gap-2 rounded-lg border border-neutral-200 bg-white p-3 text-sm shadow-lg max-sm:inset-x-3 max-sm:bottom-3 max-sm:w-auto dark:border-neutral-700 dark:bg-neutral-900"
        >
          <div className="min-w-0 flex-1">
            <p>Meeting deleted from this computer.</p>
            <p className="text-xs text-neutral-500">Its brainy page stays.</p>
          </div>
          <button
            type="button"
            onClick={() => void undo()}
            className="rounded-md px-2 py-1 font-medium text-accent hover:bg-neutral-100 dark:hover:bg-neutral-800"
          >
            Undo
          </button>
        </div>
      )}
      <main className="min-h-0 min-w-0 flex-1 overflow-hidden">{main}</main>
    </div>
  );
}
