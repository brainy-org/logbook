// The voices known by name: the people a meeting's speakers are named
// from, kept on this computer only (crates/client voices.rs). A list made
// for hundreds of people: searched as one types (any case, any accent),
// sorted by when they were last heard (recognized or named) or by name,
// either way round, the ones with too little
// voice to be recognized every time marked and filterable. A person can be
// renamed (refused onto someone else's name) or forgotten, after asking.
// Their voiceprints never reach this page: only names, seconds and the
// meetings they come from.
import { useDeferredValue, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { ArrowDown, ArrowUp, AudioLines, Check, Pencil, Search, Trash2, X } from 'lucide-react';
import { ask } from '@tauri-apps/plugin-dialog';
import { android } from '../lib/platform.js';
import { api, type MeetingSummary, type Voice } from '../lib/api.js';
import {
  compactFieldClass,
  fieldClass,
  mutedTextClass,
  quietDangerButtonClass,
} from './form-styles.js';

type Sort = 'heard' | 'name';

/** Lower case without accents: "Élodie" is found typing "elo". */
const fold = (s: string) => s.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();

const collator = new Intl.Collator(undefined, { sensitivity: 'base', numeric: true });

/** "42 s", "2 min 11 s". */
function voiceLength(seconds: number): string {
  const s = Math.round(seconds);
  if (s < 60) return `${s} s`;
  return s % 60 === 0 ? `${s / 60} min` : `${Math.floor(s / 60)} min ${s % 60} s`;
}

/** A meeting's id (`YYYYMMDD-HHMMSS`) as a date: "27 Oct", "3 Mar 2025". */
function meetingDate(id: string): string | null {
  const m = /^(\d{4})(\d{2})(\d{2})-/.exec(id);
  if (!m) return null;
  const date = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  const thisYear = date.getFullYear() === new Date().getFullYear();
  return date.toLocaleDateString(undefined, {
    day: 'numeric',
    month: 'short',
    year: thisYear ? undefined : 'numeric',
  });
}

const initials = (name: string) =>
  name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0]!.toUpperCase())
    .join('');

interface Props {
  /** The meetings kept, to link the one a person was last heard in. */
  meetings: MeetingSummary[];
  onOpenMeeting: (id: string) => void;
}

