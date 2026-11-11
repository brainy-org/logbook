// The app's commands (src-tauri/src/lib.rs) and events, typed.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type Transcription = 'local' | 'server';

/** Alone on a call, several people in the room, or both. */
export type Mode = 'call' | 'room' | 'roomCall';

export interface Settings {
  transcription: Transcription;
  serverUrl: string;
  me: string;
  audio: {
    mode: Mode;
    micDevice: string | null;
    systemDevice: string | null;
    headphones: boolean;
    micGainAuto: boolean;
  };
  /** Calls noticed as they start (macOS): told, and recorded at once with `record`. */
  calls: { notice: boolean; record: boolean };
  brainy: { enabled: boolean; url: string; parent: string };
  model: { report: boolean; url: string; name: string };
}

export interface SettingsView {
  settings: Settings;
  secrets: { brainyToken: boolean; modelKey: boolean; serverToken: boolean };
  data: string;
}

export interface Devices {
  inputs: string[];
  outputs: string[];
}

/** A call app holding the microphone (src-tauri calls.rs). */
export interface Call {
  /** "Teams". */
  app: string;
  /** Recorded since it started (`calls.record`). */
  recording: boolean;
  /** macOS refused its notification: off for Logbook in the system's settings. */
  notificationsOff: boolean;
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
  /** Which meeting of the recording, from 0: a recording left running is cut into meetings. */
  part: number;
  /** Once it has stopped: where finishing its meeting is. */
  step: Step | null;
}

/** Where a meeting being finished is (client meeting.rs, `Step`); `done` from 0 to 1. */
export type Step =
  | { stage: 'sending'; done: number; audioS: number }
  | { stage: 'queued' }
  | { stage: 'reading'; done: number; audioS: number }
  | { stage: 'transcribing'; done: number | null; audioS: number }
  | { stage: 'publishing' }
  | { stage: 'report' };

/** Why finishing a meeting stopped (client finishing.rs, `Problem`). */
export type ProblemKind = 'unreachable' | 'busy' | 'token' | 'refused' | 'failed';

/** A pending meeting being finished, or stopped (src-tauri finisher.rs). */
export interface Finishing {
  id: string;
  active: boolean;
  /** Being recorded, or finished by `logbook finish`. */
  busy: boolean;
  step: Step | null;
  /** When the step started, ms since the epoch. */
  since: number;
  problem: {
    kind: ProblemKind;
    who: 'server' | 'brainy';
    message: string;
    /** Tried again on its own. */
    retried: boolean;
  } | null;
  /** When it is tried again, ms since the epoch. */
  retryAt: number | null;
}

/** How the models of a local backend get ready; null with a server. */
export type ModelsStatus =
  | { state: 'fetching'; done: number; total: number }
  | { state: 'loading' }
  | { state: 'ready' }
  | { state: 'failed'; error: string };

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
  turns: {
    label: string;
    start: number;
    phrases: { start: number; end: number; text: string }[];
  }[];
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
  /** Not saved because it may not be theirs; `saveVoice` saves it anyway. */
  voice_doubt: string | null;
  page_blocks: number;
}

/** A person known by voice; their voiceprints never leave the Rust side. */
export interface Voice {
  name: string;
  /** Seconds of their voice, all meetings together. */
  seconds: number;
  /** The meetings it comes from (the latest 5 at most). */
  meetings: number;
  /** The latest meeting they were heard in, recognized or named: `YYYYMMDD-HHMMSS`. */
  heard: string;
  /** No meeting gave 30 s of their voice: they may not be recognized every time. */
  weak: boolean;
}

export interface PendingMeeting {
  id: string;
  started: string;
  title: string | null;
  /** Seconds of its audio. */
  audioS: number;
  finishing: Finishing | null;
}

/** A screenshot taken: its file, and the caption of its line in the notes. */
export interface Shot {
  name: string;
  caption: string;
}

