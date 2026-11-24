// A past meeting: its report, your notes and the final transcript, and its
// speakers, each of which can be given a name ("Speaker 2" → "Luca"):
// here, on the brainy page, and in the voices known by name, so that the
// next meetings name them by themselves. The names a speaker could be are
// offered, and the report is written again with the new names a few
// seconds after the last rename (or on leaving the meeting).
import { useEffect, useRef, useState, type FormEvent } from 'react';
import { ExternalLink, RefreshCw, Send, Trash2, UserRound } from 'lucide-react';
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
  /** Its title in the list: renamed there, this page follows. */
  listedTitle?: string;
}

export function MeetingPage({ id, onDelete, onChanged, listedTitle }: Props) {
  const [meeting, setMeeting] = useState<MeetingView | null>(null);
  const [tab, setTab] = useState<Tab>('report');
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // The speaker just renamed whose voice was not saved, as it may not be
  // theirs: the notice offers to save it anyway.
  const [doubted, setDoubted] = useState<string | null>(null);
  const [writing, setWriting] = useState(false);
  const [sending, setSending] = useState(false);
  // Read by the rename's timer: no report written while the page is.
  const sendingNow = useRef(false);
  // A rename waiting for the report to be written again with it.
  const due = useRef<{ id: string; timer: number } | null>(null);
  const naming = useRef(false);
  // A title being saved: no report written meanwhile, nor the other way.
  const renaming = useRef(false);

  useEffect(() => {
    api.getMeeting(id).then(
      (m) => {
        setMeeting(m);
        if (!m.report && !m.reportError) setTab(m.notes ? 'notes' : 'transcript');
      },
      (e) => setError(String(e)),
    );
  }, [id]);

  // Renamed from the list's menu.
  useEffect(() => {
    if (listedTitle !== undefined)
      setMeeting((m) => (m && m.title !== listedTitle ? { ...m, title: listedTitle } : m));
  }, [listedTitle]);

  // Leaving the meeting writes its report now, not never.
  useEffect(
    () => () => {
      if (!due.current) return;
      window.clearTimeout(due.current.timer);
      void api.writeReport(due.current.id).catch(() => undefined);
      due.current = null;
    },
    [id],
  );

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
      else {
        setNotice(null);
        setDoubted(null);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setWriting(false);
    }
  };

  // After a rename, once no other follows for a while: one report for
  // several names, never one written while a name is being typed.
  const rewriteSoon = (meetingId: string) => {
    if (due.current) window.clearTimeout(due.current.timer);
    const fire = () => {
      if (naming.current || sendingNow.current || renaming.current) {
        due.current = { id: meetingId, timer: window.setTimeout(fire, REWRITE_DELAY_MS) };
        return;
      }
      due.current = null;
      void rewrite();
    };
    due.current = { id: meetingId, timer: window.setTimeout(fire, REWRITE_DELAY_MS) };
  };

  // A meeting published without brainy, to a new page. A report due
  // after a rename is written first: both load and save the meeting.
  const send = async () => {
    if (due.current) {
      window.clearTimeout(due.current.timer);
      due.current = null;
      await rewrite();
    }
    setSending(true);
    sendingNow.current = true;
    setError(null);
    try {
      const m = await api.sendToBrainy(meeting.id);
      setMeeting(m);
      onChanged();
      if (m.reportError) setError(`On brainy, without the report: ${m.reportError}`);
    } catch (e) {
      setError(String(e));
    } finally {
      sendingNow.current = false;
      setSending(false);
    }
  };

  const started = new Date(meeting.started);

  return (
    <div className="flex h-full flex-col">
      <header
        data-tauri-drag-region
        className="flex h-11 shrink-0 items-center justify-end gap-1 px-4"
      >
        {!meeting.page && (
          <button
            type="button"
            disabled={writing || sending}
            className={secondaryButtonSmClass}
            onClick={() => void send()}
          >
            <span className="flex items-center gap-1.5">
              <Send className="size-3.5" />
              {sending ? 'Sending…' : 'Send to brainy'}
            </span>
          </button>
        )}
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
          <Title
            key={meeting.id}
            meeting={meeting}
            busy={writing || sending}
            onRenaming={(on) => (renaming.current = on)}
            onRenamed={(m) => {
              setMeeting(m);
              onChanged();
            }}
            onError={setError}
          />
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
            busy={writing || sending}
            onNaming={(on) => (naming.current = on)}
            onRenamed={(m, message, doubtedName) => {
              setMeeting(m);
              setNotice(message);
              setDoubted(doubtedName);
              onChanged();
              if (m.report) rewriteSoon(m.id);
            }}
            onError={setError}
          />

          {notice && (
            <p className="mt-3 rounded-md bg-accent/10 px-3 py-2 text-sm text-neutral-700 dark:text-neutral-200">
              {notice}{' '}
              {doubted && (
                <button
                  type="button"
                  className="underline underline-offset-2"
                  onClick={() => {
                    const name = doubted;
                    api.saveVoice(meeting.id, name).then(
                      () => {
                        setDoubted(null);
                        setNotice(
                          `${name}'s voice is saved: the next meetings name them by themselves.`,
                        );
                      },
                      (e) => setError(String(e)),
                    );
                  }}
                >
                  It is {doubted}: save the voice
                </button>
              )}{' '}
              {meeting.report &&
                (writing
                  ? 'Writing the report again with the new names…'
                  : 'The report will be written again with the new names.')}
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
                disabled={writing || sending}
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

/** How long after the last rename the report is written again. */
const REWRITE_DELAY_MS = 8000;

function Speakers({
  meeting,
  busy,
  onNaming,
  onRenamed,
  onError,
}: {
  meeting: MeetingView;
  /** The report is being written: a rename now would race it. */
  busy: boolean;
  onNaming: (on: boolean) => void;
  /** `doubted`: the new name, when its voice was not saved as it may not be theirs. */
  onRenamed: (m: MeetingView, message: string, doubted: string | null) => void;
  onError: (e: string | null) => void;
}) {
  const [editing, setEditingState] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [saving, setSaving] = useState(false);
  // The names the speaker being named could be, likeliest first.
  const [candidates, setCandidates] = useState<string[]>([]);
  // Every voice known by name, searched as the name is typed.
  const [known, setKnown] = useState<string[]>([]);
  const [open, setOpen] = useState(false);
  // The suggestion the arrows are on; -1: none, Enter names what is typed.
  const [active, setActive] = useState(-1);

  // The voices that could be the speaker first, then the others that
  // match what is typed; never a name another speaker here already has.
  const taken = meeting.speakers.map((s) => s.label);
  const query = name.trim().toLowerCase();
  const suggestions = [
    ...candidates.map((n) => ({ name: n, close: true })),
    ...known.filter((n) => !candidates.includes(n)).map((n) => ({ name: n, close: false })),
  ].filter((v) => !taken.includes(v.name) && v.name.toLowerCase().includes(query));

  const setEditing = (label: string | null) => {
    setEditingState(label);
    onNaming(label !== null);
    setCandidates([]);
    setActive(-1);
    setOpen(label !== null);
    if (label === null) return;
    api.speakerCandidates(meeting.id, label).then(setCandidates, () => undefined);
    api.listVoices().then(
      (v) => setKnown(v.map((x) => x.name)),
      () => undefined,
    );
  };

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
        : renamed.voice_doubt
          ? `Their voice is not saved: ${renamed.voice_doubt}.`
          : `Their voice is not saved (${renamed.voice_error}).`;
      onRenamed(
        m,
        `${editing} is now ${name.trim()}. ${voice}`,
        renamed.voice_doubt ? name.trim() : null,
      );
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
            <div className="relative">
              <input
                autoFocus
                value={name}
                onChange={(e) => {
                  setName(e.target.value);
                  setActive(-1);
                  setOpen(true);
                }}
                onFocus={() => setOpen(true)}
                onBlur={() => setOpen(false)}
                onKeyDown={(e) => {
                  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                    e.preventDefault();
                    setOpen(true);
                    const n = suggestions.length;
                    if (n === 0) return;
                    setActive((i) =>
                      e.key === 'ArrowDown' ? (i + 1) % n : i <= 0 ? n - 1 : i - 1,
                    );
                  } else if (e.key === 'Enter' && open && suggestions[active]) {
                    // The suggestion goes in the field; Enter again names.
                    e.preventDefault();
                    setName(suggestions[active].name);
                    setOpen(false);
                    setActive(-1);
                  } else if (e.key === 'Escape') {
                    if (open && suggestions.length > 0) setOpen(false);
                    else setEditing(null);
                  }
                }}
                placeholder={`Who is ${s.label}?`}
                className={`${compactFieldClass} w-52`}
              />
              {open && suggestions.length > 0 && (
                <ul className="absolute top-full left-0 z-10 mt-1 max-h-64 w-64 overflow-y-auto rounded-md border border-neutral-200 bg-white py-1 text-sm shadow-lg dark:border-neutral-700 dark:bg-neutral-900">
                  {suggestions.map((v, i) => (
                    <li key={v.name}>
                      <button
                        type="button"
                        // Before the field's blur closes the list.
                        onMouseDown={(e) => {
                          e.preventDefault();
                          setName(v.name);
                          setOpen(false);
                          setActive(-1);
                        }}
                        onMouseEnter={() => setActive(i)}
                        className={`flex w-full items-center justify-between gap-2 px-3 py-1.5 text-left ${
                          i === active ? 'bg-neutral-100 dark:bg-neutral-800' : ''
                        }`}
                      >
                        {v.name}
                        {v.close && (
                          <span className={`text-xs ${mutedTextClass}`}>voice close</span>
                        )}
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </div>
            <button
              type="submit"
              disabled={saving || busy || !name.trim()}
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

/**
 * The title, edited in place: saved on Enter or on leaving the field, here
 * and on the brainy page; Escape gives the change up. Not while the report
 * is written or the page sent: each loads and saves the meeting, and would
 * undo the title.
 */
function Title({
  meeting,
  busy,
  onRenaming,
  onRenamed,
  onError,
}: {
  meeting: MeetingView;
  busy: boolean;
  onRenaming: (on: boolean) => void;
  onRenamed: (m: MeetingView) => void;
  onError: (e: string | null) => void;
}) {
  const [title, setTitle] = useState(meeting.title);
  const [saving, setSaving] = useState(false);
  useEffect(() => setTitle(meeting.title), [meeting.title]);
  const save = async () => {
    if (title.trim() === meeting.title) {
      setTitle(meeting.title);
      return;
    }
    setSaving(true);
    onRenaming(true);
    onError(null);
    try {
      const m = await api.renameMeeting(meeting.id, title);
      setTitle(m.title);
      onRenamed(m);
    } catch (e) {
      setTitle(meeting.title);
      onError(String(e));
    } finally {
      onRenaming(false);
      setSaving(false);
    }
  };
  return (
    <input
      value={title}
      onChange={(e) => setTitle(e.target.value)}
      onBlur={() => void save()}
      onKeyDown={(e) => {
        if (e.key === 'Escape') setTitle(meeting.title);
        if (e.key === 'Enter' || e.key === 'Escape') {
          // Escape's reset lands before the blur that saves.
          const field = e.currentTarget;
          window.setTimeout(() => field.blur());
        }
      }}
      disabled={busy || saving}
      placeholder={`Meeting ${meeting.started.slice(0, 16).replace('T', ' ')}`}
      aria-label="Title"
      title={
        meeting.page ? "The meeting's title: the page in brainy takes it" : "The meeting's title"
      }
      className="w-full bg-transparent text-3xl font-semibold tracking-tight outline-none placeholder:text-neutral-400 disabled:opacity-100"
    />
  );
}

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
