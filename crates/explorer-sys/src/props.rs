//! Property-system columns (Authors, Dimensions, Length, …): the same values, formatted the
//! same way, as Explorer's Details view shows, read from the item's property store.

use std::io;
use std::path::Path;

use explorer_core::Prop;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Storage::EnhancedStorage::{
    PKEY_Audio_EncodingBitrate, PKEY_Author, PKEY_Comment, PKEY_Document_PageCount, PKEY_FileOwner,
    PKEY_Image_Dimensions, PKEY_Keywords, PKEY_Media_Duration, PKEY_Media_Year, PKEY_Music_AlbumTitle,
    PKEY_Photo_CameraModel, PKEY_Photo_DateTaken, PKEY_Rating, PKEY_Title,
};
use windows::Win32::System::Com::StructuredStorage::{InitPropVariantFromStringVector, PROPVARIANT, PropVariantClear};
use windows::Win32::System::Com::{CoTaskMemAlloc, CoTaskMemFree};
use windows::Win32::System::Variant::{VT_EMPTY, VT_LPWSTR, VT_UI4};
use windows::Win32::UI::Shell::PropertiesSystem::{
    GPS_BESTEFFORT, GPS_READWRITE, IPropertyStore, PDFF_DEFAULT, PSFormatForDisplayAlloc,
    SHGetPropertyStoreFromParsingName,
};
use windows::Win32::UI::Shell::{IShellItem2, SHCreateItemFromParsingName};
use windows::core::{Interface, PCWSTR, PWSTR, Result};
fn key(p: Prop) -> Option<&'static PROPERTYKEY> {
    Some(match p {
        Prop::Status => return None,
        Prop::Authors => &PKEY_Author,
        Prop::Title => &PKEY_Title,
        Prop::Tags => &PKEY_Keywords,
        Prop::Rating => &PKEY_Rating,
        Prop::Dimensions => &PKEY_Image_Dimensions,
        Prop::Length => &PKEY_Media_Duration,
        Prop::DateTaken => &PKEY_Photo_DateTaken,
        Prop::Owner => &PKEY_FileOwner,
        Prop::CameraModel => &PKEY_Photo_CameraModel,
        Prop::Album => &PKEY_Music_AlbumTitle,
        Prop::Year => &PKEY_Media_Year,
        Prop::BitRate => &PKEY_Audio_EncodingBitrate,
        Prop::Pages => &PKEY_Document_PageCount,
        Prop::Comment => &PKEY_Comment,
    })
}

/// The display text of each of `props` for `path` (empty where the item has none).
pub fn read_props(path: &Path, props: &[Prop]) -> Vec<String> {
    crate::windows::com_init();
    let item: Option<IShellItem2> = if path.starts_with(explorer_core::SHELL) {
        // A phone's items: found through the parent folder (see `shell_ns`).
        crate::shell_ns::item(path).ok().and_then(|i| i.cast().ok())
    } else {
        let w: Vec<u16> = path.to_string_lossy().encode_utf16().chain(Some(0)).collect();
        unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None) }.ok()
    };
    let store: Option<IPropertyStore> = item.and_then(|i| unsafe { i.GetPropertyStore(GPS_BESTEFFORT) }.ok());
    let Some(store) = store else { return vec![String::new(); props.len()] };
    props
        .iter()
        .map(|&p| unsafe {
            let Some(k) = key(p) else { return String::new() };
            let Ok(v) = store.GetValue(k) else { return String::new() };
            match PSFormatForDisplayAlloc(k, &v, PDFF_DEFAULT) {
                Ok(s) => {
                    let text = s.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(s.0 as *const _));
                    text
                }
                Err(_) => String::new(),
            }
        })
        .collect()
}

/// Whether this item supports writing properties through the Windows property store.
pub fn can_write_props(path: &Path) -> bool {
    crate::windows::com_init();
    let w: Vec<u16> = path.to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let store: Result<IPropertyStore> =
        unsafe { SHGetPropertyStoreFromParsingName(PCWSTR(w.as_ptr()), None, GPS_READWRITE) };
    store.is_ok()
}

