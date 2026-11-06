#![forbid(unsafe_code)]

//! Platform-independent part of the File Explorer.
//!
//! `explorer-sys` fills [`Entry`] values straight from the OS (one `FindFirstFileExW` pass on Windows,
//! no extra stat per file). Everything here — sorting, filtering, size/date/type text, the
//! back/forward history — touches no OS API and is unit-tested on any platform.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// One item in a folder listing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    /// Bytes; 0 for folders.
    pub size: u64,
    /// Seconds since 1970-01-01 UTC; 0 when unknown.
    pub modified: i64,
    /// Creation time, same units; 0 when unknown.
    pub created: i64,
    /// Win32 `FILE_ATTRIBUTE_*` bits (see [`attr`]).
    pub attrs: u32,
    pub hidden: bool,
    pub system: bool,
    /// Reparse point (symlink / junction / cloud placeholder).
    pub link: bool,
    /// Shown instead of `name` when set (Recycle Bin items: the original name).
    pub label: Option<String>,
    /// Extra text for the third column where it isn't the type (Recycle Bin: original folder).
    pub note: Option<String>,
}

impl Entry {
    /// The name the user sees.
    pub fn display(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }

    /// Lowercase extension without the dot, "" for folders and extension-less files.
    pub fn extension(&self) -> String {
        if self.is_dir {
            return String::new();
        }
        let name = self.display();
        match name.rfind('.') {
            Some(i) if i > 0 && i + 1 < name.len() => name[i + 1..].to_ascii_lowercase(),
            _ => String::new(),
        }
    }

    /// Text for the "Type" column.
    pub fn type_name(&self) -> String {
        if self.is_dir {
            return "File folder".into();
        }
        type_name_for_ext(&self.extension())
    }
}

/// Short type names for common extensions; others become "XYZ File", as Explorer does when no
/// application registered a friendly name. Reading the registry for every row would cost more
/// than the whole listing, so this is a static table.
pub fn type_name_for_ext(ext: &str) -> String {
    let known = match ext {
        "" => return "File".into(),
        "txt" | "log" => "Text Document",
        "md" => "Markdown File",
        "pdf" => "PDF Document",
        "doc" | "docx" => "Word Document",
        "xls" | "xlsx" => "Excel Worksheet",
        "ppt" | "pptx" => "PowerPoint Presentation",
        "csv" => "CSV File",
        "exe" => "Application",
        "msi" => "Windows Installer Package",
        "dll" => "Application extension",
        "bat" | "cmd" => "Windows Batch File",
        "ps1" => "PowerShell Script",
        "lnk" => "Shortcut",
        "url" => "Internet Shortcut",
        "zip" => "Compressed (zipped) Folder",
        "7z" | "rar" | "tar" | "gz" | "xz" | "zst" => "Archive",
        "iso" => "Disc Image File",
        "jpg" | "jpeg" => "JPEG Image",
        "png" => "PNG Image",
        "gif" => "GIF Image",
        "bmp" => "Bitmap Image",
        "webp" => "WebP Image",
        "svg" => "SVG Image",
        "ico" => "Icon",
        "mp3" => "MP3 Audio",
        "wav" => "WAV Audio",
        "flac" => "FLAC Audio",
        "mp4" => "MP4 Video",
        "mkv" => "MKV Video",
        "avi" => "AVI Video",
        "mov" => "QuickTime Movie",
        "html" | "htm" => "HTML Document",
        "json" => "JSON File",
        "xml" => "XML Document",
        "ini" | "cfg" | "conf" => "Configuration Settings",
        "rs" => "Rust Source File",
        "py" => "Python File",
        "js" => "JavaScript File",
        "ts" => "TypeScript File",
        "c" | "h" | "cpp" | "hpp" => "C/C++ Source File",
        "ttf" | "otf" => "Font File",
        "sys" => "System file",
        _ => return format!("{} File", ext.to_ascii_uppercase()),
    };
    known.into()
}

/// Glyph for the name column (egui's bundled emoji font has these).
pub fn icon_for(e: &Entry) -> &'static str {
    if e.is_dir {
        return "📁";
    }
    match e.extension().as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "svg" | "ico" => "🖼",
        "mp3" | "wav" | "flac" | "ogg" | "m4a" => "🎵",
        "mp4" | "mkv" | "avi" | "mov" | "webm" => "🎞",
        "zip" | "7z" | "rar" | "tar" | "gz" | "xz" | "zst" => "📦",
        "exe" | "msi" | "bat" | "cmd" | "ps1" => "⚙",
        "lnk" | "url" => "🔗",
        _ => "📄",
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortKey {
    #[default]
    Name,
    Modified,
    Type,
    Size,
    Created,
    Attributes,
}

