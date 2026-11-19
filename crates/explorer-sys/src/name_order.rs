//! Name order from Windows itself.
//!
//! Explorer sorts names with `StrCmpLogicalW`: digits compare as numbers, and punctuation, accents
//! and the user's language follow Windows' collation tables, which `explorer_core::natural_cmp_portable`
//! can't reproduce. `use_windows_name_order` makes the whole app use the real thing.

use std::cmp::Ordering;

use windows_sys::Win32::UI::Shell::StrCmpLogicalW;

/// Calls `f` with `s` as a NUL-terminated UTF-16 string, without allocating for names under 260 units.
fn with_wide<R>(s: &str, f: impl FnOnce(*const u16) -> R) -> R {
    let mut buf = [0u16; 260];
    let mut n = 0;
    for unit in s.encode_utf16() {
        if n == buf.len() - 1 {
            let long: Vec<u16> = s.encode_utf16().chain(Some(0)).collect();
            return f(long.as_ptr());
        }
        buf[n] = unit;
        n += 1;
    }
    buf[n] = 0;
    f(buf.as_ptr())
}

/// `StrCmpLogicalW` on two names.
pub fn windows_name_cmp(a: &str, b: &str) -> Ordering {
    with_wide(a, |pa| {
        // SAFETY: both pointers are to NUL-terminated buffers that outlive the call.
        with_wide(b, |pb| unsafe { StrCmpLogicalW(pa, pb) }.cmp(&0))
    })
}

/// Makes `explorer_core::natural_cmp` (and with it every sorted view) use [`windows_name_cmp`].
pub fn use_windows_name_order() {
    explorer_core::set_name_order(windows_name_cmp);
}
