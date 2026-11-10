// The Android app (src-tauri/src/android.rs): the microphone only, sent to
// a server; no call to record, no models, no devices to choose. Its
// WebView names Android in its user agent.
export const android = /Android/i.test(navigator.userAgent);

// macOS: the only one that notices calls (src-tauri calls.rs).
export const macos = /Macintosh/i.test(navigator.userAgent);
