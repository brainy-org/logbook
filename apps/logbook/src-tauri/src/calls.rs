//! Calls noticed as they start, on macOS: a call app (Teams, Zoom…) that
//! opens the microphone is a call starting, and one that lets it go for
//! `ENDED_AFTER` is its end. CoreAudio says which process has an input
//! running (`kAudioHardwarePropertyProcessObjectList`, macOS 14.4): the
//! same family as the call's tap, so no other permission.
//!
//! With the settings' `calls.notice` the person is told (a notification,
//! and `call` to the pages, whose notice has Record); with `calls.record`
//! too the recording starts at once. A recording started for a call
//! (`tied`) stops when the call ends; one started by hand never does.
//!
//! A call in a browser (Meet, Teams on the web) is not noticed: its
//! microphone is the browser's, as for any page.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

/// The call apps, by the start of their processes' bundle ids (their
/// helpers hold the microphone: `com.microsoft.teams2.helper`).
#[cfg(target_os = "macos")]
const APPS: &[(&str, &str)] = &[
    ("com.microsoft.teams", "Teams"),
    ("us.zoom.", "Zoom"),
    ("com.cisco.webex", "Webex"),
    ("Cisco-Systems.Spark", "Webex"),
    ("com.tinyspeck.slackmacgap", "Slack"),
    ("com.hnc.Discord", "Discord"),
    ("net.whatsapp.WhatsApp", "WhatsApp"),
];

/// How often CoreAudio is asked: a few dozen property reads.
#[cfg(target_os = "macos")]
const EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// How long the microphone stays free before the call is over: an app
/// opens it again when a device changes.
#[cfg(target_os = "macos")]
const ENDED_AFTER: std::time::Duration = std::time::Duration::from_secs(10);

/// What the pages are told of the call going on. Elsewhere than macOS
/// no call is ever noticed.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Call {
    /// "Teams".
    pub app: &'static str,
    /// Being recorded since it started (`calls.record`).
    pub recording: bool,
}

#[derive(Default)]
pub struct Calls {
    current: Mutex<Option<Call>>,
    /// The recording going on was started for the call: it stops with it.
    tied: AtomicBool,
}

impl Calls {
    pub fn current(&self) -> Option<Call> {
        self.current.lock().unwrap().clone()
    }

    /// The recording just started is the call's.
    pub fn tie(&self) {
        self.tied.store(true, Ordering::SeqCst);
    }
}

/// Watches for calls for as long as the app runs, on a thread of its own.
#[cfg(target_os = "macos")]
pub fn watch(handle: tauri::AppHandle) {
    use std::time::Instant;
    use tauri::{Emitter, Manager};

    std::thread::spawn(move || {
        let mut quiet_since: Option<Instant> = None;
        loop {
            std::thread::sleep(EVERY);
            let app = handle.state::<crate::App>();
            // Its recording over (stopped by hand, or ended), the next is not the call's.
            if app.recorder.state().is_none() {
                app.calls.tied.store(false, Ordering::SeqCst);
            }
            let holding = coreaudio::input_apps()
                .into_iter()
                .find_map(|bundle| APPS.iter().find(|(id, _)| bundle.starts_with(id)))
                .map(|(_, name)| *name);
            let going = app.calls.current().is_some();
            match holding {
                Some(name) => {
                    quiet_since = None;
                    if !going {
                        started(&handle, &app, name);
                    }
                }
                None if going => {
                    let since = *quiet_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= ENDED_AFTER {
                        quiet_since = None;
                        *app.calls.current.lock().unwrap() = None;
                        tracing::info!("the call has ended");
                        if app.calls.tied.swap(false, Ordering::SeqCst) {
                            app.recorder.stop(&handle);
                        }
                        let _ = handle.emit("call", None::<Call>);
                    }
                }
                None => {}
            }
        }
    });

    fn started(handle: &tauri::AppHandle, app: &crate::App, name: &'static str) {
        use tauri_plugin_notification::NotificationExt;

        tracing::info!("a {name} call has started");
        let settings = app.settings();
        let mut call = Call {
            app: name,
            recording: false,
        };
        // Noticed even when off: turned on during the call, it is not news.
        let busy = app.recorder.state().is_some();
        if settings.calls.notice && !busy && settings.calls.record {
            match crate::record_call(app, handle.clone()) {
                Ok(_) => call.recording = true,
                Err(error) => tracing::warn!("the call could not be recorded: {error}"),
            }
        }
        *app.calls.current.lock().unwrap() = Some(call.clone());
        if !settings.calls.notice || busy {
            return;
        }
        let (title, body) = if call.recording {
            (
                format!("Logbook is recording the {name} call"),
                "Its transcript, with speakers, is in Logbook's window. \
                 It stops on its own when the call ends."
                    .to_string(),
            )
        } else {
            (
                format!("{name} call started"),
                "Logbook can transcribe it: press Record in its window. \
                 It stops on its own when the call ends."
                    .to_string(),
            )
        };
        if let Err(error) = handle
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
        {
            tracing::warn!("could not show the call's notification: {error}");
        }
        let _ = handle.emit("call", Some(call));
    }
}

