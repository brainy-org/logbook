// What a meeting being finished says, on the home page's pending meetings
// and in the recorder's pill: each step's label and how far it is, and what
// stopped it (src-tauri finisher.rs, client meeting.rs `Step`).
import type { Finishing, Step } from './api.js';

/** The step's label; `local`: the pass runs on this computer. */
export function stepLabel(step: Step, local: boolean): string {
  switch (step.stage) {
    case 'sending':
      return 'Sending the recording';
    case 'queued':
      return 'Waiting its turn on the server';
    case 'reading':
      return 'Preparing the transcript';
    case 'transcribing':
      return local ? 'Transcribing on this computer' : 'Transcribing on the server';
    case 'publishing':
      return 'Writing the page in brainy';
    case 'report':
      return 'Writing the report';
  }
}

/** How far the step is, 0 to 1, when it can be told. */
export function stepDone(step: Step): number | null {
  switch (step.stage) {
    case 'sending':
    case 'reading':
    case 'transcribing':
      return step.done;
    default:
      return null;
  }
}

/** "29 of 47 min": how much of the audio the step has gone through. */
export function stepAmount(step: Step): string | null {
  const done = stepDone(step);
  if (done === null || !('audioS' in step)) return null;
  const total = Math.max(1, Math.round(step.audioS / 60));
  return `${Math.min(total, Math.floor((done * step.audioS) / 60))} of ${total} min`;
}

export function percent(done: number): string {
  return `${Math.floor(done * 100)}%`;
}

/** "3 min", "1 h 5 min": a duration in milliseconds, rounded down. */
export function minutes(ms: number): string {
  const m = Math.floor(ms / 60_000);
  if (m < 1) return 'less than a minute';
  return m < 60 ? `${m} min` : `${Math.floor(m / 60)} h ${m % 60} min`;
}

/** "1:05": a countdown in milliseconds, rounded up. */
export function countdown(ms: number): string {
  const s = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

/** What stopped it, as the person reads it; `device`: "this phone" or "this computer". */
export function problemText(
  problem: NonNullable<Finishing['problem']>,
  device: string,
  server: string,
): { text: string; hint?: string } {
  const brainy = problem.who === 'brainy';
  switch (problem.kind) {
    case 'unreachable':
      return brainy
        ? { text: `Can't reach brainy. The transcript is kept on ${device}.` }
        : { text: `Can't reach the server at ${server}.` };
    case 'busy':
      return brainy
        ? { text: `brainy is busy or restarting. The transcript is kept on ${device}.` }
        : { text: 'The server is busy or restarting.' };
    case 'token':
      return brainy
        ? { text: 'brainy refused your token.', hint: 'Paste a new brainy token in Settings.' }
        : {
            text: `The server refused ${device}'s token.`,
            hint: "Make a new token on the server's admin page and paste it in Settings.",
          };
    case 'refused':
      return brainy
        ? { text: `brainy refused the page. The transcript is kept on ${device}.` }
        : { text: 'The server refused the recording.' };
    case 'failed':
      return { text: brainy ? 'Writing to brainy failed.' : 'Finishing this meeting failed.' };
  }
}
