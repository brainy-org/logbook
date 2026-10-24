//! libparakeet (parakeet.cpp's C API, `parakeet_capi.h`) loaded at run time.
//!
//! Only the calls the live pipeline makes are declared: load a model, Silero
//! VAD as a stream, batched transcription with word times, diarization as a
//! stream. Every string the library returns is copied and freed at once;
//! every failure becomes an error carrying the library's own message.

use std::ffi::{CStr, CString, c_char, c_float, c_int, c_void};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use libloading::Library;
use serde::{Deserialize, Serialize};

/// The ABI of the functions declared here (`parakeet_capi.h` of v0.6.1).
const ABI: c_int = 10;

pub const SAMPLE_RATE: u32 = 16_000;

const KIND_ASR: c_int = 1;
const KIND_DIARIZATION: c_int = 2;
const KIND_VAD: c_int = 5;

type Ctx = *mut c_void;

#[repr(C)]
struct DiarSegment {
    speaker: c_int,
    start: c_float,
    end: c_float,
}

struct Functions {
    abi_version: unsafe extern "C" fn() -> c_int,
    load: unsafe extern "C" fn(*const c_char) -> Ctx,
    load_error: unsafe extern "C" fn() -> *const c_char,
    free: unsafe extern "C" fn(Ctx),
    last_error: unsafe extern "C" fn(Ctx) -> *const c_char,
    free_string: unsafe extern "C" fn(*mut c_char),
    model_kind: unsafe extern "C" fn(Ctx) -> c_int,
    transcribe_pcm_batch_json:
        unsafe extern "C" fn(Ctx, *const c_float, *const c_int, c_int, c_int, c_int) -> *mut c_char,
    vad_stream_begin: unsafe extern "C" fn(Ctx, c_int, *const c_char) -> *mut c_void,
    vad_stream_feed_json:
        unsafe extern "C" fn(*mut c_void, *const c_float, c_int, c_int) -> *mut c_char,
    vad_stream_free: unsafe extern "C" fn(*mut c_void),
    diarize_stream_begin_latency: unsafe extern "C" fn(Ctx, c_int) -> *mut c_void,
    diarize_stream_time: unsafe extern "C" fn(*mut c_void) -> c_float,
    diarize_stream_active:
        unsafe extern "C" fn(*mut c_void, *mut *mut DiarSegment, *mut c_int) -> c_int,
    diarize_stream_feed: unsafe extern "C" fn(
        *mut c_void,
        *const c_float,
        c_int,
        c_int,
        *mut *mut DiarSegment,
        *mut c_int,
    ) -> c_int,
    free_diar_segments: unsafe extern "C" fn(*mut DiarSegment),
    diarize_stream_free: unsafe extern "C" fn(*mut c_void),
}

/// The loaded library. Models and streams keep it alive.
pub struct Parakeet {
    f: Functions,
    _library: Library,
}

impl Parakeet {
    /// Opens libparakeet at `path`, or by its file name on the loader's
    /// search path, and checks its ABI.
    pub fn open(path: Option<&Path>) -> Result<Arc<Self>> {
        let name = if cfg!(target_os = "macos") {
            "libparakeet.dylib"
        } else if cfg!(windows) {
            "parakeet.dll"
        } else {
            "libparakeet.so"
        };
        let target = path.map(Path::to_path_buf).unwrap_or_else(|| name.into());
        // SAFETY: loading a library runs its initializers; libparakeet's are ggml's.
        let library = unsafe { Library::new(&target) }
            .with_context(|| format!("could not open {}", target.display()))?;
        macro_rules! symbol {
            ($name:literal) => {
                // SAFETY: the types above are the ones of parakeet_capi.h.
                *unsafe { library.get(concat!("parakeet_capi_", $name)) }
                    .with_context(|| format!("libparakeet has no parakeet_capi_{}", $name))?
            };
        }
        let f = Functions {
            abi_version: symbol!("abi_version"),
            load: symbol!("load"),
            load_error: symbol!("load_error"),
            free: symbol!("free"),
            last_error: symbol!("last_error"),
            free_string: symbol!("free_string"),
            model_kind: symbol!("model_kind"),
            transcribe_pcm_batch_json: symbol!("transcribe_pcm_batch_json"),
            vad_stream_begin: symbol!("vad_stream_begin"),
            vad_stream_feed_json: symbol!("vad_stream_feed_json"),
            vad_stream_free: symbol!("vad_stream_free"),
            diarize_stream_begin_latency: symbol!("diarize_stream_begin_latency"),
            diarize_stream_time: symbol!("diarize_stream_time"),
            diarize_stream_active: symbol!("diarize_stream_active"),
            diarize_stream_feed: symbol!("diarize_stream_feed"),
            free_diar_segments: symbol!("free_diar_segments"),
            diarize_stream_free: symbol!("diarize_stream_free"),
        };
        let abi = unsafe { (f.abi_version)() };
        if abi != ABI {
            bail!("libparakeet has ABI {abi}, this build speaks {ABI}");
        }
        Ok(Arc::new(Self {
            f,
            _library: library,
        }))
    }