/// Win32 file attribute bits, as `FindFirstFileExW` reports them.
pub mod attr {
    pub const READONLY: u32 = 0x1;
    pub const HIDDEN: u32 = 0x2;
    pub const SYSTEM: u32 = 0x4;
    pub const DIRECTORY: u32 = 0x10;
    pub const ARCHIVE: u32 = 0x20;
    pub const COMPRESSED: u32 = 0x800;
    pub const OFFLINE: u32 = 0x1000;
    pub const NOT_INDEXED: u32 = 0x2000;
    pub const ENCRYPTED: u32 = 0x4000;
    pub const REPARSE: u32 = 0x400;
    pub const PINNED: u32 = 0x80000;
    pub const UNPINNED: u32 = 0x100000;
}

/// The Attributes column: one letter per set attribute, in Explorer's order
/// (R read-only, H hidden, S system, A archive, C compressed, E encrypted, O offline,
/// N not indexed, L link, P pinned, U unpinned).
pub fn format_attributes(attrs: u32) -> String {
    const LETTERS: [(u32, char); 11] = [
        (attr::READONLY, 'R'),
        (attr::HIDDEN, 'H'),
        (attr::SYSTEM, 'S'),
        (attr::ARCHIVE, 'A'),
        (attr::COMPRESSED, 'C'),
        (attr::ENCRYPTED, 'E'),
        (attr::OFFLINE, 'O'),
        (attr::NOT_INDEXED, 'N'),
        (attr::REPARSE, 'L'),
        (attr::PINNED, 'P'),
        (attr::UNPINNED, 'U'),
    ];
    LETTERS.iter().filter(|(bit, _)| attrs & bit != 0).map(|&(_, c)| c).collect()
}

/// Natural ("logical") order like Explorer: case-insensitive and "file2" < "file10".
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let na: String = std::iter::from_fn(|| a.next_if(|c| c.is_ascii_digit())).collect();
                let nb: String = std::iter::from_fn(|| b.next_if(|c| c.is_ascii_digit())).collect();
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)).then_with(|| na.len().cmp(&nb.len()));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// Indices into `entries`: filtered by `filter` (case-insensitive substring), hidden items
/// dropped unless `show_hidden`, folders always first, then sorted by `key`.
pub fn sorted_view(entries: &[Entry], filter: &str, show_hidden: bool, key: SortKey, ascending: bool) -> Vec<usize> {
    sorted_view_with(entries, filter, show_hidden, show_hidden, key, ascending)
}

/// [`sorted_view`] with "protected operating system files" (hidden + system) controlled on
/// their own, as Explorer's folder options do.
pub fn sorted_view_with(
    entries: &[Entry],
    filter: &str,
    show_hidden: bool,
    show_protected: bool,
    key: SortKey,
    ascending: bool,
) -> Vec<usize> {
    let needle = filter.trim().to_lowercase();
    let mut idx: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| if e.system { show_hidden && show_protected } else { show_hidden || !e.hidden })
        .filter(|(_, e)| needle.is_empty() || e.display().to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect();
    // Type names are computed once per entry, not once per comparison.
    let types: Vec<String> =
        if key == SortKey::Type { entries.iter().map(Entry::type_name).collect() } else { Vec::new() };
    idx.sort_by(|&i, &j| {
        let (a, b) = (&entries[i], &entries[j]);
        let by_key = match key {
            SortKey::Name => Ordering::Equal,
            SortKey::Modified => a.modified.cmp(&b.modified),
            SortKey::Type => types[i].cmp(&types[j]),
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Created => a.created.cmp(&b.created),
            SortKey::Attributes => format_attributes(a.attrs).cmp(&format_attributes(b.attrs)),
        }
        .then_with(|| natural_cmp(a.display(), b.display()));
        b.is_dir.cmp(&a.is_dir).then(if ascending { by_key } else { by_key.reverse() })
    });
    idx
}

/// Explorer's Size column: always whole KB with thousands separators ("1 KB" minimum for a
/// non-empty file), "0 KB" for empty files.
pub fn format_size_kb(bytes: u64) -> String {
    let kb = bytes.div_ceil(1024);
    format!("{} KB", group_thousands(kb))
}

