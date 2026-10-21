//! Compiles the Google OAuth client into the app (`docs/architecture.md` §8).
//!
//! The id and secret come from the environment (CI sets them from repository secrets) or, for a
//! local build, from the untracked `.env` at the repository root. Without them the app still
//! builds, and Google sign-in asks for the user's own client id.

/// The variables `option_env!` reads in `src/lib.rs`.
const KEYS: [&str; 2] = [
    "SIMPLECALENDAR_GOOGLE_CLIENT_ID",
    "SIMPLECALENDAR_GOOGLE_CLIENT_SECRET",
];

fn main() {
    for key in KEYS {
        println!("cargo:rerun-if-env-changed={key}");
    }
    // Only watched when it exists: Cargo reruns a build script on every build for a watched
    // path that is missing. After creating `.env`, run `touch src-tauri/build.rs` once.
    let env_file = std::path::Path::new("../.env");
    if let Ok(contents) = std::fs::read_to_string(env_file) {
        println!("cargo:rerun-if-changed=../.env");
        for line in contents.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim().trim_matches('"'));
            // A variable set in the environment wins over the file.
            if KEYS.contains(&key) && !value.is_empty() && std::env::var_os(key).is_none() {
                println!("cargo:rustc-env={key}={value}");
            }
        }
    }
    tauri_build::build()
}