    /// Copies a string the library returned and frees it; None for NULL.
    fn take(&self, s: *mut c_char) -> Option<String> {
        if s.is_null() {
            return None;
        }
        let text = unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned();
        unsafe { (self.f.free_string)(s) };
        Some(text)
    }
}

fn borrowed(s: *const c_char) -> String {
    if s.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned()
}

/// What a model file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Asr,
    Diarization,
    Vad,
}

/// A loaded model. ASR and VAD contexts may be called from several threads;
/// a diarization context may not, so its streams take `serial` for each call.
pub struct Model {
    lib: Arc<Parakeet>,
    ctx: Ctx,
    pub kind: Kind,
    serial: Mutex<()>,
}

// SAFETY: the library is thread safe on ASR and VAD contexts (parakeet_capi.h,
// "Concurrent requests" and "Standalone voice-activity detection"); the one
// rule it does not enforce, one thread at a time on a diarization context, is
// kept by `serial`.
unsafe impl Send for Model {}
unsafe impl Sync for Model {}

impl Model {
    pub fn load(lib: &Arc<Parakeet>, path: &Path, expected: Kind) -> Result<Arc<Self>> {
        let c_path = CString::new(path.to_string_lossy().as_bytes())?;
        let ctx = unsafe { (lib.f.load)(c_path.as_ptr()) };
        if ctx.is_null() {
            let reason = borrowed(unsafe { (lib.f.load_error)() });
            bail!("could not load {}: {reason}", path.display());
        }
        let kind = match unsafe { (lib.f.model_kind)(ctx) } {
            KIND_ASR => Some(Kind::Asr),
            KIND_DIARIZATION => Some(Kind::Diarization),
            KIND_VAD => Some(Kind::Vad),
            _ => None,
        };
        let model = Self {
            lib: lib.clone(),
            ctx,
            kind: kind.unwrap_or(expected),
            serial: Mutex::new(()),
        };
        if kind != Some(expected) {
            bail!("{} is not a {expected:?} model", path.display());
        }
        Ok(Arc::new(model))
    }

    fn error(&self) -> String {
        borrowed(unsafe { (self.lib.f.last_error)(self.ctx) })
    }

    /// Transcribes 16 kHz mono clips in one call. Word times are relative
    /// to the start of each clip.
    pub fn transcribe(&self, clips: &[&[f32]]) -> Result<Vec<Transcript>> {
        if clips.is_empty() {
            return Ok(Vec::new());
        }
        let lengths: Vec<c_int> = clips.iter().map(|c| c.len() as c_int).collect();
        let samples: Vec<f32> = clips.concat();
        let out = unsafe {
            (self.lib.f.transcribe_pcm_batch_json)(
                self.ctx,
                samples.as_ptr(),
                lengths.as_ptr(),
                clips.len() as c_int,
                SAMPLE_RATE as c_int,
                0,
            )
        };
        let json = self
            .lib
            .take(out)
            .with_context(|| format!("transcription failed: {}", self.error()))?;
        Ok(serde_json::from_str(&json)?)
    }

    /// A Silero VAD stream over 16 kHz audio; `options` as in
    /// `parakeet_capi_vad_pcm_json` (mode "speech").
    pub fn vad_stream(self: &Arc<Self>, options: &str) -> Result<VadStream> {
        let c_options = CString::new(options)?;
        let s = unsafe {
            (self.lib.f.vad_stream_begin)(self.ctx, SAMPLE_RATE as c_int, c_options.as_ptr())
        };
        if s.is_null() {
            bail!("VAD stream: {}", self.error());
        }
        Ok(VadStream {
            model: self.clone(),
            s,
        })
    }