export function VoicesPage({ meetings, onOpenMeeting }: Props) {
  const [voices, setVoices] = useState<Voice[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [sort, setSort] = useState<Sort>('heard');
  // Reversed from each sort's natural order: the most recent first, A to Z.
  const [reversed, setReversed] = useState(false);
  const [weakOnly, setWeakOnly] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  const search = useRef<HTMLInputElement>(null);
  // The list follows the typing without holding it up, with hundreds of rows.
  const wanted = fold(useDeferredValue(query).trim());
  const here = android ? 'this phone' : 'this computer';

  const load = () =>
    api.listVoices().then(
      (v) => {
        setVoices(v);
        setError(null);
      },
      (e) => setError(String(e)),
    );
  useEffect(() => void load(), []);

  // ⌘F (Ctrl+F) finds a person.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'f') {
        e.preventDefault();
        search.current?.focus();
        search.current?.select();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const titles = useMemo(() => new Map(meetings.map((m) => [m.id, m.title])), [meetings]);

  // Folded names once per load, sorted once per sort: a keystroke only filters.
  const sorted = useMemo(() => {
    const all = (voices ?? []).map((v) => ({ voice: v, key: fold(v.name) }));
    const byName = (a: (typeof all)[number], b: (typeof all)[number]) =>
      collator.compare(a.voice.name, b.voice.name);
    all.sort((a, b) =>
      sort === 'name' ? byName(a, b) : b.voice.heard.localeCompare(a.voice.heard) || byName(a, b),
    );
    return reversed ? all.reverse() : all;
  }, [voices, sort, reversed]);
  const order =
    sort === 'name' ? (reversed ? 'Z to A' : 'A to Z') : reversed ? 'Oldest first' : 'Newest first';
  const weak = useMemo(() => sorted.filter((v) => v.voice.weak).length, [sorted]);
  const shown = useMemo(
    () =>
      sorted.filter(
        (v) => (!weakOnly || v.voice.weak) && (wanted === '' || v.key.includes(wanted)),
      ),
    [sorted, wanted, weakOnly],
  );

  const forget = async (name: string) => {
    if (
      !(await ask(
        `${name} will no longer be recognized in the next meetings. The meetings already written keep the name.`,
        { title: `Forget ${name}'s voice?`, kind: 'warning' },
      ))
    )
      return;
    try {
      await api.forgetVoice(name);
    } catch (e) {
      setError(String(e));
    }
    await load();
  };

  const total = voices?.length ?? 0;
  const filtering = wanted !== '' || weakOnly;

  return (
    <div className="flex h-full flex-col">
      <div data-tauri-drag-region className="h-11 shrink-0 max-sm:hidden" />
      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto max-w-2xl px-10 pb-24 max-sm:px-4 max-sm:pt-4">
          <div className="flex items-baseline justify-between gap-3">
            <h1 className="text-2xl font-semibold tracking-tight">Voices</h1>
            {total > 0 && (
              <span className="text-sm text-neutral-500 tabular-nums">
                {filtering ? `${shown.length} of ${total}` : total}{' '}
                {total === 1 ? 'person' : 'people'}
              </span>
            )}
          </div>
          <p className={`mt-2 text-sm ${mutedTextClass}`}>
            The people your meetings&apos; speakers are named from, kept on {here} only. Ask before
            you name someone: a voiceprint is personal data.
          </p>

          {error && (
            <p className="mt-4 rounded-md bg-red-50 px-3 py-2 text-sm text-red-600 dark:bg-red-950/40">
              {error}
            </p>
          )}

          {voices !== null && total === 0 && <Empty here={here} />}

          {total > 0 && (
            <>
              <div className="sticky top-0 z-10 -mx-1 mt-6 flex flex-wrap items-center gap-2 bg-white px-1 py-2 dark:bg-neutral-950">
                <div className="relative min-w-0 flex-1 basis-40 max-sm:basis-full">
                  <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-neutral-400" />
                  <input
                    ref={search}
                    type="search"
                    value={query}
                    autoFocus={!android}
                    onChange={(e) => setQuery(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Escape' && query) {
                        e.preventDefault();
                        setQuery('');
                      }
                    }}
                    placeholder="Find a person"
                    aria-label="Find a person"
                    className={`${fieldClass} w-full pr-8 pl-8 [&::-webkit-search-cancel-button]:hidden`}
                  />
                  {query && (
                    <button
                      type="button"
                      title="Clear"
                      onClick={() => {
                        setQuery('');
                        search.current?.focus();
                      }}
                      className="absolute top-1/2 right-1.5 -translate-y-1/2 rounded p-1 text-neutral-400 hover:bg-neutral-100 hover:text-neutral-600 dark:hover:bg-neutral-800 dark:hover:text-neutral-300"
                    >
                      <X className="size-3.5" />
                    </button>
                  )}
                </div>
                <Segmented
                  label="Sort"
                  value={sort}
                  onChange={(s) => {
                    setSort(s);
                    setReversed(false);
                  }}
                  options={[
                    { value: 'heard', label: 'Last heard' },
                    { value: 'name', label: 'Name' },
                  ]}
                />
                <button
                  type="button"
                  onClick={() => setReversed(!reversed)}
                  // One icon, so the toolbar stays on one line: the order is in its title.
                  title={`${order}: click to reverse`}
                  aria-label={`${order}: reverse the order`}
                  className="flex size-8 shrink-0 items-center justify-center rounded-md border border-neutral-300 text-neutral-700 hover:bg-neutral-100 dark:border-neutral-600 dark:text-neutral-300 dark:hover:bg-neutral-800"
                >
                  {reversed ? <ArrowUp className="size-4" /> : <ArrowDown className="size-4" />}
                </button>
                {weak > 0 && (
                  <button
                    type="button"
                    aria-pressed={weakOnly}
                    onClick={() => setWeakOnly(!weakOnly)}
                    title="People with less than 30 s of voice from any one meeting"
                    className={`h-8 shrink-0 rounded-md border px-2.5 text-xs font-medium whitespace-nowrap ${
                      weakOnly
                        ? 'border-amber-400 bg-amber-50 text-amber-800 dark:border-amber-600 dark:bg-amber-950/40 dark:text-amber-300'
                        : 'border-neutral-300 text-neutral-700 hover:bg-neutral-100 dark:border-neutral-600 dark:text-neutral-300 dark:hover:bg-neutral-800'
                    }`}
                  >
                    {weak} need more voice
                  </button>
                )}
              </div>

              {shown.length === 0 ? (
                <div className="py-12 text-center text-sm text-neutral-500">
                  <p>
                    {wanted
                      ? `No one known as “${query.trim()}”${weakOnly ? ' among those who need more voice' : ''}.`
                      : 'No one needs more voice.'}
                  </p>
                  <button
                    type="button"
                    onClick={() => {
                      setQuery('');
                      setWeakOnly(false);
                    }}
                    className="mt-2 font-medium text-accent hover:underline"
                  >
                    Show everyone
                  </button>
                </div>
              ) : (
                <ul className="mt-2 divide-y divide-neutral-200 rounded-md border border-neutral-200 dark:divide-neutral-800 dark:border-neutral-800">
                  {shown.map(({ voice: v, key }) => (
                    <Row
                      key={v.name}
                      voice={v}
                      match={highlight(v.name, key, wanted)}
                      lastTitle={titles.get(v.heard)}
                      editing={editing === v.name}
                      taken={(name) =>
                        voices!.find(
                          (o) => o.name !== v.name && o.name.toLowerCase() === name.toLowerCase(),
                        )?.name
                      }
                      onEdit={(on) => setEditing(on ? v.name : null)}
                      onRenamed={() => {
                        setEditing(null);
                        void load();
                      }}
                      onForget={() => void forget(v.name)}
                      onOpenMeeting={onOpenMeeting}
                    />
                  ))}
                </ul>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}

/** The name with the part that matches the search in bold, when it can be placed. */
function highlight(name: string, key: string, wanted: string): ReactNode {
  const at = wanted ? key.indexOf(wanted) : -1;
  // Folding kept the length (precomposed accents): the match is at the same place.
  if (at < 0 || key.length !== name.length) return name;
  return (
    <>
      {name.slice(0, at)}
      <mark className="rounded-sm bg-accent/15 text-inherit">
        {name.slice(at, at + wanted.length)}
      </mark>
      {name.slice(at + wanted.length)}
    </>
  );
}

function Row({
  voice: v,
  match,
  lastTitle,
  editing,
  taken,
  onEdit,
  onRenamed,
  onForget,
  onOpenMeeting,
}: {
  voice: Voice;
  match: ReactNode;
  lastTitle: string | undefined;
  editing: boolean;
  /** The other person already known by `name` (any case), if any. */
  taken: (name: string) => string | undefined;
  onEdit: (on: boolean) => void;
  onRenamed: () => void;
  onForget: () => void;
  onOpenMeeting: (id: string) => void;
}) {
  const date = meetingDate(v.heard);
  return (
    <li className="group flex items-start gap-3 px-3 py-2.5">
      <span
        aria-hidden
        className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-full bg-neutral-100 text-xs font-medium text-neutral-600 dark:bg-neutral-800 dark:text-neutral-300"
      >
        {initials(v.name)}
      </span>
      <div className="min-w-0 flex-1">
        {editing ? (
          <Rename name={v.name} taken={taken} onCancel={() => onEdit(false)} onDone={onRenamed} />
        ) : (
          <>
            <p className="flex items-center gap-2 text-sm">
              <span className="truncate font-medium">{match}</span>
              {v.weak && (
                <span className="shrink-0 rounded-full bg-amber-100 px-1.5 py-px text-[11px] font-medium text-amber-800 dark:bg-amber-950/60 dark:text-amber-300">
                  Needs more voice
                </span>
              )}
            </p>
            <p className="mt-0.5 text-xs text-neutral-500">
              {voiceLength(v.seconds)} of voice · {v.meetings} meeting{v.meetings === 1 ? '' : 's'}
              {date && (
                <>
                  {' · last heard '}
                  {date}
                  {lastTitle !== undefined && (
                    <>
                      {' in '}
                      <button
                        type="button"
                        onClick={() => onOpenMeeting(v.heard)}
                        className="max-w-48 truncate align-bottom text-accent hover:underline"
                      >
                        {lastTitle}
                      </button>
                    </>
                  )}
                </>
              )}
            </p>
            {v.weak && (
              <p className="mt-0.5 text-xs text-amber-700 dark:text-amber-400">
                Under 30 s from any one meeting, so not always recognized: name them in another
                meeting to improve.
              </p>
            )}
          </>
        )}
      </div>
      {!editing && (
        // Quiet until the row is hovered or focused; always shown on a touch screen.
        <div className="flex shrink-0 items-center opacity-0 group-focus-within:opacity-100 group-hover:opacity-100 touch:opacity-100">
          <button
            type="button"
            title={`Rename ${v.name}`}
            aria-label={`Rename ${v.name}`}
            onClick={() => onEdit(true)}
            className="h-8 rounded-md px-2.5 text-neutral-700 hover:bg-neutral-100 focus-visible:outline-2 focus-visible:outline-accent dark:text-neutral-300 dark:hover:bg-neutral-800"
          >
            <Pencil className="size-4" />
          </button>
          <button
            type="button"
            title={`Forget ${v.name}'s voice`}
            aria-label={`Forget ${v.name}'s voice`}
            onClick={onForget}
            className={quietDangerButtonClass}
          >
            <Trash2 className="size-4" />
          </button>
        </div>
      )}
    </li>
  );
}

/** A person's name edited in place: Enter saves, Esc gives up. */
function Rename({
  name,
  taken,
  onCancel,
  onDone,
}: {
  name: string;
  taken: (name: string) => string | undefined;
  onCancel: () => void;
  onDone: () => void;
}) {
  const [value, setValue] = useState(name);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const next = value.trim();
  const other = next ? taken(next) : undefined;
  const problem = other ? `${other} is someone else: forget one of them first.` : error;

  const save = async () => {
    if (!next || other || saving) return;
    if (next === name) return onCancel();
    setSaving(true);
    try {
      await api.renameVoice(name, next);
      onDone();
    } catch (e) {
      setError(String(e));
      setSaving(false);
    }
  };

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <div className="flex items-center gap-1.5">
        <input
          autoFocus
          value={value}
          disabled={saving}
          onChange={(e) => {
            setValue(e.target.value);
            setError(null);
          }}
          onFocus={(e) => e.target.select()}
          onKeyDown={(e) => {
            if (e.key === 'Escape') {
              e.preventDefault();
              e.stopPropagation();
              onCancel();
            }
          }}
          aria-label={`New name for ${name}`}
          className={`${compactFieldClass} min-w-0 flex-1`}
        />
        <button
          type="submit"
          title="Save"
          aria-label="Save"
          disabled={!next || !!other || saving}
          className="rounded-md p-1.5 text-accent hover:bg-neutral-100 disabled:opacity-40 dark:hover:bg-neutral-800"
        >
          <Check className="size-4" />
        </button>
        <button
          type="button"
          title="Cancel"
          aria-label="Cancel"
          onClick={onCancel}
          className="rounded-md p-1.5 text-neutral-500 hover:bg-neutral-100 dark:hover:bg-neutral-800"
        >
          <X className="size-4" />
        </button>
      </div>
      <p className={`mt-1 text-xs ${problem ? 'text-red-600' : 'text-neutral-500'}`}>
        {problem ?? 'The next meetings use the new name; those already written keep the old one.'}
      </p>
    </form>
  );
}

function Empty({ here }: { here: string }) {
  return (
    <div className="mt-10 flex flex-col items-center rounded-lg border border-dashed border-neutral-300 px-6 py-10 text-center dark:border-neutral-700">
      <span className="flex size-10 items-center justify-center rounded-full bg-accent/10 text-accent">
        <AudioLines className="size-5" />
      </span>
      <p className="mt-4 text-sm font-medium">No voices yet</p>
      <p className={`mt-1 max-w-sm text-sm ${mutedTextClass}`}>
        Open a meeting and click a speaker (&ldquo;Speaker 2&rdquo;) to give them a name. Their
        voice is kept on {here}, and the next meetings name them by themselves.
      </p>
      <p className="mt-3 max-w-sm text-xs text-neutral-500">
        About 30 s of their voice, heard alone, makes them recognized every time.
      </p>
    </div>
  );
}

function Segmented<T extends string>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
}) {
  return (
    <div
      role="radiogroup"
      aria-label={label}
      className="flex h-8 shrink-0 items-center rounded-md border border-neutral-300 p-0.5 dark:border-neutral-600"
    >
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={value === o.value}
          onClick={() => onChange(o.value)}
          className={`h-full rounded px-2.5 text-xs font-medium whitespace-nowrap ${
            value === o.value
              ? 'bg-neutral-200/80 text-neutral-900 dark:bg-neutral-700 dark:text-neutral-100'
              : 'text-neutral-600 hover:text-neutral-900 dark:text-neutral-400 dark:hover:text-neutral-200'
          }`}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