/// Short human size for the status bar and drives ("12.4 GB").
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} bytes")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub fn group_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "2026-11-06 16:41" in local time. `utc_offset_secs` comes from `explorer-sys`.
pub fn format_time(unix_secs: i64, utc_offset_secs: i64) -> String {
    if unix_secs <= 0 {
        return String::new();
    }
    let t = unix_secs + utc_offset_secs;
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

/// Days since 1970-01-01 to (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// A name the user typed for a new or renamed item: rejects what Windows rejects.
pub fn validate_name(name: &str) -> Result<(), &'static str> {
    let n = name.trim_end_matches([' ', '.']);
    if n.is_empty() {
        return Err("Name cannot be empty");
    }
    if name.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 32) {
        return Err(r#"A name can't contain any of: \ / : * ? " < > |"#);
    }
    let stem = n.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0');
    if reserved {
        return Err("This name is reserved by Windows");
    }
    Ok(())
}

/// "New folder", "New folder (2)", … — the first name not in `existing` (case-insensitive).
pub fn unique_name(base: &str, existing: &[Entry]) -> String {
    let taken = |n: &str| existing.iter().any(|e| e.name.eq_ignore_ascii_case(n));
    if !taken(base) {
        return base.into();
    }
    (2..).map(|i| format!("{base} ({i})")).find(|n| !taken(n)).expect("unbounded")
}

/// Browser-style back/forward over visited folders.
#[derive(Debug, Default)]
pub struct History {
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
}

impl History {
    /// Record leaving `from` for a new place (clears forward).
    pub fn visit(&mut self, from: PathBuf) {
        if self.back.last() != Some(&from) {
            self.back.push(from);
        }
        self.forward.clear();
    }
    pub fn back(&mut self, current: PathBuf) -> Option<PathBuf> {
        let to = self.back.pop()?;
        self.forward.push(current);
        Some(to)
    }
    pub fn forward(&mut self, current: PathBuf) -> Option<PathBuf> {
        let to = self.forward.pop()?;
        self.back.push(current);
        Some(to)
    }
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }
    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// The Recent locations list, newest first: (steps, place) where steps > 0 are forward,
    /// 0 is `current`, < 0 are back. Pass `steps` to [`History::jump`].
    pub fn recent(&self, current: &Path) -> Vec<(isize, PathBuf)> {
        let mut out: Vec<(isize, PathBuf)> =
            self.forward.iter().enumerate().map(|(i, p)| ((self.forward.len() - i) as isize, p.clone())).collect();
        out.push((0, current.to_path_buf()));
        out.extend(self.back.iter().rev().enumerate().map(|(i, p)| (-(i as isize) - 1, p.clone())));
        out
    }

    /// Moves `steps` back (negative) or forward (positive) at once.
    pub fn jump(&mut self, mut current: PathBuf, steps: isize) -> Option<PathBuf> {
        let mut moved = false;
        for _ in 0..steps.unsigned_abs() {
            let next = if steps < 0 { self.back(current.clone()) } else { self.forward(current.clone()) };
            match next {
                Some(p) => {
                    current = p;
                    moved = true;
                }
                None => break,
            }
        }
        moved.then_some(current)
    }
}

/// Breadcrumb segments: (label, full path) from the root down to `path`.
pub fn breadcrumbs(path: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = path
        .ancestors()
        .map(|p| {
            let label = match p.file_name() {
                Some(n) => n.to_string_lossy().into_owned(),
                None => p.to_string_lossy().trim_end_matches(['\\', '/']).to_string(),
            };
            let label = if label.is_empty() { p.to_string_lossy().into_owned() } else { label };
            (label, p.to_path_buf())
        })
        .collect();
    out.reverse();
    out
}

/// The virtual "Network" location (computers on the LAN). Not a real path.
pub const NETWORK: &str = "::network";

/// The virtual Recycle Bin location.
pub const RECYCLE_BIN: &str = "::recycle-bin";

/// The virtual This PC page: known folders and drives. Its items are named by full path.
pub const THIS_PC: &str = "::this-pc";

/// What kind of location a path is, for the parts that are not plain folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// The list of computers.
    Network,
    RecycleBin,
    ThisPc,
    /// `\\server`: the list of its shares.
    Server(String),
    /// `\\server\share\...`
    Unc {
        server: String,
        share: String,
    },
    /// Anything else: a drive path.
    Local,
}

pub fn classify(p: &Path) -> Place {
    let s = p.to_string_lossy();
    if s == NETWORK {
        return Place::Network;
    }
    if s == RECYCLE_BIN {
        return Place::RecycleBin;
    }
    if s == THIS_PC {
        return Place::ThisPc;
    }
    let Some(rest) = s.strip_prefix(r"\\") else { return Place::Local };
    if rest.starts_with(r"?\") || rest.starts_with(r".\") {
        return Place::Local; // \\?\C:\... and device paths
    }
    let mut parts = rest.split('\\').filter(|x| !x.is_empty());
    match (parts.next(), parts.next()) {
        (Some(server), None) => Place::Server(server.to_string()),
        (Some(server), Some(share)) => Place::Unc { server: server.into(), share: share.into() },
        _ => Place::Network,
    }
}