/// Writes `value` to `prop` on `path` through `IPropertyStore`.
pub fn write_prop(path: &Path, prop: Prop, value: &str) -> io::Result<()> {
    crate::windows::com_init();
    let Some(k) = key(prop) else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "unsupported property for writing"));
    };
    let w: Vec<u16> = path.to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let store: IPropertyStore = unsafe { SHGetPropertyStoreFromParsingName(PCWSTR(w.as_ptr()), None, GPS_READWRITE) }
        .map_err(|e| io::Error::other(e.to_string()))?;

    let mut var = match prop {
        Prop::Rating => {
            let r: u32 = value.trim().parse().unwrap_or(0);
            let mut v = PROPVARIANT::default();
            if r > 0 {
                unsafe {
                    (*v.Anonymous.Anonymous).vt = VT_UI4;
                    (*v.Anonymous.Anonymous).Anonymous.ulVal = r;
                }
            } else {
                unsafe {
                    (*v.Anonymous.Anonymous).vt = VT_EMPTY;
                }
            }
            v
        }
        Prop::Tags => {
            let tags = explorer_core::parse_tags(value);
            if tags.is_empty() {
                let mut v = PROPVARIANT::default();
                unsafe {
                    (*v.Anonymous.Anonymous).vt = VT_EMPTY;
                }
                v
            } else {
                let wstrings: Vec<Vec<u16>> = tags.iter().map(|s| s.encode_utf16().chain(Some(0)).collect()).collect();
                let pcwstrs: Vec<PCWSTR> = wstrings.iter().map(|w| PCWSTR(w.as_ptr())).collect();
                unsafe { InitPropVariantFromStringVector(Some(&pcwstrs)) }
                    .map_err(|e| io::Error::other(e.to_string()))?
            }
        }
        Prop::Authors => {
            let authors = if value.contains(';') {
                explorer_core::parse_tags(value)
            } else if !value.trim().is_empty() {
                vec![value.trim().to_string()]
            } else {
                Vec::new()
            };
            if authors.is_empty() {
                let mut v = PROPVARIANT::default();
                unsafe {
                    (*v.Anonymous.Anonymous).vt = VT_EMPTY;
                }
                v
            } else {
                let wstrings: Vec<Vec<u16>> =
                    authors.iter().map(|s| s.encode_utf16().chain(Some(0)).collect()).collect();
                let pcwstrs: Vec<PCWSTR> = wstrings.iter().map(|w| PCWSTR(w.as_ptr())).collect();
                unsafe { InitPropVariantFromStringVector(Some(&pcwstrs)) }
                    .map_err(|e| io::Error::other(e.to_string()))?
            }
        }
        _ => {
            if value.trim().is_empty() {
                let mut v = PROPVARIANT::default();
                unsafe {
                    (*v.Anonymous.Anonymous).vt = VT_EMPTY;
                }
                v
            } else {
                let w: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
                let bytes = w.len() * std::mem::size_of::<u16>();
                let ptr = unsafe { CoTaskMemAlloc(bytes) } as *mut u16;
                if ptr.is_null() {
                    return Err(io::Error::new(io::ErrorKind::OutOfMemory, "CoTaskMemAlloc failed"));
                }
                unsafe {
                    std::ptr::copy_nonoverlapping(w.as_ptr(), ptr, w.len());
                    let mut v = PROPVARIANT::default();
                    (*v.Anonymous.Anonymous).vt = VT_LPWSTR;
                    (*v.Anonymous.Anonymous).Anonymous.pwszVal = PWSTR(ptr);
                    v
                }
            }
        }
    };

    let res = unsafe { store.SetValue(k, &var).and_then(|_| store.Commit()) };
    unsafe {
        let _ = PropVariantClear(&mut var);
    }
    res.map_err(|e| io::Error::other(e.to_string()))
}
