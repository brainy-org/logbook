//! The model files' pins: what the mirror serves (mirror.rs fetches them,
//! with the `models` feature) and what the client names a voice library
//! by (client voices.rs), which needs them without the models: the
//! Android app links no model code (docs/decisions/2026-10-29-android-app.md).

/// Model files: name, sha256, size in bytes, bundle. The `parakeet` bundle
/// is the one brainy-worker loads; `parakeet-voice` the speaker encoder,
/// logbook's alone. `MODELS_DIR` names the folder they go in. The sizes are
/// the mirror's (`models.lock.json`): what a download's progress counts to.
pub const MODELS: [(&str, &str, u64, &str); 4] = [
    (
        "tdt-0.6b-v3-q8_0.gguf",
        "4d69a4a6683f4f2d952bad794c1357ca6eb628027695b4699c5a9ad4cd07d757",
        940663680,
        "parakeet",
    ),
    (
        "silero-vad-f16.gguf",
        "8160489282352accc0e95925c2f6bf3d76fb8f7bccdce5c7e46808cb15e8443c",
        1264928,
        "parakeet",
    ),
    (
        "nemotron-3-diarization-q8_0.gguf",
        "76c5bb1fb20d82706142ad32769b7ab496d2458489473a000fd7074c52ceec22",
        108674624,
        "parakeet",
    ),
    (
        "wespeaker-resnet34-lm.gguf",
        "72040372494eafec299836bc1977cfc13c603cb486674ed59b0f4c03758d29da",
        26527360,
        "parakeet-voice",
    ),
];
/// The `parakeet` bundle's files are mudler/parakeet-cpp-gguf at commit
/// 741158ae; under the cache folder.
pub const MODELS_DIR: &str = "parakeet-741158ae";