/// Whether `p` starts with a drive letter or `\\?\` prefix (Windows paths).
fn has_drive(p: &Path) -> bool {
    matches!(p.components().next(), Some(std::path::Component::Prefix(_)))
}

/// Path of the item `name` inside `cur` (handles the virtual Network and server levels).
pub fn child_path(cur: &Path, name: &str) -> PathBuf {
    match classify(cur) {
        Place::Network => PathBuf::from(format!(r"\\{name}")),
        Place::ThisPc => PathBuf::from(name),
        Place::Server(server) => PathBuf::from(format!(r"\\{server}\{name}")),
        _ => cur.join(name),
    }
}

/// One level up: share root -> server -> Network; drive root -> none.
pub fn parent_path(cur: &Path) -> Option<PathBuf> {
    match classify(cur) {
        Place::Network | Place::RecycleBin | Place::ThisPc => None,
        Place::Server(_) => Some(PathBuf::from(NETWORK)),
        Place::Unc { server, share } => {
            let s = cur.to_string_lossy();
            let root = format!(r"\\{server}\{share}");
            if s.trim_end_matches('\\').eq_ignore_ascii_case(&root) {
                Some(PathBuf::from(format!(r"\\{server}")))
            } else {
                let trimmed = s.trim_end_matches('\\');
                trimmed.rfind('\\').map(|i| PathBuf::from(&trimmed[..i]))
            }
        }
        // A drive root ("C:\\") goes up to This PC, as in Explorer.
        Place::Local => cur.parent().map(Path::to_path_buf).or_else(|| has_drive(cur).then(|| PathBuf::from(THIS_PC))),
    }
}

/// Breadcrumbs for any location, network levels included.
pub fn location_crumbs(cur: &Path) -> Vec<(String, PathBuf)> {
    match classify(cur) {
        Place::Network => vec![("Network".into(), PathBuf::from(NETWORK))],
        Place::RecycleBin => vec![("Recycle Bin".into(), PathBuf::from(RECYCLE_BIN))],
        Place::ThisPc => vec![("This PC".into(), PathBuf::from(THIS_PC))],
        Place::Server(server) => {
            vec![("Network".into(), PathBuf::from(NETWORK)), (server.clone(), PathBuf::from(format!(r"\\{server}")))]
        }
        Place::Unc { server, .. } => {
            let mut out = vec![
                ("Network".into(), PathBuf::from(NETWORK)),
                (server.clone(), PathBuf::from(format!(r"\\{server}"))),
            ];
            let s = cur.to_string_lossy();
            let rest = s.trim_start_matches('\\').trim_end_matches('\\');
            let mut acc = String::from(r"\\");
            for (i, seg) in rest.split('\\').enumerate() {
                if i > 0 {
                    acc.push('\\');
                }
                acc.push_str(seg);
                if i > 0 {
                    out.push((seg.to_string(), PathBuf::from(&acc)));
                }
            }
            out
        }
        Place::Local if has_drive(cur) => {
            let mut v = vec![("This PC".into(), PathBuf::from(THIS_PC))];
            v.extend(breadcrumbs(cur));
            v
        }
        Place::Local => breadcrumbs(cur),
    }
}

/// Extensions the preview pane shows as text.
pub fn is_text_ext(ext: &str) -> bool {
    matches!(
        ext,
        "txt"
            | "md"
            | "log"
            | "ini"
            | "cfg"
            | "conf"
            | "csv"
            | "tsv"
            | "json"
            | "xml"
            | "yaml"
            | "yml"
            | "toml"
            | "rs"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "cs"
            | "java"
            | "kt"
            | "go"
            | "py"
            | "rb"
            | "js"
            | "ts"
            | "tsx"
            | "jsx"
            | "css"
            | "html"
            | "htm"
            | "sql"
            | "sh"
            | "ps1"
            | "psm1"
            | "bat"
            | "cmd"
            | "reg"
            | "gitignore"
            | "editorconfig"
            | "lock"
            | "vbs"
            | "lua"
            | "php"
            | "swift"
            | "srt"
    )
}

/// Text for the preview pane from the first bytes of a file: UTF-8 (with or without BOM) or
/// UTF-16 with a BOM. `None` for binary data. A cut-off last character is dropped.
pub fn decode_text(bytes: &[u8]) -> Option<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return Some(String::from_utf16_lossy(&units));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return Some(String::from_utf16_lossy(&units));
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    if bytes.contains(&0) {
        return None;
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Some(s.to_owned()),
        // Cut in the middle of a character at the end of the read.
        Err(e) if e.error_len().is_none() => Some(String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned()),
        // Not UTF-8: probably a legacy code page; show it lossily like Notepad would garble it.
        Err(_) => Some(String::from_utf8_lossy(bytes).into_owned()),
    }
}

