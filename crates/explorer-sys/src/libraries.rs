//! Windows libraries (Documents, Music, Pictures, Videos and any the user made): each is a
//! `.library-ms` file in `%APPDATA%\Microsoft\Windows\Libraries` that lists folders. Read
//! with `IShellLibrary`, which resolves known-folder and moved locations.

use std::path::PathBuf;

use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::Win32::UI::Shell::{
    IShellItem, IShellItemArray, IShellLibrary, LFF_FORCEFILESYSTEM, SHCreateItemFromParsingName, SIGDN,
    SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY, ShellLibrary,
};
use windows::core::PCWSTR;

fn name(item: &IShellItem, kind: SIGDN) -> Option<String> {
    let p = unsafe { item.GetDisplayName(kind) }.ok()?;
    let s = unsafe { p.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

/// (display name, folders) of each library, sorted by name.
pub fn libraries() -> Vec<(String, Vec<PathBuf>)> {
    use std::os::windows::ffi::OsStrExt;
    crate::windows::com_init();
    let Some(dir) = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(r"Microsoft\Windows\Libraries")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for f in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = f.path();
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("library-ms")) {
            continue;
        }
        let w: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let read = || -> windows::core::Result<(String, Vec<PathBuf>)> {
            unsafe {
                let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None)?;
                let lib: IShellLibrary = CoCreateInstance(&ShellLibrary, None, CLSCTX_INPROC_SERVER)?;
                lib.LoadLibraryFromItem(&item, STGM_READ.0)?;
                let folders: IShellItemArray = lib.GetFolders(LFF_FORCEFILESYSTEM)?;
                let mut paths = Vec::new();
                for i in 0..folders.GetCount()? {
                    if let Some(p) = folders.GetItemAt(i).ok().and_then(|f| name(&f, SIGDN_FILESYSPATH)) {
                        paths.push(PathBuf::from(p));
                    }
                }
                let display = name(&item, SIGDN_NORMALDISPLAY)
                    .unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().into_owned());
                Ok((display.trim_end_matches(".library-ms").to_string(), paths))
            }
        };
        if let Ok(lib) = read() {
            out.push(lib);
        }
    }
    out.sort_by(|a, b| explorer_core::natural_cmp(&a.0, &b.0));
    out
}