export const api = {
  getSettings: () => invoke<SettingsView>('get_settings'),
  /** A secret left undefined stays as it is; an empty one is removed. */
  saveSettings: (
    settings: Settings,
    brainyToken?: string,
    modelKey?: string,
    serverToken?: string,
  ) => invoke<SettingsView>('save_settings', { settings, brainyToken, modelKey, serverToken }),
  modelsState: () => invoke<ModelsStatus | null>('models_state'),
  /** Fetches and loads the models again, after they failed to. */
  loadModels: () => invoke<void>('load_models'),
  listDevices: () => invoke<Devices>('list_devices'),
  checkServer: (url: string, token?: string) => invoke<void>('check_server', { url, token }),
  checkBrainy: () => invoke<void>('check_brainy'),
  /** The text models of an OpenAI-compatible API; `key` undefined: the one kept. */
  listModels: (url: string, key?: string) => invoke<string[]>('list_models', { url, key }),
  /** `mode` is kept in the settings for the next meeting. */
  /** `expected`: the voices known by name the speakers are named from; none, every one. */
  startMeeting: (title: string, mode: Mode, expected: string[]) =>
    invoke<RecorderState>('start_meeting', { title, mode, expected }),
  stopMeeting: () => invoke<void>('stop_meeting'),
  /** The call going on, if one was noticed (macOS). */
  callState: () => invoke<Call | null>('call_state'),
  /** The system's Notifications settings at Logbook's page (macOS). */
  openNotificationSettings: () => invoke<void>('open_notification_settings'),
  /** Records the call going on; it stops when the call ends. */
  startCallMeeting: () => invoke<RecorderState>('start_call_meeting'),
  /** The notes of meeting `part` of the recording (`RecorderState.part`). */
  setNotes: (part: number, notes: string) => invoke<void>('set_notes', { part, notes }),
  /** The recording's title from meeting `part` on; empty, the default one. */
  setTitle: (part: number, title: string) => invoke<void>('set_title', { part, title }),
  /** The screen under the pointer, or an area (macOS), for the notes; null if given up. */
  takeScreenshot: (kind: 'screen' | 'area') => invoke<Shot | null>('take_screenshot', { kind }),
  meetingState: () => invoke<RecorderState | null>('meeting_state'),
  getLogs: () => invoke<LogLine[]>('get_logs'),
  listMeetings: () => invoke<MeetingSummary[]>('list_meetings'),
  getMeeting: (id: string) => invoke<MeetingView>('get_meeting', { id }),
  renameSpeaker: (id: string, from: string, to: string) =>
    invoke<{ renamed: Renamed; meeting: MeetingView }>('rename_speaker', { id, from, to }),
  /** The voice of speaker `name` saved under that name although it was doubted. */
  saveVoice: (id: string, name: string) =>
    invoke<{ name: string; seconds: number; prints: number }>('save_voice', { id, name }),
  /** The names a speaker could be, likeliest first, from the voices known by name. */
  speakerCandidates: (id: string, label: string) =>
    invoke<string[]>('speaker_candidates', { id, label }),
  writeReport: (id: string) => invoke<MeetingView>('write_report', { id }),
  /** A meeting published without brainy, written to a new page. */
  sendToBrainy: (id: string) => invoke<MeetingView>('send_to_brainy', { id }),
  saveMeetingNotes: (id: string, notes: string) =>
    invoke<void>('save_meeting_notes', { id, notes }),
  /** Into the trash: `restoreMeeting` undoes it, `purgeMeeting` makes it final. */
  deleteMeeting: (id: string) => invoke<void>('delete_meeting', { id }),
  restoreMeeting: (id: string) => invoke<void>('restore_meeting', { id }),
  purgeMeeting: (id: string) => invoke<void>('purge_meeting', { id }),
  listPending: () => invoke<PendingMeeting[]>('list_pending'),
  finishPending: () => invoke<void>('finish_pending'),
  listVoices: () => invoke<Voice[]>('list_voices'),
  /** Refused when someone else already has the name. */
  renameVoice: (from: string, to: string) => invoke<void>('rename_voice', { from, to }),
  forgetVoice: (name: string) => invoke<void>('forget_voice', { name }),
  /** Android: whether battery optimization leaves the app alone (true elsewhere). */
  backgroundState: () => invoke<boolean>('background_state'),
  /** Android: the system's dialog that exempts the app from battery optimization. */
  askBackground: () => invoke<boolean>('ask_background'),
};

export const events = {
  state: (f: (s: RecorderState | null) => void) =>
    listen<RecorderState | null>('meeting-state', (e) => f(e.payload)),
  utterance: (f: (u: LiveUtterance) => void) =>
    listen<LiveUtterance>('utterance', (e) => f(e.payload)),
  models: (f: (s: ModelsStatus | null) => void) =>
    listen<ModelsStatus | null>('models', (e) => f(e.payload)),
  levels: (f: (l: number[]) => void) => listen<number[]>('levels', (e) => f(e.payload)),
  log: (f: (l: LogLine) => void) => listen<LogLine>('log', (e) => f(e.payload)),
  done: (f: (o: Outcome | null) => void) =>
    listen<Outcome | null>('meeting-done', (e) => f(e.payload)),
  failed: (f: (m: string) => void) => listen<string>('meeting-failed', (e) => f(e.payload)),
  meetingsChanged: (f: () => void) => listen('meetings-changed', () => f()),
  /** A call noticed as it starts, or null at its end (only when the settings ask). */
  call: (f: (c: Call | null) => void) => listen<Call | null>('call', (e) => f(e.payload)),
  /** A pending meeting's state as it is finished. */
  finishing: (f: (m: Finishing) => void) => listen<Finishing>('finishing', (e) => f(e.payload)),
  /** A screenshot taken by the shortcut while a meeting is recorded. */
  screenshot: (f: (s: Shot) => void) => listen<Shot>('screenshot', (e) => f(e.payload)),
  screenshotFailed: (f: (m: string) => void) =>
    listen<string>('screenshot-failed', (e) => f(e.payload)),
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

/** "680 MB", "1.2 GB". */
export function size(bytes: number): string {
  const mb = bytes / 1e6;
  return mb < 1000 ? `${Math.round(mb)} MB` : `${(mb / 1000).toFixed(1)} GB`;
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