/// Expands `%NAME%` environment variables as the address bar does; unknown names stay as typed.
pub fn expand_env(text: &str, var: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => {
                let name = &after[..end];
                match var(name) {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `shell:` folder names (case-insensitive, after the colon) as a path template with
/// environment variables, or one of the virtual places. Known folders that can be
/// redirected (Desktop, Documents, …) are resolved by the caller from the system first.
pub fn shell_folder_template(name: &str) -> Option<&'static str> {
    let n = name.trim().to_ascii_lowercase().replace(' ', "");
    Some(match n.as_str() {
        "appdata" => "%APPDATA%",
        "localappdata" => "%LOCALAPPDATA%",
        "localappdatalow" => "%USERPROFILE%\\AppData\\LocalLow",
        "profile" | "userprofiles" => "%USERPROFILE%",
        "startup" => "%APPDATA%\\Microsoft\\Windows\\Start Menu\\Programs\\Startup",
        "commonstartup" => "%ProgramData%\\Microsoft\\Windows\\Start Menu\\Programs\\StartUp",
        "programs" => "%APPDATA%\\Microsoft\\Windows\\Start Menu\\Programs",
        "commonprograms" => "%ProgramData%\\Microsoft\\Windows\\Start Menu\\Programs",
        "startmenu" => "%APPDATA%\\Microsoft\\Windows\\Start Menu",
        "sendto" => "%APPDATA%\\Microsoft\\Windows\\SendTo",
        "recent" => "%APPDATA%\\Microsoft\\Windows\\Recent",
        "templates" => "%APPDATA%\\Microsoft\\Windows\\Templates",
        "quicklaunch" => "%APPDATA%\\Microsoft\\Internet Explorer\\Quick Launch",
        "fonts" => "%WINDIR%\\Fonts",
        "windows" => "%WINDIR%",
        "system" => "%WINDIR%\\System32",
        "programfiles" => "%ProgramFiles%",
        "programfilesx86" => "%ProgramFiles(x86)%",
        "commonappdata" => "%ProgramData%",
        "public" => "%PUBLIC%",
        "downloads" => "%USERPROFILE%\\Downloads",
        "desktop" => "%USERPROFILE%\\Desktop",
        "personal" | "documents" => "%USERPROFILE%\\Documents",
        "mypictures" | "pictures" => "%USERPROFILE%\\Pictures",
        "mymusic" | "music" => "%USERPROFILE%\\Music",
        "myvideo" | "videos" => "%USERPROFILE%\\Videos",
        "recyclebinfolder" => RECYCLE_BIN,
        "networkplacesfolder" => NETWORK,
        "mycomputerfolder" | "thispc" => THIS_PC,
        _ => return None,
    })
}

/// Group by, as in Explorer's View ▸ Group by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GroupBy {
    #[default]
    None,
    Name,
    Modified,
    Type,
    Size,
}

/// One group of the view: its heading and the positions `start..start + len` of `view`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub label: String,
    pub start: usize,
    pub len: usize,
}

/// Explorer's group for an item: (order, heading). Groups sort by `order`.
pub fn group_of(e: &Entry, by: GroupBy, now: i64, utc_offset: i64) -> (u32, String) {
    match by {
        GroupBy::None => (0, String::new()),
        GroupBy::Name => {
            let c = e.display().chars().next().map(|c| c.to_ascii_uppercase()).unwrap_or(' ');
            match c {
                '0'..='9' => (0, "0–9".into()),
                'A'..='H' => (1, "A – H".into()),
                'I'..='P' => (2, "I – P".into()),
                'Q'..='Z' => (3, "Q – Z".into()),
                _ => (4, "Other".into()),
            }
        }
        GroupBy::Type => (0, e.type_name()),
        GroupBy::Size => {
            if e.is_dir {
                return (0, "Folders".into());
            }
            const KB: u64 = 1024;
            const MB: u64 = KB * 1024;
            const GB: u64 = MB * 1024;
            let (o, l) = match e.size {
                0 => (1, "Empty (0 KB)"),
                s if s <= 16 * KB => (2, "Tiny (0 – 16 KB)"),
                s if s <= MB => (3, "Small (16 KB – 1 MB)"),
                s if s <= 128 * MB => (4, "Medium (1 – 128 MB)"),
                s if s <= GB => (5, "Large (128 MB – 1 GB)"),
                s if s <= 4 * GB => (6, "Huge (1 – 4 GB)"),
                _ => (7, "Gigantic (> 4 GB)"),
            };
            (o, l.into())
        }
        GroupBy::Modified => {
            if e.modified <= 0 {
                return (9, "Unknown".into());
            }
            let day = |t: i64| (t + utc_offset).div_euclid(86_400);
            let (today, d) = (day(now), day(e.modified));
            // 1970-01-01 was a Thursday; weeks start on Monday.
            let weekday = (today + 3).rem_euclid(7);
            let week_start = today - weekday;
            let (y, m, _) = civil_from_days(today);
            let (dy, dm, _) = civil_from_days(d);
            let (o, l) = if d > today {
                (0, "In the future")
            } else if d == today {
                (1, "Today")
            } else if d == today - 1 {
                (2, "Yesterday")
            } else if d >= week_start {
                (3, "Earlier this week")
            } else if d >= week_start - 7 {
                (4, "Last week")
            } else if dy == y && dm == m {
                (5, "Earlier this month")
            } else if (dy == y && dm + 1 == m) || (dy + 1 == y && m == 1 && dm == 12) {
                (6, "Last month")
            } else if dy == y {
                (7, "Earlier this year")
            } else {
                (8, "A long time ago")
            };
            (o, l.into())
        }
    }
}

