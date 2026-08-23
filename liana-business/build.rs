fn main() {
    #[cfg(feature = "windows-icon")]
    embed_icon();
}

/// Embeds the application icon, which only the Windows executable carries.
#[cfg(feature = "windows-icon")]
fn embed_icon() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }

    let out_dir = std::env::var("OUT_DIR").expect("set by cargo");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../liana-ui/static/logos/liana-business.ico");

    // Cross-compiling from Linux needs the mingw toolchain pointed at explicitly.
    if let Ok(path) = std::env::var("TOOLKIT_x86_64_pc_windows_gnu") {
        res.set_toolkit_path(&path);
    }
    if let Ok(path) = std::env::var("WINDRES_x86_64_pc_windows_gnu") {
        res.set_windres_path(&path);
    }
    if let Ok(path) = std::env::var("AR_x86_64_pc_windows_gnu") {
        res.set_ar_path(&path);
    }

    if let Err(e) = res.compile() {
        panic!("Windows resource compilation failed: {e}");
    }

    // Cross-compilation does not pick the resource object up on its own.
    let resource_obj = format!("{out_dir}/resource.o");
    if std::path::Path::new(&resource_obj).exists() {
        println!("cargo:rustc-link-arg-bins={resource_obj}");
    }
}
