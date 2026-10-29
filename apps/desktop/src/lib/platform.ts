// The Android app (src-tauri/src/android.rs): the microphone only, sent to
// a server; no call to record, no models, no devices to choose. Its
// WebView names Android in its user agent.
export const android = /Android/i.test(navigator.userAgent);