/// Reorders `view` (already sorted) so each group is contiguous, groups in Explorer's order
/// (by name for Type; reversed when sorting descending), keeping the sort inside each group.
pub fn group_view(
    entries: &[Entry],
    view: &mut Vec<usize>,
    by: GroupBy,
    ascending: bool,
    now: i64,
    utc_offset: i64,
) -> Vec<Group> {
    if by == GroupBy::None {
        return Vec::new();
    }
    let keyed: Vec<((u32, String), usize)> =
        view.iter().map(|&i| (group_of(&entries[i], by, now, utc_offset), i)).collect();
    let mut order: Vec<(u32, String)> = keyed.iter().map(|(k, _)| k.clone()).collect();
    order.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| natural_cmp(&a.1, &b.1)));
    order.dedup();
    if !ascending {
        order.reverse();
    }
    let mut groups = Vec::with_capacity(order.len());
    let mut out = Vec::with_capacity(view.len());
    for k in order {
        let start = out.len();
        out.extend(keyed.iter().filter(|(g, _)| *g == k).map(|(_, i)| *i));
        groups.push(Group { label: k.1, start, len: out.len() - start });
    }
    *view = out;
    groups
}

/// Batch rename, as Explorer does it: every item gets `base (1)`, `base (2)`, … in the given
/// order, each keeping its own extension (folders have none). `base` may carry the first
/// item's extension, which is dropped. Folders count as extension-less.
pub fn batch_names(base: &str, items: &[(String, bool)]) -> Vec<String> {
    let stem_of = |n: &str| match n.rfind('.') {
        Some(i) if i > 0 => (n[..i].to_string(), n[i..].to_string()),
        _ => (n.to_string(), String::new()),
    };
    let base = match items.first() {
        Some((first, false)) => {
            let (_, ext) = stem_of(first);
            if !ext.is_empty() && base.to_lowercase().ends_with(&ext.to_lowercase()) {
                base[..base.len() - ext.len()].to_string()
            } else {
                base.to_string()
            }
        }
        _ => base.to_string(),
    };
    let base = base.trim_end();
    items
        .iter()
        .enumerate()
        .map(|(i, (name, is_dir))| {
            let ext = if *is_dir { String::new() } else { stem_of(name).1 };
            format!("{base} ({}){ext}", i + 1)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str, size: u64) -> Entry {
        Entry { name: name.into(), size, ..Default::default() }
    }
    fn d(name: &str) -> Entry {
        Entry { name: name.into(), is_dir: true, ..Default::default() }
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["file10", "File2", "file1", "a", "file02"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["a", "file1", "File2", "file02", "file10"]);
    }

    #[test]
    fn folders_first_and_sort_by_size_desc() {
        let e = vec![f("b.txt", 10), d("zeta"), f("a.txt", 500), d("alpha")];
        let v = sorted_view(&e, "", true, SortKey::Size, false);
        let names: Vec<&str> = v.iter().map(|&i| e[i].name.as_str()).collect();
        assert_eq!(names, ["zeta", "alpha", "a.txt", "b.txt"]);
        let v = sorted_view(&e, "", true, SortKey::Name, true);
        let names: Vec<&str> = v.iter().map(|&i| e[i].name.as_str()).collect();
        assert_eq!(names, ["alpha", "zeta", "a.txt", "b.txt"]);
    }

    #[test]
    fn filter_and_hidden() {
        let mut h = f("secret.TXT", 1);
        h.hidden = true;
        let e = vec![f("Report.txt", 1), h, f("photo.png", 1)];
        assert_eq!(sorted_view(&e, "TXT", false, SortKey::Name, true), [0]);
        assert_eq!(sorted_view(&e, "txt", true, SortKey::Name, true).len(), 2);
    }

    #[test]
    fn sizes() {
        assert_eq!(format_size_kb(0), "0 KB");
        assert_eq!(format_size_kb(1), "1 KB");
        assert_eq!(format_size_kb(1024), "1 KB");
        assert_eq!(format_size_kb(1025), "2 KB");
        assert_eq!(format_size_kb(5_000_000_000), "4,882,813 KB");
        assert_eq!(format_bytes(512), "512 bytes");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(500 * 1024 * 1024 * 1024), "500 GB");
    }

    #[test]
    fn times() {
        assert_eq!(format_time(0, 0), "");
        assert_eq!(format_time(1_793_981_287, 0), "2026-11-06 16:08");
        assert_eq!(format_time(1_793_981_287, 7 * 3600), "2026-11-06 23:08");
        assert_eq!(format_time(951_782_400, 0), "2000-02-29 00:00");
    }

    #[test]
    fn types_and_ext() {
        assert_eq!(f("a.TXT", 1).type_name(), "Text Document");
        assert_eq!(f("a.foo", 1).type_name(), "FOO File");
        assert_eq!(f(".gitignore", 1).type_name(), "File");
        assert_eq!(f("noext", 1).type_name(), "File");
        assert_eq!(d("x.txt").type_name(), "File folder");
    }

    #[test]
    fn names() {
        assert!(validate_name("ok name.txt").is_ok());
        assert!(validate_name("a:b").is_err());
        assert!(validate_name("   ").is_err());
        assert!(validate_name("con.txt").is_err());
        assert!(validate_name("COM1").is_err());
        assert!(validate_name("COM0").is_ok());
        assert!(validate_name("console").is_ok());
        let e = vec![d("New folder"), d("new folder (2)")];
        assert_eq!(unique_name("New folder", &e), "New folder (3)");
        assert_eq!(unique_name("Other", &e), "Other");
    }

    #[test]
    fn history() {
        let mut h = History::default();
        h.visit("/a".into());
        h.visit("/b".into());
        assert_eq!(h.back("/c".into()), Some("/b".into()));
        assert_eq!(h.forward("/b".into()), Some("/c".into()));
        assert!(h.can_back());
        h.visit("/c".into());
        assert!(!h.can_forward());
    }

    #[test]
    fn crumbs() {
        let c = breadcrumbs(Path::new("/home/user/docs"));
        let labels: Vec<&str> = c.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["/", "home", "user", "docs"]);
        assert_eq!(c[2].1, PathBuf::from("/home/user"));
    }

    #[test]
    fn network_places() {
        let n = Path::new(NETWORK);
        assert_eq!(classify(n), Place::Network);
        assert_eq!(classify(Path::new(r"\\nas")), Place::Server("nas".into()));
        assert_eq!(classify(Path::new(r"\\nas\")), Place::Server("nas".into()));
        assert_eq!(classify(Path::new(r"\\nas\media\x")), Place::Unc { server: "nas".into(), share: "media".into() });
        assert_eq!(classify(Path::new(r"\\?\C:\long")), Place::Local);
        assert_eq!(child_path(n, "NAS"), PathBuf::from(r"\\NAS"));
        assert_eq!(child_path(Path::new(r"\\nas"), "media"), PathBuf::from(r"\\nas\media"));
        assert_eq!(parent_path(Path::new(r"\\nas\media\films")), Some(PathBuf::from(r"\\nas\media")));
        assert_eq!(parent_path(Path::new(r"\\nas\media\")), Some(PathBuf::from(r"\\nas")));
        assert_eq!(parent_path(Path::new(r"\\nas")), Some(PathBuf::from(NETWORK)));
        assert_eq!(parent_path(n), None);
        let c = location_crumbs(Path::new(r"\\nas\media\films"));
        let labels: Vec<&str> = c.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["Network", "nas", "media", "films"]);
        assert_eq!(c[2].1, PathBuf::from(r"\\nas\media"));
    }

    #[test]
    fn attribute_letters() {
        assert_eq!(format_attributes(0), "");
        assert_eq!(format_attributes(attr::ARCHIVE), "A");
        assert_eq!(format_attributes(attr::HIDDEN | attr::SYSTEM | attr::ARCHIVE | attr::DIRECTORY), "HSA");
        assert_eq!(format_attributes(attr::READONLY | attr::REPARSE), "RL");
    }

    #[test]
    fn text_preview_decoding() {
        assert_eq!(decode_text(b"hello").as_deref(), Some("hello"));
        assert_eq!(decode_text(b"\xEF\xBB\xBFhi").as_deref(), Some("hi"));
        assert_eq!(decode_text(&[0xFF, 0xFE, b'h', 0, b'i', 0]).as_deref(), Some("hi"));
        assert_eq!(decode_text(b"MZ\x90\0\x03"), None);
        // "é" is C3 A9; a read that stops after C3 drops it.
        assert_eq!(decode_text(b"caf\xC3").as_deref(), Some("caf"));
        assert!(is_text_ext("rs") && !is_text_ext("exe"));
    }

    #[test]
    fn address_expansion() {
        let env = |k: &str| match k {
            "TEMP" => Some(r"C:\T".to_string()),
            "APPDATA" => Some(r"C:\U\R".to_string()),
            _ => None,
        };
        assert_eq!(expand_env(r"%TEMP%\x", env), r"C:\T\x");
        assert_eq!(expand_env("%NOPE%\\x %", env), "%NOPE%\\x %");
        assert_eq!(expand_env("100%", env), "100%");
        let t = shell_folder_template("Startup").unwrap();
        assert_eq!(expand_env(t, env), r"C:\U\R\Microsoft\Windows\Start Menu\Programs\Startup");
        assert_eq!(shell_folder_template("RecycleBinFolder"), Some(RECYCLE_BIN));
        assert_eq!(shell_folder_template("common startup"), shell_folder_template("CommonStartup"));
        assert_eq!(shell_folder_template("bogus"), None);
    }

    #[test]
    fn recent_locations() {
        let p = |s: &str| PathBuf::from(s);
        let mut h = History::default();
        h.visit(p("/a"));
        h.visit(p("/b"));
        // now at /c, back = [/a, /b]
        let back1 = h.back(p("/c")).unwrap();
        assert_eq!(back1, p("/b"));
        let r = h.recent(&back1);
        assert_eq!(r, [(1, p("/c")), (0, p("/b")), (-1, p("/a"))]);
        assert_eq!(h.jump(p("/b"), -1), Some(p("/a")));
        assert_eq!(h.jump(p("/a"), 2), Some(p("/c")));
        assert!(!h.can_forward());
    }

    #[test]
    fn grouping() {
        let f =
            |name: &str, size: u64, modified: i64| Entry { name: name.into(), size, modified, ..Default::default() };
        // 2026-11-06 (Friday) 12:00 UTC.
        let now = 1_793_966_400;
        assert_eq!(civil_from_days(now / 86_400), (2026, 11, 6));
        let day = 86_400;
        let e = vec![
            f("b.txt", 10, now - 60),
            f("a.txt", 5_000_000, now - day),
            f("zeta.txt", 0, now - 3 * day), // Tuesday: earlier this week
            f("old.txt", 2_000_000_000, now - 400 * day),
            f("9.txt", 20_000, now - 8 * day), // last week
        ];
        let label = |by, i: usize| group_of(&e[i], by, now, 0).1;
        assert_eq!(label(GroupBy::Modified, 0), "Today");
        assert_eq!(label(GroupBy::Modified, 1), "Yesterday");
        assert_eq!(label(GroupBy::Modified, 2), "Earlier this week");
        assert_eq!(label(GroupBy::Modified, 4), "Last week");
        assert_eq!(label(GroupBy::Modified, 3), "A long time ago");
        assert_eq!(label(GroupBy::Size, 3), "Huge (1 – 4 GB)");
        assert_eq!(label(GroupBy::Name, 4), "0–9");

        let mut view: Vec<usize> = (0..e.len()).collect();
        let g = group_view(&e, &mut view, GroupBy::Name, true, now, 0);
        let labels: Vec<&str> = g.iter().map(|g| g.label.as_str()).collect();
        assert_eq!(labels, ["0–9", "A – H", "I – P", "Q – Z"]);
        assert_eq!(view, [4, 0, 1, 3, 2]);
        assert_eq!(g[1], Group { label: "A – H".into(), start: 1, len: 2 });
    }

    #[test]
    fn batch_rename_names() {
        let items = vec![("a.jpg".to_string(), false), ("b.PNG".to_string(), false), ("dir".to_string(), true)];
        assert_eq!(batch_names("Trip.jpg", &items), ["Trip (1).jpg", "Trip (2).PNG", "Trip (3)"]);
        assert_eq!(batch_names("Trip", &items)[0], "Trip (1).jpg");
    }
}
