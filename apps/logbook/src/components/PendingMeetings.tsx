// The meetings left pending, on the home page: one row each, oldest first,
// saying where finishing it is (sending, transcribing on the server, writing
// to brainy…) with how far when it can be told, or what stopped it and what
// to do about it. The app finishes them on its own while it is open
// (src-tauri finisher.rs); Finish now and Try again start it again.
import { useEffect, useState } from 'react';
import { Cloud, CloudOff, FileText, Laptop, Server, Upload, type LucideIcon } from 'lucide-react';
import type { Finishing, PendingMeeting, Step } from '../lib/api.js';
import {
  countdown,
  minutes,
  percent,
  problemText,
  stepAmount,
  stepDone,
  stepLabel,
} from '../lib/finishing.js';
import { mutedTextClass, secondaryButtonSmClass } from './form-styles.js';

/** Rows shown before "and n more". */
const SHOWN = 3;

interface Props {
  pending: PendingMeeting[];
  /** "this phone" or "this computer". */
  device: string;
  server: string;
  local: boolean;
  onFinish: () => void;
  onOpenSettings: () => void;
}

export function PendingMeetings({
  pending,
  device,
  server,
  local,
  onFinish,
  onOpenSettings,
}: Props) {
  // The countdowns and the time a step has taken move on their own.
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);
  // Asked for, until a run says where it is.
  const [asked, setAsked] = useState(false);
  const anyActive = pending.some((p) => p.finishing?.active);
  useEffect(() => {
    if (anyActive) setAsked(false);
  }, [anyActive]);

  if (pending.length === 0) return null;
  const finish = () => {
    setAsked(true);
    onFinish();
  };
  const shown = pending.slice(0, SHOWN);
  return (
    <div className="mt-8 rounded-lg border border-neutral-200 px-4 py-3 text-sm dark:border-neutral-700">
      {pending.length > 1 && (
        <p className="mb-3 font-medium">Finishing {pending.length} meetings</p>
      )}
      {shown.map((p, i) => (
        <div
          key={p.id}
          className={i > 0 ? 'mt-3 border-t border-neutral-200 pt-3 dark:border-neutral-700' : ''}
        >
          <Row
            meeting={p}
            // One is finished at a time, oldest first: the others wait for it.
            behind={anyActive && !p.finishing?.active && !p.finishing?.problem}
            now={now}
            device={device}
            server={server}
            local={local}
            asked={asked}
            onFinish={finish}
            onOpenSettings={onOpenSettings}
          />
        </div>
      ))}
      {pending.length > SHOWN && (
        <p className={`mt-3 text-xs ${mutedTextClass}`}>and {pending.length - SHOWN} more</p>
      )}
    </div>
  );
}

function Row({
  meeting,
  behind,
  now,
  device,
  server,
  local,
  asked,
  onFinish,
  onOpenSettings,
}: {
  meeting: PendingMeeting;
  behind: boolean;
  now: number;
  device: string;
  server: string;
  local: boolean;
  asked: boolean;
  onFinish: () => void;
  onOpenSettings: () => void;
}) {
  const [details, setDetails] = useState(false);
  const f: Finishing | null = meeting.finishing;
  // Its title, when, how long; `right`: how far, when it can be told.
  const head = (right?: string) => (
    <p className="flex items-baseline gap-2">
      <span className="truncate font-medium">{meeting.title?.trim() || 'Untitled meeting'}</span>
      <span className={`shrink-0 text-xs ${mutedTextClass}`}>
        {when(meeting.started)} · {minutes(meeting.audioS * 1000)}
      </span>
      {right && <span className="ml-auto shrink-0 font-mono text-xs tabular-nums">{right}</span>}
    </p>
  );
  const heading = head();

  if (f?.problem) {
    const { text, hint } = problemText(f.problem, device, server);
    const retried = f.problem.retried;
    return (
      <div>
        {heading}
        <div
          className={`mt-2 rounded-md px-3 py-2 ${retried ? '' : 'bg-red-50 text-red-600 dark:bg-red-950/40'}`}
        >
          <p className="flex items-start gap-2">
            {retried && <CloudOff className="mt-0.5 size-4 shrink-0 text-amber-600" />}
            <span>
              {text}
              {retried &&
                f.retryAt &&
                f.retryAt > now &&
                ` Trying again in ${countdown(f.retryAt - now)}.`}
            </span>
          </p>
          {hint && <p className="mt-1 text-xs">{hint}</p>}
          <div className="mt-2 flex flex-wrap items-center gap-3">
            {f.problem.kind === 'token' ? (
              <button type="button" className={secondaryButtonSmClass} onClick={onOpenSettings}>
                Open settings
              </button>
            ) : (
              <button
                type="button"
                disabled={asked}
                className={secondaryButtonSmClass}
                onClick={onFinish}
              >
                {asked ? 'Trying…' : retried ? 'Try now' : 'Try again'}
              </button>
            )}
            <button
              type="button"
              className="text-xs text-accent hover:underline"
              onClick={() => setDetails(!details)}
            >
              {details ? 'Hide details' : 'Details'}
            </button>
          </div>
          {details && (
            <pre className={`mt-2 text-xs break-words whitespace-pre-wrap ${mutedTextClass}`}>
              {f.problem.message}
            </pre>
          )}
        </div>
      </div>
    );
  }

  if (f?.busy && !f.active) {
    return (
      <div>
        {heading}
        <p className={`mt-1 text-xs ${mutedTextClass}`}>Being finished elsewhere</p>
      </div>
    );
  }

  if (!f?.active || !f.step) {
    // Not taken up yet: behind another, or waiting for a run.
    return (
      <div>
        {heading}
        <div className={`mt-1 flex items-center justify-between gap-3 text-xs ${mutedTextClass}`}>
          <span>
            {behind
              ? 'Waiting for the meeting above'
              : f?.active
                ? 'Starting…'
                : 'Waiting to be finished'}
          </span>
          {!behind && !f?.active && (
            <button
              type="button"
              disabled={asked}
              className={`${secondaryButtonSmClass} shrink-0`}
              onClick={onFinish}
            >
              {asked ? 'Finishing…' : 'Finish now'}
            </button>
          )}
        </div>
      </div>
    );
  }

  const step = f.step;
  const done = stepDone(step);
  const Icon = icon(step, local);
  const amount = stepAmount(step);
  return (
    <div>
      {head(done === null ? undefined : percent(done))}
      <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700">
        <div
          className={`h-full rounded-full bg-accent ${done === null ? 'w-full animate-pulse' : 'transition-[width] duration-300'}`}
          style={done === null ? undefined : { width: `${done * 100}%` }}
        />
      </div>
      <p className={`mt-1.5 flex flex-wrap justify-between gap-x-3 text-xs ${mutedTextClass}`}>
        <span className="flex items-center gap-1.5">
          <Icon className="size-3.5 shrink-0" />
          {stepLabel(step, local)}
        </span>
        <span className="tabular-nums">{amount ?? `for ${minutes(now - f.since)}`}</span>
      </p>
    </div>
  );
}

function icon(step: Step, local: boolean): LucideIcon {
  switch (step.stage) {
    case 'sending':
      return Upload;
    case 'publishing':
      return Cloud;
    case 'report':
      return FileText;
    default:
      return local ? Laptop : Server;
  }
}

/** "Thu 10:00": when the meeting started. */
function when(started: string): string {
  const d = new Date(started);
  if (Number.isNaN(d.getTime())) return '';
  return d.toLocaleString(undefined, { weekday: 'short', hour: '2-digit', minute: '2-digit' });
}
