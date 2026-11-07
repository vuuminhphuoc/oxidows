//! Embeds the app icon and version info in the Windows exe.

fn main() {
    let icon = concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates/ui-common/assets/explorer.ico");
    println!("cargo:rerun-if-changed={icon}");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon(icon);
        res.set("FileDescription", "File Explorer (oxidows)");
        res.set("ProductName", "oxidows");
        res.compile().expect("embedding the exe icon");
    }
}
