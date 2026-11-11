# The desktop app ends on Windows without libparakeet's teardown

**Verdict.** On Windows the desktop app ends its process with
`TerminateProcess` (`parakeet::exit`, engine `parakeet.rs`, called on
Tauri's `RunEvent::Exit` in `lib.rs`), not with `std::process::exit`.
Closed after a meeting, the app crashed with 0xc0000409 inside Intel's
Vulkan driver while libparakeet's static destructors ran. Nothing is lost
by skipping them: the meeting is written before the window can close,
and one closed while recording stays in `pending/` by design. Since
2026-11-11 macOS and Linux end with `_exit` for the same reason (below).

## What was seen

Windows 11 over Remote Desktop, Intel Iris Xe, the `win-vulkan-x64`
libparakeet. Closing the window right after start exited cleanly; closing
it after a meeting (recorded, final pass, report) ended with
`STATUS_STACK_BUFFER_OVERRUN`, three times out of three, and no Rust panic
in the log. The Application event log held no faulting module.

Under `cdb`, the main thread's stack at the fast fail (subcode 7,
`FAST_FAIL_FATAL_APP_EXIT`):

```
igvk64!...                         C++ exception (e06d7363) thrown in the driver
parakeet!pk::Backend::~Backend
ucrtbase!execute_onexit_table      parakeet.dll's static destructors
ntdll!LdrShutdownProcess           DLL_PROCESS_DETACH
KERNEL32!ExitProcessImplementation
std::process::exit
tao::...::EventLoop::run           Tauri's event loop, the window closed
```

`ExitProcess` first stops every other thread, then unloads each DLL. ggml's
global Vulkan backend, a static in libparakeet, tears its device down
there, and the driver throws from the teardown.

## Not reproduced outside the app

A probe that loads the same library and models, runs a transcription, a
diarization, a voice embedding and VAD and diarization streams (on the
main thread, or on threads left parked as the batcher's), then calls
`process::exit`, returned 0 every time. The app adds two diarization
contexts, WebView2 on the same GPU, and cpal's streams; which of them
leaves the driver in the state that throws was not chased: the stack
shows the teardown itself is the failing step, and the fix removes it.

## Why not free the models first

Freeing every `Model` before exit does not reach `pk::Backend`: it is a
static of the library, destroyed by the CRT at unload whatever the
contexts. Pinning the DLL does not help either: a pinned module still
gets `DLL_PROCESS_DETACH` at process exit. `TerminateProcess` is the one
exit that runs no detach routine; the OS reclaims the device memory as it
does for any process.

## Scope

Only the desktop app calls `parakeet::exit`. The CLI ended cleanly
(`logbook finish`, exit code 0) after a final pass on the same machine,
and the server's image is Linux. Either one that crashes the same way at
exit calls it at the end of `main`.

## macOS, 2026-11-11

The same teardown crashed on macOS: the window closed with its red button
while a meeting ran locally ended the app, and `exit` ran libparakeet's
static destructors (`pk::Backend::~Backend`, `ggml_metal_free` on the
main thread) while a session thread was in `SileroVad::run_chunk`: SIGSEGV
at address 0 in `pk::Backend::compute`. `parakeet::exit` now ends every
Unix process with `_exit`, which runs no atexit handler nor destructor.
The red button on macOS no longer quits at all: it hides the window, as a
Mac app does, so a recording and the calls' watch go on.
