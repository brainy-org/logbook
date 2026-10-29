// Screenshots in the notes (crates/client shots.rs): a line that is only
// `![caption](name.png)` names a file the app keeps, shown through its
// `shot` scheme (src-tauri/src/screenshot.rs). `folder` is where the file
// is: `screenshots` while the meeting is recorded, `meetings/<id>` after.

const mac = /Mac/i.test(navigator.userAgent);

/** Take the screen, or an area, while a meeting is recorded, from any app. */
export const SCREEN_KEYS = mac ? '⌘⇧2' : 'Ctrl+Shift+2';
export const AREA_KEYS = '⌘⇧1';
/** Selecting an area is macOS's own tool (src-tauri/src/screenshot.rs). */
export const AREA = mac;

/** A file name the app could keep: nothing that walks out of its folder. */
export function isShot(name: string): boolean {
  return /^[A-Za-z0-9_-][A-Za-z0-9_.-]*\.png$/.test(name);
}

/** Where the page loads a screenshot from: the scheme's address on this platform. */
export function shotUrl(folder: string, name: string): string {
  const path = `${folder}/${encodeURIComponent(name)}`;
  return /Windows/i.test(navigator.userAgent)
    ? `http://shot.localhost/${path}`
    : `shot://localhost/${path}`;
}
