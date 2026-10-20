//! Mirrors the JS build, which inlines `.env` into the bundle: expose the Discord
//! variables in `.env` (if present) as compile-time env vars so a shipped binary
//! can fall back to them via `option_env!`.

fn main() {
    println!("cargo:rerun-if-changed=.env");
    let Ok(text) = std::fs::read_to_string(".env") else { return };
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
            if matches!(k.trim(), "DISCORD_CLIENT_ID" | "DISCORD_CLIENT_SECRET") {
                println!("cargo:rustc-env={}={}", k.trim(), v);
            }
        }
    }
}
