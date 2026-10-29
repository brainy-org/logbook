// The app's frame: the meetings on the left, and on the right the meeting
// being recorded, a past meeting, or the settings. The recorder's state
// and the live transcript live here, so that they survive moving between
// views while a meeting is recorded.
import { useCallback, useEffect, useState } from 'react';
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

  const recording = recorder !== null;
  let main;
  if (view.kind === 'settings') {
    main = <SettingsPage recording={recording} />;
  } else if (view.kind === 'meeting') {
    main = (
      <MeetingPage
        key={view.id}
        id={view.id}
        onDeleted={() => {
          refresh();
          setView({ kind: 'home' });
        }}
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
        <Sidebar view={view} recorder={recorder} meetings={meetings} onView={show} />
      </div>
      <main className="min-h-0 min-w-0 flex-1 overflow-hidden">{main}</main>
    </div>
  );
}
