fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    // Embed the icon and version info into the Windows executable.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("FileDescription", "SuperFastImageView");
        res.set("ProductName", "SuperFastImageView");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed Windows resources: {e}");
        }
    }
}
