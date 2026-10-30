// The meetings, the latest first and grouped by day, the one being
// recorded on top, and the voices known by name and the settings at the bottom. A meeting's right
// click opens the system's menu (Delete); Backspace or Delete on its row
// deletes it too, undone from the app's notice (App.tsx).
import type { MouseEvent } from 'react';
import { Plus, Settings, Users } from 'lucide-react';
import { LogicalPosition } from '@tauri-apps/api/dpi';
import { Menu } from '@tauri-apps/api/menu';
import { android } from '../lib/platform.js';
import type { View } from '../App.js';
import { duration, type MeetingSummary, type RecorderState } from '../lib/api.js';

interface Props {
  view: View;
  recorder: RecorderState | null;
  meetings: MeetingSummary[];
  onView: (view: View) => void;
  onDelete: (id: string) => void;
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

/** The meeting's menu, the system's own; none on Android (its page deletes it). */
async function openMenu(e: MouseEvent, onDelete: () => void) {
  e.preventDefault();
  if (android) return;
  const menu = await Menu.new({
    items: [{ id: 'delete', text: 'Delete', accelerator: 'CmdOrCtrl+Backspace', action: onDelete }],
  });
  await menu.popup(new LogicalPosition(e.clientX, e.clientY));
}

export function Sidebar({ view, recorder, meetings, onView, onDelete }: Props) {
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
                  onContextMenu={(e) => void openMenu(e, () => onDelete(m.id))}
                  onKeyDown={(e) => {
                    if (e.key === 'Backspace' || e.key === 'Delete') {
                      e.preventDefault();
                      onDelete(m.id);
                    }
                  }}
                  className={`${row} ${selected ? active : ''} flex-col !items-start !gap-0 select-none`}
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
      <div className="border-t border-neutral-200 p-2 dark:border-neutral-800">
        <button
          type="button"
          onClick={() => onView({ kind: 'voices' })}
          className={`${row} ${view.kind === 'voices' ? active : ''}`}
        >
          <Users className="size-4 text-neutral-500" />
          Voices
        </button>
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