/// CoreAudio's process objects (macOS 14.4), declared from
/// `AudioHardware.h`, `AudioHardwareBase.h` and `CFString.h`.
#[cfg(target_os = "macos")]
mod coreaudio {
    use std::ffi::{c_char, c_void};

    #[repr(C)]
    struct Address {
        selector: u32,
        scope: u32,
        element: u32,
    }

    const SYSTEM_OBJECT: u32 = 1;
    const SCOPE_GLOBAL: u32 = u32::from_be_bytes(*b"glob");
    const ELEMENT_MAIN: u32 = 0;
    const PROCESS_OBJECT_LIST: u32 = u32::from_be_bytes(*b"prs#");
    const PROCESS_BUNDLE_ID: u32 = u32::from_be_bytes(*b"pbid");
    const PROCESS_IS_RUNNING_INPUT: u32 = u32::from_be_bytes(*b"piri");
    const UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreAudio", kind = "framework")]
    unsafe extern "C" {
        fn AudioObjectGetPropertyDataSize(
            object: u32,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
        ) -> i32;
        fn AudioObjectGetPropertyData(
            object: u32,
            address: *const Address,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
            data: *mut c_void,
        ) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringGetCString(
            string: *const c_void,
            buffer: *mut c_char,
            size: isize,
            encoding: u32,
        ) -> u8;
        fn CFRelease(object: *const c_void);
    }

    fn address(selector: u32) -> Address {
        Address {
            selector,
            scope: SCOPE_GLOBAL,
            element: ELEMENT_MAIN,
        }
    }

    /// One property of a fixed size; None if CoreAudio refuses it.
    fn get<T: Default>(object: u32, selector: u32) -> Option<T> {
        let mut value = T::default();
        let mut size = std::mem::size_of::<T>() as u32;
        // SAFETY: `value` is `size` bytes, the property's own size.
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                &address(selector),
                0,
                std::ptr::null(),
                &mut size,
                (&mut value as *mut T).cast(),
            )
        };
        (status == 0 && size as usize == std::mem::size_of::<T>()).then_some(value)
    }

    /// The bundle ids of the processes with an input running; none
    /// before macOS 14.4, which has no process objects.
    pub fn input_apps() -> Vec<String> {
        let list = address(PROCESS_OBJECT_LIST);
        let mut size = 0u32;
        // SAFETY: the address is valid; `size` receives the list's bytes.
        let status = unsafe {
            AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &list, 0, std::ptr::null(), &mut size)
        };
        if status != 0 || size == 0 {
            return Vec::new();
        }
        let mut ids = vec![0u32; size as usize / 4];
        // SAFETY: `ids` holds `size` bytes; CoreAudio writes at most that.
        let status = unsafe {
            AudioObjectGetPropertyData(
                SYSTEM_OBJECT,
                &list,
                0,
                std::ptr::null(),
                &mut size,
                ids.as_mut_ptr().cast(),
            )
        };
        if status != 0 {
            return Vec::new();
        }
        ids.truncate(size as usize / 4);
        ids.into_iter()
            .filter(|&id| get::<u32>(id, PROCESS_IS_RUNNING_INPUT) == Some(1))
            .filter_map(bundle_id)
            .collect()
    }

    fn bundle_id(process: u32) -> Option<String> {
        let string: usize = get(process, PROCESS_BUNDLE_ID)?;
        let string = string as *const c_void;
        if string.is_null() {
            return None;
        }
        let mut buffer = [0 as c_char; 256];
        // SAFETY: a CFString the caller owns (the header says so), released here.
        let ok = unsafe {
            let ok = CFStringGetCString(string, buffer.as_mut_ptr(), buffer.len() as isize, UTF8);
            CFRelease(string);
            ok
        };
        if ok == 0 {
            return None;
        }
        // SAFETY: CFStringGetCString wrote a NUL-terminated string.
        let id = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) };
        Some(id.to_string_lossy().into_owned())
    }
}
