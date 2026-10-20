fn main() {
    // tauri-build embeds icons/icon.ico into the exe but does not watch it:
    // without this a changed icon keeps the old one until a clean build.
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
