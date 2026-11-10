# A call is noticed by the microphone it opens

On macOS the app notices a call when a call app (Teams, Zoom, Webex,
Slack, Discord, WhatsApp) opens the microphone, and its end when the app
has let it go for 10 s. CoreAudio says which process has an input
running (`kAudioHardwarePropertyProcessObjectList`, then each process's
`kAudioProcessPropertyIsRunningInput` and `kAudioProcessPropertyBundleID`,
macOS 14.4). Off by default; the person chooses to be told, or to have it
recorded at once (`apps/logbook/src-tauri/src/calls.rs`, docs/app.md).

## Why not something else

- **Teams' local API** (Settings → Privacy → Third-party app API, a
  WebSocket on `localhost:8124` that sends `meetingUpdate` with
  `isInMeeting`): exact, but Teams only, turned on by hand and paired at
  first use, and made for devices like Stream Deck, with no promise it
  stays. Still the way to add later if the microphone proves too coarse.
- **The calendar** (Graph, Outlook): knows the meetings planned, not the
  calls made on the spot, nor whether a planned one started. At most a
  title for the meeting.
- **Starting without asking by default**: recording a call is the
  person's decision each time unless they chose otherwise in the
  settings; the notification always says it is being recorded.

## What it cannot see

- A call in a browser (Meet, Teams on the web): the microphone is the
  browser's, as for any page that uses it.
- An app that opens the microphone outside a call (Teams' pre-join
  screen with its level meter, a device test) is taken for a call: it is
  noticed a little early, and a join given up ends it 10 s later; a
  recording in which nothing was said shows nothing at its end.
- Before macOS 14.4 there are no process objects: nothing is noticed.

## Found on the way

- New Teams (`com.microsoft.teams2`) runs its audio in helpers:
  `com.microsoft.teams2.helper` and `com.microsoft.teams2.modulehost` are
  the processes CoreAudio lists, so apps are matched by the start of the
  bundle id. In a call it is `modulehost` that has the input (and the
  output) running; outside one, none of them has (checked on macOS 27).
