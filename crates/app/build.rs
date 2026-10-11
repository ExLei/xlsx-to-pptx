fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let icon = format!("{}/assets/icon.ico", std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon(&icon);
        if let Err(error) = resource.compile() {
            eprintln!("warning: failed to set icon: {error}");
        }
    }
}
