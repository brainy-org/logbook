// The app's frame: the meetings on the left, and on the right the meeting
// being recorded, a past meeting, or the settings. The recorder's state
// and the live transcript live here, so that they survive moving between
// views while a meeting is recorded.
import { useCallback, useEffect, useState } from 'react';
import {
  api,
  events,
  subscribe,
  type LiveUtterance,
  type LogLine,
  type MeetingSummary,
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

export function App() {
  const [view, setView] = useState<View>({ kind: 'home' });
  const [recorder, setRecorder] = useState<RecorderState | null>(null);
  const [utterances, setUtterances] = useState<LiveUtterance[]>([]);
  const [levels, setLevels] = useState<number[]>([]);
  const [logs, setLogs] = useState<LogLine[]>([]);
  const [meetings, setMeetings] = useState<MeetingSummary[]>([]);
  const [pending, setPending] = useState<PendingMeeting[]>([]);
  const [failure, setFailure] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api.listMeetings().then(setMeetings, () => setMeetings([]));
    api.listPending().then(setPending, () => setPending([]));
  }, []);

  useEffect(() => {
    refresh();
    api.meetingState().then(setRecorder);
    api.getLogs().then(setLogs);
    return subscribe(
      events.state((s) => {
        setRecorder(s);
        if (s?.phase === 'preparing') {
          setUtterances([]);
          setFailure(null);
        }
      }),
      events.utterance((u) => setUtterances((all) => [...all, u])),
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
    main = <Recorder state={recorder} utterances={utterances} levels={levels} logs={logs} />;
  } else {
    main = (
      <Home
        pending={pending}
        failure={failure}
        logs={logs}
        onStarted={setRecorder}
        onOpenSettings={() => setView({ kind: 'settings' })}
        onFinishPending={() => api.finishPending()}
      />
    );
  }

  return (
    <div className="flex h-full">
      <Sidebar view={view} recorder={recorder} meetings={meetings} onView={setView} />
      <main className="min-w-0 flex-1 overflow-hidden">{main}</main>
    </div>
  );
}
