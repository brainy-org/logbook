// A past meeting: its report, your notes and the final transcript, the
// tasks proposed in brainy (to confirm there, each a click from its
// moment on the page), and its
// speakers, each of which can be given a name ("Speaker 2" → "Luca"):
// here, on the brainy page, and in the voices known by name, so that the
// next meetings name them by themselves.
import { useEffect, useRef, useState, type FormEvent } from 'react';
import { ExternalLink, ListChecks, RefreshCw, Trash2, UserRound } from 'lucide-react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { api, clock, duration, type MeetingView } from '../lib/api.js';
import { Markdown } from './Markdown.js';
import { NotesEditor } from './NotesEditor.js';
import {
  compactFieldClass,
  mutedTextClass,
  primaryButtonSmClass,
  quietDangerButtonClass,
  secondaryButtonSmClass,
} from './form-styles.js';

type Tab = 'report' | 'notes' | 'transcript';

interface Props {
  id: string;
  /** Deleted at once: the app offers to undo it. */
  onDelete: () => void;
  onChanged: () => void;
}

export function MeetingPage({ id, onDelete, onChanged }: Props) {
  const [meeting, setMeeting] = useState<MeetingView | null>(null);
  const [tab, setTab] = useState<Tab>('report');
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [writing, setWriting] = useState(false);

  useEffect(() => {
    api.getMeeting(id).then(
      (m) => {
        setMeeting(m);
        if (!m.report && !m.reportError) setTab(m.notes ? 'notes' : 'transcript');
      },
      (e) => setError(String(e)),
    );
  }, [id]);

  if (!meeting) {
    return (
      <div className="flex h-full flex-col">
        <div data-tauri-drag-region className="h-11 shrink-0" />
        <p className="p-10 text-sm text-red-600">{error}</p>
      </div>
    );
  }

  const rewrite = async () => {
    setWriting(true);
    setError(null);
    try {
      const m = await api.writeReport(meeting.id);
      setMeeting(m);
      setTab('report');
      if (m.reportError) setError(m.reportError);
      // The rename's notice offered this: it is done.
      else setNotice(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setWriting(false);
    }
  };

  const started = new Date(meeting.started);

  return (
    <div className="flex h-full flex-col">
      <header
        data-tauri-drag-region
        className="flex h-11 shrink-0 items-center justify-end gap-1 px-4"
      >
        {meeting.page && (
          <button
            type="button"
            className={secondaryButtonSmClass}
            onClick={() => openUrl(meeting.page!).catch((e) => setError(String(e)))}
          >
            <span className="flex items-center gap-1.5">
              <ExternalLink className="size-3.5" />
              Open in brainy
            </span>
          </button>
        )}
        <button
          type="button"
          title="Delete (⌘⌫)"
          className={quietDangerButtonClass}
          onClick={onDelete}
        >
          <Trash2 className="size-4" />
        </button>
      </header>
      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto max-w-2xl px-10 pb-16 max-sm:px-4">
          <h1 className="text-3xl font-semibold tracking-tight">{meeting.title}</h1>
          <p className={`mt-1 text-sm ${mutedTextClass}`}>
            {started.toLocaleString(undefined, {
              weekday: 'long',
              day: 'numeric',
              month: 'long',
              hour: '2-digit',
              minute: '2-digit',
            })}{' '}
            · {duration(meeting.durationS)}
          </p>

          <Speakers
            meeting={meeting}
            onRenamed={(m, message) => {
              setMeeting(m);
              setNotice(message);
              onChanged();
            }}
            onError={setError}
          />

          {notice && (
            <p className="mt-3 rounded-md bg-accent/10 px-3 py-2 text-sm text-neutral-700 dark:text-neutral-200">
              {notice}{' '}
              {meeting.report && (
                <button
                  type="button"
                  disabled={writing}
                  className="text-accent hover:underline disabled:no-underline disabled:opacity-70"
                  onClick={() => void rewrite()}
                >
                  {writing ? 'Writing the report…' : 'Write the report again with the new names'}
                </button>
              )}
            </p>
          )}
          {error && (
            <p className="mt-3 rounded-md bg-red-50 px-3 py-2 text-sm text-red-600 dark:bg-red-950/40">
              {error}
            </p>
          )}
          {meeting.warnings.map((w) => (
            <p
              key={w}
              className="mt-3 rounded-md bg-amber-50 px-3 py-2 text-sm dark:bg-amber-950/30"
            >
              {w}
            </p>
          ))}

          {meeting.tasks.length > 0 && <Tasks meeting={meeting} onError={setError} />}

          <nav className="mt-8 mb-5 flex gap-1 border-b border-neutral-200 dark:border-neutral-800">
            {(['report', 'notes', 'transcript'] as const).map((t) => (
              <button
                key={t}
                type="button"
                onClick={() => setTab(t)}
                className={`-mb-px border-b-2 px-3 pb-2 text-sm capitalize ${
                  tab === t
                    ? 'border-accent font-medium text-neutral-900 dark:text-neutral-100'
                    : 'border-transparent text-neutral-500 hover:text-neutral-800 dark:hover:text-neutral-200'
                }`}
              >
                {t}
              </button>
            ))}
          </nav>

          {tab === 'report' && (
            <section>
              {meeting.report ? (
                <Markdown text={meeting.report} shots={`meetings/${meeting.id}`} />
              ) : (
                <p className={`text-sm ${mutedTextClass}`}>
                  {meeting.reportError
                    ? `The report could not be written: ${meeting.reportError}`
                    : 'No report was asked for this meeting.'}
                </p>
              )}
              <button
                type="button"
                disabled={writing}
                onClick={() => void rewrite()}
                className={`${secondaryButtonSmClass} mt-6`}
              >
                <span className="flex items-center gap-1.5">
                  <RefreshCw className={`size-3.5 ${writing ? 'animate-spin' : ''}`} />
                  {writing ? 'Writing…' : meeting.report ? 'Write it again' : 'Write the report'}
                </span>
              </button>
            </section>
          )}
          {tab === 'notes' && (
            <Notes meeting={meeting} onSaved={(notes) => setMeeting({ ...meeting, notes })} />
          )}
          {tab === 'transcript' && <Transcript meeting={meeting} />}
        </div>
      </div>
    </div>
  );
}

/** The tasks proposed as rows "To confirm": each opens its row or its moment on the page. */
function Tasks({ meeting, onError }: { meeting: MeetingView; onError: (e: string) => void }) {
  const open = (url: string) => openUrl(url).catch((e) => onError(String(e)));
  return (
    <section className="mt-6 rounded-md border border-neutral-200 px-4 py-3 dark:border-neutral-800">
      <h2 className="flex items-center gap-1.5 text-sm font-medium">
        <ListChecks className="size-4" />
        {meeting.tasks.length === 1 ? '1 task' : `${meeting.tasks.length} tasks`} to confirm in
        brainy
      </h2>
      <ul className="mt-2 space-y-1.5 text-sm">
        {meeting.tasks.map((t) => (
          <li key={t.row} className="flex items-baseline gap-2">
            <button
              type="button"
              title="Where it was said"
              onClick={() => void open(t.moment)}
              className="shrink-0 font-mono text-xs text-neutral-500 hover:text-accent"
            >
              {clock(t.at)}
            </button>
            <button
              type="button"
              title="Open the row in brainy"
              onClick={() => void open(t.row)}
              className="text-left hover:text-accent"
            >
              {t.task}
            </button>
            <span className={`ml-auto shrink-0 text-xs ${mutedTextClass}`}>
              {[t.owner, t.due].filter(Boolean).join(' · ')}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

function Speakers({
  meeting,
  onRenamed,
  onError,
}: {
  meeting: MeetingView;
  onRenamed: (m: MeetingView, message: string) => void;
  onError: (e: string | null) => void;
}) {
  const [editing, setEditing] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [saving, setSaving] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!editing || !name.trim()) return;
    setSaving(true);
    onError(null);
    try {
      const { renamed, meeting: m } = await api.renameSpeaker(meeting.id, editing, name.trim());
      const voice = renamed.enrolled
        ? `Their voice is saved: the next meetings name ${renamed.enrolled.name} by themselves` +
          (renamed.enrolled.seconds < 30
            ? ' (from little speech: naming them again in another meeting helps).'
            : '.')
        : `Their voice is not saved (${renamed.voice_error}).`;
      const rows = renamed.rows_error
        ? ` Their tasks still name ${editing} in brainy (${renamed.rows_error}).`
        : renamed.rows > 0
          ? ` ${renamed.rows === 1 ? 'Their task names' : `Their ${renamed.rows} tasks name`} them.`
          : '';
      onRenamed(m, `${editing} is now ${name.trim()}. ${voice}${rows}`);
      setEditing(null);
    } catch (err) {
      onError(String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="mt-4 flex flex-wrap items-center gap-2">
      {meeting.speakers.map((s) =>
        editing === s.label ? (
          <form key={s.label} onSubmit={submit} className="flex items-center gap-1.5">
            <input
              autoFocus
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => e.key === 'Escape' && setEditing(null)}
              placeholder={`Who is ${s.label}?`}
              className={`${compactFieldClass} w-44`}
            />
            <button
              type="submit"
              disabled={saving || !name.trim()}
              className={primaryButtonSmClass}
            >
              {saving ? 'Saving…' : 'Name'}
            </button>
            <button
              type="button"
              className={secondaryButtonSmClass}
              onClick={() => setEditing(null)}
            >
              Cancel
            </button>
          </form>
        ) : (
          <button
            key={s.label}
            type="button"
            disabled={!s.nameable}
            title={s.nameable ? `Give ${s.label} a name` : undefined}
            onClick={() => {
              setEditing(s.label);
              // "Speaker 3 (Luca?)" offers Luca.
              setName(/\((.+)\?\)$/.exec(s.label)?.[1] ?? '');
            }}
            className="flex items-center gap-1.5 rounded-full border border-neutral-200 px-3 py-1 text-sm enabled:hover:border-accent enabled:hover:text-accent disabled:cursor-default dark:border-neutral-700"
          >
            <UserRound className="size-3.5" />
            {s.label}
          </button>
        ),
      )}
    </div>
  );
}

/** How long typing pauses before a kept meeting's notes are saved. */
const SAVE_DELAY_MS = 400;

function Notes({ meeting, onSaved }: { meeting: MeetingView; onSaved: (notes: string) => void }) {
  const text = useRef(meeting.notes);
  const saved = useRef(meeting.notes);
  const timer = useRef<number | undefined>(undefined);
  const save = () => {
    window.clearTimeout(timer.current);
    const notes = text.current;
    if (notes === saved.current) return;
    saved.current = notes;
    void api.saveMeetingNotes(meeting.id, notes).then(() => onSaved(notes));
  };
  // Leaving the tab or the meeting saves what was typed last.
  useEffect(() => save, []);
  return (
    <>
      <p className={`mb-4 text-xs ${mutedTextClass}`}>
        Edits are saved on this computer. The report and the brainy page are not rewritten.
      </p>
      <NotesEditor
        key={meeting.id}
        initial={meeting.notes}
        folder={`meetings/${meeting.id}`}
        onChange={(markdown) => {
          text.current = markdown;
          window.clearTimeout(timer.current);
          timer.current = window.setTimeout(save, SAVE_DELAY_MS);
        }}
        onBlur={save}
        placeholder="No notes were taken during this meeting. You can add some here."
      />
    </>
  );
}

/** Seconds of silence inside a turn after which its next phrase shows its time. */
const PAUSE_S = 30;

/**
 * The turns, each under its speaker and start. A phrase after a long pause
 * shows its own time, so a quiet minute is not hidden inside one turn; any
 * phrase shows its time beside it on hover. The brainy page keeps one time
 * per turn: renaming speakers there reads its turn headings as they are.
 */
function Transcript({ meeting }: { meeting: MeetingView }) {
  if (meeting.turns.length === 0) {
    return <p className={`text-sm ${mutedTextClass}`}>Nothing was said.</p>;
  }
  return (
    <div className="space-y-4">
      {meeting.turns.map((t, i) => (
        <div key={i}>
          <p className="text-sm">
            <span className="font-semibold">{t.label}</span>{' '}
            <span className="font-mono text-xs text-neutral-500">{clock(t.start)}</span>
          </p>
          {t.phrases.map((p, j) => {
            const previous = t.phrases[j - 1];
            const pause = previous !== undefined && p.start - previous.end > PAUSE_S;
            return (
              <div key={j} className={`group relative ${pause ? 'mt-4' : 'mt-1'}`}>
                {pause && <p className="font-mono text-xs text-neutral-500">{clock(p.start)}</p>}
                <p className="text-[15px] leading-relaxed">
                  <span className="absolute top-0.5 -left-10 hidden w-9 text-right font-mono text-xs leading-6 text-neutral-500 group-hover:block">
                    {clock(p.start)}
                  </span>
                  {p.text}
                </p>
              </div>
            );
          })}
        </div>
      ))}
    </div>
  );
}