    /// A diarization stream in one of the model card's latency modes.
    pub fn diar_stream(self: &Arc<Self>, latency: Latency) -> Result<DiarStream> {
        let _turn = self.serial.lock().unwrap();
        let s = unsafe { (self.lib.f.diarize_stream_begin_latency)(self.ctx, latency as c_int) };
        if s.is_null() {
            bail!("diarization stream: {}", self.error());
        }
        Ok(DiarStream {
            model: self.clone(),
            s,
        })
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        unsafe { (self.lib.f.free)(self.ctx) };
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Transcript {
    pub text: String,
    #[serde(default)]
    pub words: Vec<Word>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Word {
    pub w: String,
    pub start: f64,
    pub end: f64,
    pub conf: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VadEdge {
    Start,
    End,
}

#[derive(Clone, Copy, Debug)]
pub struct VadEvent {
    pub edge: VadEdge,
    /// Seconds from the start of the stream.
    pub time: f64,
}

pub struct VadStream {
    model: Arc<Model>,
    s: *mut c_void,
}

// SAFETY: one stream is used by one thread at a time (it is owned); the
// library allows different streams on different threads.
unsafe impl Send for VadStream {}

impl VadStream {
    /// Speech starts and ends that became known with this audio; an event can
    /// lie before it (an end is known once the pause has lasted min_pause).
    pub fn feed(&mut self, pcm: &[f32], is_last: bool) -> Result<Vec<VadEvent>> {
        #[derive(Deserialize)]
        struct Doc {
            events: Vec<Event>,
        }
        #[derive(Deserialize)]
        struct Event {
            r#type: String,
            time: f64,
        }
        let f = &self.model.lib.f;
        let out = unsafe {
            (f.vad_stream_feed_json)(self.s, pcm.as_ptr(), pcm.len() as c_int, is_last as c_int)
        };
        let json = self
            .model
            .lib
            .take(out)
            .with_context(|| format!("VAD: {}", self.model.error()))?;
        let doc: Doc = serde_json::from_str(&json)?;
        Ok(doc
            .events
            .into_iter()
            .map(|e| VadEvent {
                edge: if e.r#type == "start" {
                    VadEdge::Start
                } else {
                    VadEdge::End
                },
                time: e.time,
            })
            .collect())
    }
}

impl Drop for VadStream {
    fn drop(&mut self) {
        unsafe { (self.model.lib.f.vad_stream_free)(self.s) };
    }
}

/// Diarization latency modes (`PARAKEET_DIAR_LATENCY_*`): the audio buffered
/// before a chunk runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Latency {
    /// The checkpoint's own chunks, 21.12 s: not for live use.
    Model = 0,
    /// 1.04 s
    Low = 1,
    /// 0.64 s
    VeryLow = 2,
    /// 0.32 s
    UltraLow = 3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Turn {
    pub speaker: u32,
    pub start: f64,
    pub end: f64,
}

pub struct DiarStream {
    model: Arc<Model>,
    s: *mut c_void,
}

// SAFETY: owned by one thread; calls on the shared context are serialized by
// the model's `serial`.
unsafe impl Send for DiarStream {}

impl DiarStream {
    /// The turns that ended with this audio.
    pub fn feed(&mut self, pcm: &[f32], is_last: bool) -> Result<Vec<Turn>> {
        let f = &self.model.lib.f;
        let _turn = self.model.serial.lock().unwrap();
        let mut out = std::ptr::null_mut();
        let mut n = 0;
        let status = unsafe {
            (f.diarize_stream_feed)(
                self.s,
                pcm.as_ptr(),
                pcm.len() as c_int,
                is_last as c_int,
                &mut out,
                &mut n,
            )
        };
        if status != 0 {
            bail!("diarization: {}", self.model.error());
        }
        Ok(self.segments(out, n))
    }

    /// The turns still open at the diarized time, ending at it.
    pub fn active(&mut self) -> Result<Vec<Turn>> {
        let f = &self.model.lib.f;
        let _turn = self.model.serial.lock().unwrap();
        let mut out = std::ptr::null_mut();
        let mut n = 0;
        if unsafe { (f.diarize_stream_active)(self.s, &mut out, &mut n) } != 0 {
            bail!("diarization: {}", self.model.error());
        }
        Ok(self.segments(out, n))
    }

    /// Seconds of audio diarized so far: trails the audio fed by up to the latency.
    pub fn time(&self) -> f64 {
        let _turn = self.model.serial.lock().unwrap();
        unsafe { (self.model.lib.f.diarize_stream_time)(self.s) as f64 }
    }

    fn segments(&self, out: *mut DiarSegment, n: c_int) -> Vec<Turn> {
        if out.is_null() || n <= 0 {
            return Vec::new();
        }
        let turns = unsafe { std::slice::from_raw_parts(out, n as usize) }
            .iter()
            .map(|s| Turn {
                speaker: s.speaker.max(0) as u32,
                start: s.start as f64,
                end: s.end as f64,
            })
            .collect();
        unsafe { (self.model.lib.f.free_diar_segments)(out) };
        turns
    }
}

impl Drop for DiarStream {
    fn drop(&mut self) {
        let _turn = self.model.serial.lock().unwrap();
        unsafe { (self.model.lib.f.diarize_stream_free)(self.s) };
    }
}
