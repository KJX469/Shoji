//! Embeds the crane icon (rendered from vector data at build time) as the executable icon.

#[cfg(windows)]
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("shoji.ico");
    std::fs::write(
        &out,
        shoji_brand::ico(shoji_brand::Accent::default(), &shoji_brand::ICO_SIZES),
    )
    .expect("write icon");

    let mut res = winres::WindowsResource::new();
    res.set_icon(out.to_str().unwrap());
    res.set("FileDescription", "Shoji");
    res.set("ProductName", "Shoji");
    res.set("CompanyName", "Shoji Contributors");
    if let Some(sdk_bin) = find_windows_sdk_bin() {
        res.set_toolkit_path(&sdk_bin);
    }
    if let Err(e) = res.compile() {
        println!("cargo:warning=Failed to compile Windows resource: {e}");
    }

    // The installer embeds the browser binary; keep `cargo check` working before it's built.
    let payload = std::path::Path::new("assets").join("shoji.exe");
    println!("cargo:rerun-if-changed=assets/shoji.exe");
    if !payload.exists() {
        let _ = std::fs::create_dir_all("assets");
        let _ = std::fs::write(&payload, b"");
    }
}

/// Locate the newest Windows SDK `rc.exe` so builds work without a VS developer prompt.
#[cfg(windows)]
fn find_windows_sdk_bin() -> Option<String> {
    let prog_files = std::env::var("ProgramFiles(x86)")
        .or_else(|_| std::env::var("ProgramFiles"))
        .unwrap_or_else(|_| r"C:\Program Files (x86)".to_string());
    let base = std::path::PathBuf::from(prog_files).join(r"Windows Kits\10\bin");
    let mut dirs: Vec<_> = std::fs::read_dir(base)
        .ok()?
        .flatten()
        .map(|e| e.path().join("x64"))
        .filter(|p| p.join("rc.exe").exists())
        .collect();
    dirs.sort();
    dirs.pop().map(|p| p.to_string_lossy().into_owned())
}

#[cfg(not(windows))]
fn main() {}
