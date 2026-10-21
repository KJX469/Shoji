//! Embeds the crane icon (rendered from vector data at build time) as the executable icon.

/// A short id for this build ("5cd3acc", or "5cd3acc+" with uncommitted changes), shown in
/// Settings so it is clear which build is running.
fn build_id() -> String {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let Some(hash) = git(&["rev-parse", "--short", "HEAD"]) else {
        return String::new();
    };
    let dirty = git(&["status", "--porcelain", "--", "crates"]).is_some_and(|s| !s.is_empty());
    format!("{hash}{}", if dirty { "+" } else { "" })
}

#[cfg(windows)]
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rustc-env=SHOJI_BUILD={}", build_id());
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
fn main() {
    println!("cargo:rustc-env=SHOJI_BUILD={}", build_id());
}
