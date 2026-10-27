// The app's commands (src-tauri/src/lib.rs) and events, typed.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type Transcription = 'local' | 'server';

export interface Settings {
  transcription: Transcription;
  serverUrl: string;
  me: string;
  audio: {
    system: boolean;
    micDevice: string | null;
    systemDevice: string | null;
    headphones: boolean;
    micGainAuto: boolean;
    diarizeMic: boolean;
  };
  brainy: { enabled: boolean; url: string; parent: string };
  model: { polish: boolean; report: boolean; url: string; name: string };
}

export interface SettingsView {
  settings: Settings;
  secrets: { brainyToken: boolean; modelKey: boolean };
  data: string;
}

export interface Devices {
  inputs: string[];
  outputs: string[];
}

export type Phase = 'preparing' | 'recording' | 'finishing';

export interface RecorderState {
  phase: Phase;
  /** The models are loaded: until then the audio is kept and waits. */
  ready: boolean;
  title: string;
  started: number | null;
  channels: string[];
  me: string;
  notes: string;
}

export interface LiveUtterance {
  channel: string;
  speaker: number | null;
  start: number;
  end: number;
  text: string;
}

export interface LogLine {
  warning: boolean;
  message: string;
  at: number;
}

export interface MeetingSummary {
  id: string;
  title: string;
  started: string;
  duration_s: number;
  speakers: string[];
  page: string | null;
}

export interface MeetingView {
  id: string;
  title: string;
  started: string;
  durationS: number;
  page: string | null;
  notes: string;
  report: string | null;
  reportError: string | null;
  warnings: string[];
  speakers: { label: string; nameable: boolean }[];
  turns: { label: string; start: number; texts: string[] }[];
}

export interface Outcome {
  id: string;
  page: { id: string; url: string } | null;
  report: string | null;
  report_error: string | null;
}

export interface Renamed {
  enrolled: { name: string; seconds: number; prints: number } | null;
  voice_error: string | null;
  page_blocks: number;
}

export interface Voice {
  name: string;
  prints: number;
  seconds: number;
}

export interface PendingMeeting {
  id: string;
  started: string;
  title: string | null;
}

export const api = {
  getSettings: () => invoke<SettingsView>('get_settings'),
  /** A secret left undefined stays as it is; an empty one is removed. */
  saveSettings: (settings: Settings, brainyToken?: string, modelKey?: string) =>
    invoke<SettingsView>('save_settings', { settings, brainyToken, modelKey }),
  listDevices: () => invoke<Devices>('list_devices'),
  checkServer: (url: string) => invoke<void>('check_server', { url }),
  checkBrainy: () => invoke<void>('check_brainy'),
  startMeeting: (title: string) => invoke<RecorderState>('start_meeting', { title }),
  stopMeeting: () => invoke<void>('stop_meeting'),
  setNotes: (notes: string) => invoke<void>('set_notes', { notes }),
  meetingState: () => invoke<RecorderState | null>('meeting_state'),
  getLogs: () => invoke<LogLine[]>('get_logs'),
  listMeetings: () => invoke<MeetingSummary[]>('list_meetings'),
  getMeeting: (id: string) => invoke<MeetingView>('get_meeting', { id }),
  renameSpeaker: (id: string, from: string, to: string) =>
    invoke<{ renamed: Renamed; meeting: MeetingView }>('rename_speaker', { id, from, to }),
  writeReport: (id: string) => invoke<MeetingView>('write_report', { id }),
  saveMeetingNotes: (id: string, notes: string) =>
    invoke<void>('save_meeting_notes', { id, notes }),
  deleteMeeting: (id: string) => invoke<void>('delete_meeting', { id }),
  listPending: () => invoke<PendingMeeting[]>('list_pending'),
  finishPending: () => invoke<void>('finish_pending'),
  listVoices: () => invoke<Voice[]>('list_voices'),
  forgetVoice: (name: string) => invoke<void>('forget_voice', { name }),
};

export const events = {
  state: (f: (s: RecorderState | null) => void) =>
    listen<RecorderState | null>('meeting-state', (e) => f(e.payload)),
  utterance: (f: (u: LiveUtterance) => void) =>
    listen<LiveUtterance>('utterance', (e) => f(e.payload)),
  levels: (f: (l: number[]) => void) => listen<number[]>('levels', (e) => f(e.payload)),
  log: (f: (l: LogLine) => void) => listen<LogLine>('log', (e) => f(e.payload)),
  done: (f: (o: Outcome | null) => void) =>
    listen<Outcome | null>('meeting-done', (e) => f(e.payload)),
  failed: (f: (m: string) => void) => listen<string>('meeting-failed', (e) => f(e.payload)),
  meetingsChanged: (f: () => void) => listen('meetings-changed', () => f()),
};

/** Subscribes in an effect: the returned function unsubscribes, even before `listen` resolved. */
export function subscribe(...subscriptions: Promise<UnlistenFn>[]): () => void {
  return () => {
    for (const s of subscriptions) void s.then((off) => off());
  };
}

/** "12:04", "1:02:04". */
export function clock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s / 60) % 60);
  const pad = (n: number) => String(n).padStart(2, '0');
  return h > 0 ? `${h}:${pad(m)}:${pad(s % 60)}` : `${pad(m)}:${pad(s % 60)}`;
}

/** "42 min", "1 h 5 min". */
export function duration(seconds: number): string {
  const minutes = Math.round(seconds / 60);
  if (minutes < 1) return `${Math.round(seconds)} s`;
  if (minutes < 60) return `${minutes} min`;
  return `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

/** The label of a live utterance's speaker, as the page will have it until named. */
export function liveLabel(u: LiveUtterance, me: string): string {
  if (u.channel === 'mic') return u.speaker === null ? me : `Room ${u.speaker + 1}`;
  return u.speaker === null ? 'Call' : `Speaker ${u.speaker + 1}`;
}
