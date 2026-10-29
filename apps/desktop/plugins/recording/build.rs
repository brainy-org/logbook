// No command for the pages: the app's Rust calls the Kotlin side
// (`android/`) through `run_mobile_plugin`.
const COMMANDS: &[&str] = &[];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .build();
}
