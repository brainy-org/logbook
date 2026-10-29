// The meetings, the latest first and grouped by day, the one being
// recorded on top, and the settings at the bottom. A meeting's right
// click opens its menu (Delete).
import { useEffect, useState } from 'react';
import { Plus, Settings, Trash2 } from 'lucide-react';
import type { View } from '../App.js';
import { duration, type MeetingSummary, type RecorderState } from '../lib/api.js';

interface Props {
  view: View;
  recorder: RecorderState | null;
  meetings: MeetingSummary[];
  onView: (view: View) => void;
  onDelete: (meeting: MeetingSummary) => void;
}

/** "Today", "Yesterday", or the date. */
function day(date: Date): string {
  const today = new Date();
  const yesterday = new Date();
  yesterday.setDate(today.getDate() - 1);
  if (date.toDateString() === today.toDateString()) return 'Today';
  if (date.toDateString() === yesterday.toDateString()) return 'Yesterday';
  return date.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
}

const row =
  'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-neutral-200/60 dark:hover:bg-neutral-800';
const active = 'bg-neutral-200/80 dark:bg-neutral-800';

export function Sidebar({ view, recorder, meetings, onView, onDelete }: Props) {
  const [menu, setMenu] = useState<{ meeting: MeetingSummary; x: number; y: number } | null>(null);
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const escape = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close();
    };
    window.addEventListener('click', close);
    window.addEventListener('blur', close);
    window.addEventListener('keydown', escape);
    return () => {
      window.removeEventListener('click', close);
      window.removeEventListener('blur', close);
      window.removeEventListener('keydown', escape);
    };
  }, [menu]);
  const groups: { day: string; meetings: MeetingSummary[] }[] = [];
  for (const m of meetings) {
    const d = day(new Date(m.started));
    const last = groups[groups.length - 1];
    if (last && last.day === d) last.meetings.push(m);
    else groups.push({ day: d, meetings: [m] });
  }
  const home = view.kind === 'home';

  return (
    <aside className="flex w-60 shrink-0 flex-col border-r border-neutral-200 bg-sidebar dark:border-neutral-800">
      {/* Room for the window's buttons, and a handle to drag it by. */}
      <div data-tauri-drag-region className="h-11 shrink-0" />
      <div className="px-2">
        <button
          type="button"
          onClick={() => onView({ kind: 'home' })}
          className={`${row} ${home ? active : ''} font-medium`}
        >
          {recorder ? (
            <>
              <span className="relative flex size-4 items-center justify-center">
                <span className="absolute size-2.5 animate-ping rounded-full bg-red-500/60" />
                <span className="size-2 rounded-full bg-red-500" />
              </span>
              {recorder.phase === 'finishing' ? 'Finishing…' : recorder.title || 'Recording'}
            </>
          ) : (
            <>
              <Plus className="size-4 text-neutral-500" />
              New meeting
            </>
          )}
        </button>
      </div>
      <nav className="mt-3 min-h-0 flex-1 overflow-y-auto px-2 pb-2">
        {groups.length === 0 && (
          <p className="px-2 py-6 text-center text-xs text-neutral-500">
            Your meetings will be listed here.
          </p>
        )}
        {groups.map((g) => (
          <section key={g.day} className="mb-3">
            <h3 className="px-2 pb-1 text-xs font-medium text-neutral-500">{g.day}</h3>
            {g.meetings.map((m) => {
              const selected = view.kind === 'meeting' && view.id === m.id;
              const time = new Date(m.started).toLocaleTimeString(undefined, {
                hour: '2-digit',
                minute: '2-digit',
              });
              return (
                <button
                  key={m.id}
                  type="button"
                  onClick={() => onView({ kind: 'meeting', id: m.id })}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    setMenu({ meeting: m, x: e.clientX, y: e.clientY });
                  }}
                  className={`${row} ${selected ? active : ''} ${menu?.meeting.id === m.id ? active : ''} flex-col !items-start !gap-0 select-none`}
                >
                  <span className="w-full truncate">{m.title}</span>
                  <span className="text-xs text-neutral-500">
                    {time} · {duration(m.duration_s)}
                  </span>
                </button>
              );
            })}
          </section>
        ))}
      </nav>
      {menu && (
        <div
          role="menu"
          style={{ left: menu.x, top: menu.y }}
          className="fixed z-50 min-w-36 rounded-md border border-neutral-200 bg-white p-1 shadow-lg dark:border-neutral-700 dark:bg-neutral-900"
        >
          <button
            type="button"
            role="menuitem"
            onClick={() => onDelete(menu.meeting)}
            className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-sm text-red-700 hover:bg-neutral-100 dark:text-red-400 dark:hover:bg-neutral-800"
          >
            <Trash2 className="size-3.5" />
            Delete
          </button>
        </div>
      )}
      <div className="border-t border-neutral-200 p-2 dark:border-neutral-800">
        <button
          type="button"
          onClick={() => onView({ kind: 'settings' })}
          className={`${row} ${view.kind === 'settings' ? active : ''}`}
        >
          <Settings className="size-4 text-neutral-500" />
          Settings
        </button>
      </div>
    </aside>
  );
}
