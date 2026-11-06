//! Property-system columns (Authors, Dimensions, Length, …): the same values, formatted the
//! same way, as Explorer's Details view shows, read from the item's property store.

use std::path::Path;

use explorer_core::Prop;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Storage::EnhancedStorage::{
    PKEY_Author, PKEY_FileOwner, PKEY_Image_Dimensions, PKEY_Keywords, PKEY_Media_Duration, PKEY_Photo_DateTaken,
    PKEY_Rating, PKEY_Title,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::PropertiesSystem::{
    GPS_BESTEFFORT, IPropertyStore, PDFF_DEFAULT, PSFormatForDisplayAlloc,
};
use windows::Win32::UI::Shell::{IShellItem2, SHCreateItemFromParsingName};
use windows::core::PCWSTR;

fn key(p: Prop) -> &'static PROPERTYKEY {
    match p {
        Prop::Authors => &PKEY_Author,
        Prop::Title => &PKEY_Title,
        Prop::Tags => &PKEY_Keywords,
        Prop::Rating => &PKEY_Rating,
        Prop::Dimensions => &PKEY_Image_Dimensions,
        Prop::Length => &PKEY_Media_Duration,
        Prop::DateTaken => &PKEY_Photo_DateTaken,
        Prop::Owner => &PKEY_FileOwner,
    }
}

/// The display text of each of `props` for `path` (empty where the item has none).
pub fn read_props(path: &Path, props: &[Prop]) -> Vec<String> {
    use std::os::windows::ffi::OsStrExt;
    crate::windows::com_init();
    let w: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let store: Option<IPropertyStore> = unsafe {
        SHCreateItemFromParsingName::<_, _, IShellItem2>(PCWSTR(w.as_ptr()), None)
            .and_then(|item| item.GetPropertyStore(GPS_BESTEFFORT))
            .ok()
    };
    let Some(store) = store else { return vec![String::new(); props.len()] };
    props
        .iter()
        .map(|&p| unsafe {
            let k = key(p);
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
