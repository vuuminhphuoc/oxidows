//! Search through the Windows shell, as Explorer's search box does: the query is parsed with
//! the Advanced Query Syntax parser (`kind:`, `date:`, `size:`, file contents, …) and handed to
//! a shell search folder scoped to the current folder. Windows Search answers it from its index
//! where the location is indexed, and crawls the folder otherwise.
//!
//! The search folder is built with `ISearchFolderItemFactory` rather than by parsing a
//! `search-ms:` URL: enumerating a parsed `search-ms:` item fails with E_UNEXPECTED outside
//! Explorer, on both STA and MTA threads.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Search::{
    ICondition, IQueryParser, IQueryParserManager, QueryParserManager, SQRO_DONT_SPLIT_WORDS,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Shell::{
    BHID_EnumItems, IEnumShellItems, ISearchFolderItemFactory, IShellItem, IShellItemArray,
    SHCreateItemFromParsingName, SHCreateShellItemArrayFromShellItem, SIGDN_FILESYSPATH, SearchFolderItemFactory,
};
use windows::core::{HSTRING, PWSTR, w};

/// AQS keywords (`kind:`, `date:`) are read in English, which Explorer accepts in every language.
const KEYWORD_LANG: u16 = 0x0409;

/// The search folder for `query` under `root`.
fn search_folder(root: &Path, query: &str) -> windows::core::Result<IShellItem> {
    unsafe {
        let factory: ISearchFolderItemFactory = CoCreateInstance(&SearchFolderItemFactory, None, CLSCTX_INPROC_SERVER)?;
        factory.SetDisplayName(w!("Search Results"))?;
        let scope_item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(root.as_os_str()), None)?;
        let scope: IShellItemArray = SHCreateShellItemArrayFromShellItem(&scope_item)?;
        factory.SetScope(&scope)?;

        let manager: IQueryParserManager = CoCreateInstance(&QueryParserManager, None, CLSCTX_INPROC_SERVER)?;
        let parser: IQueryParser = manager.CreateLoadedParser(w!("SystemIndex"), KEYWORD_LANG)?;
        // Natural query syntax off, automatic wildcard on: "rep" finds "report.docx", as Explorer.
        manager.InitializeOptions(false, true, &parser)?;
        let solution = parser.Parse(&HSTRING::from(query), None)?;
        let mut condition: Option<ICondition> = None;
        solution.GetQuery(Some(&mut condition), None)?;
        let condition = condition.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
        // Relative dates ("date:yesterday") are resolved against now.
        let now = GetLocalTime();
        let resolved = solution.Resolve(&condition, SQRO_DONT_SPLIT_WORDS, Some(&now))?;
        factory.SetCondition(&resolved)?;
        factory.GetShellItem()
    }
}

/// Calls `found` with each result's path until done or `cancel` is set.
pub fn shell_search(root: &Path, query: &str, cancel: &AtomicBool, found: &mut dyn FnMut(PathBuf)) -> io::Result<()> {
    crate::windows::com_init();
    let err = |step: &'static str| move |e: windows::core::Error| io::Error::other(format!("{step}: {}", e.message()));
    let folder = search_folder(root, query).map_err(err("search folder"))?;
    let items: IEnumShellItems = unsafe { folder.BindToHandler(None, &BHID_EnumItems) }.map_err(err("enumerate"))?;
    let mut buf: [Option<IShellItem>; 32] = Default::default();
    while !cancel.load(Ordering::Relaxed) {
        let mut n = 0u32;
        unsafe { items.Next(&mut buf, Some(&mut n)) }.map_err(err("results"))?;
        if n == 0 {
            break;
        }
        for item in buf.iter_mut().take(n as usize).filter_map(Option::take) {
            // Results outside the file system (mail, web history) have no path.
            if let Ok(p) = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) } {
                let path = unsafe { from_pwstr(p) };
                found(path);
            }
        }
    }
    Ok(())
}

unsafe fn from_pwstr(p: PWSTR) -> PathBuf {
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _)) };
    PathBuf::from(s)
}
