#![forbid(unsafe_code)]

//! Platform-independent part of the File Explorer.
//!
//! `explorer-sys` fills [`Entry`] values straight from the OS (one `FindFirstFileExW` pass on Windows,
//! no extra stat per file). Everything here — sorting, filtering, size/date/type text, the
//! back/forward history — touches no OS API and is unit-tested on any platform.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub mod query;
pub use query::{DateFilter, Kind, Query, SearchFilters, SizeFilter};

/// Optional Details columns read from the Windows property system (per file, in the
/// background), in the column chooser's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Prop {
    /// Cloud (OneDrive) availability; worked out from the attributes, not read.
    Status,
    Authors,
    Title,
    Tags,
    Rating,
    Dimensions,
    Length,
    DateTaken,
    Owner,
    CameraModel,
    Album,
    Year,
    BitRate,
    Pages,
    Comment,
}

impl Prop {
    pub const ALL: [Prop; 15] = [
        Prop::Status,
        Prop::Authors,
        Prop::Title,
        Prop::Tags,
        Prop::Rating,
        Prop::Dimensions,
        Prop::Length,
        Prop::DateTaken,
        Prop::Owner,
        Prop::CameraModel,
        Prop::Album,
        Prop::Year,
        Prop::BitRate,
        Prop::Pages,
        Prop::Comment,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Prop::Status => "Status",
            Prop::Authors => "Authors",
            Prop::Title => "Title",
            Prop::Tags => "Tags",
            Prop::Rating => "Rating",
            Prop::Dimensions => "Dimensions",
            Prop::Length => "Length",
            Prop::DateTaken => "Date taken",
            Prop::Owner => "Owner",
            Prop::CameraModel => "Camera model",
            Prop::Album => "Album",
            Prop::Year => "Year",
            Prop::BitRate => "Bit rate",
            Prop::Pages => "Pages",
            Prop::Comment => "Comments",
        }
    }

    /// Name in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            Prop::Status => "status",
            Prop::Authors => "authors",
            Prop::Title => "title",
            Prop::Tags => "tags",
            Prop::Rating => "rating",
            Prop::Dimensions => "dimensions",
            Prop::Length => "length",
            Prop::DateTaken => "datetaken",
            Prop::Owner => "owner",
            Prop::CameraModel => "cameramodel",
            Prop::Album => "album",
            Prop::Year => "year",
            Prop::BitRate => "bitrate",
            Prop::Pages => "pages",
            Prop::Comment => "comment",
        }
    }

    pub fn parse(s: &str) -> Option<Prop> {
        Prop::ALL.into_iter().find(|p| p.key() == s.trim())
    }

    pub fn is_numeric(self) -> bool {
        matches!(self, Prop::Rating | Prop::Length | Prop::Year | Prop::BitRate | Prop::Pages)
    }
}

/// Maps a Windows property store rating (0..99) to stars (0..5).
pub fn rating_to_stars(rating: u32) -> u32 {
    match rating {
        0 => 0,
        1..=12 => 1,
        13..=37 => 2,
        38..=62 => 3,
        63..=87 => 4,
        _ => 5,
    }
}

/// Maps stars (1..5) to Windows property store rating (1, 25, 50, 75, 99). 0 clears.
pub fn stars_to_rating(stars: u32) -> u32 {
    match stars {
        1 => 1,
        2 => 25,
        3 => 50,
        4 => 75,
        5 => 99,
        _ => 0,
    }
}

/// Splits a semicolon-separated tags string into individual tags.
pub fn parse_tags(text: &str) -> Vec<String> {
    text.split(';').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// Formats a list of tags as semicolon-separated text.
pub fn format_tags(tags: &[String]) -> String {
    tags.join("; ")
}

/// Compares two property values: numerically where numeric, otherwise case-insensitively.
pub fn cmp_prop_value(p: Prop, a: &str, b: &str) -> Ordering {
    let a = a.trim();
    let b = b.trim();
    if a.is_empty() && b.is_empty() {
        return Ordering::Equal;
    }
    if a.is_empty() {
        return Ordering::Greater;
    }
    if b.is_empty() {
        return Ordering::Less;
    }
    if p.is_numeric() {
        let num_a = a.parse::<f64>().ok().or_else(|| {
            let s: String = a.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            s.parse::<f64>().ok()
        });
        let num_b = b.parse::<f64>().ok().or_else(|| {
            let s: String = b.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            s.parse::<f64>().ok()
        });
        if let (Some(na), Some(nb)) = (num_a, num_b) {
            let ord = na.partial_cmp(&nb).unwrap_or(Ordering::Equal);
            if ord != Ordering::Equal {
                return ord;
            }
        }
    }
    natural_cmp(a, b)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NameBucket {
    Digits,
    AH,
    IP,
    QZ,
    Others,
}

impl NameBucket {
    pub const ALL: [NameBucket; 5] =
        [NameBucket::Digits, NameBucket::AH, NameBucket::IP, NameBucket::QZ, NameBucket::Others];

    pub fn label(self) -> &'static str {
        match self {
            NameBucket::Digits => "0-9",
            NameBucket::AH => "A-H",
            NameBucket::IP => "I-P",
            NameBucket::QZ => "Q-Z",
            NameBucket::Others => "Others",
        }
    }
}

pub fn name_bucket(name: &str) -> NameBucket {
    let Some(first) = name.chars().next() else {
        return NameBucket::Others;
    };
    if first.is_ascii_digit() {
        NameBucket::Digits
    } else {
        match first.to_ascii_uppercase() {
            'A'..='H' => NameBucket::AH,
            'I'..='P' => NameBucket::IP,
            'Q'..='Z' => NameBucket::QZ,
            _ => NameBucket::Others,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DateModifiedBucket {
    Today,
    Yesterday,
    EarlierThisWeek,
    LastWeek,
    EarlierThisMonth,
    LastMonth,
    EarlierThisYear,
    LongAgo,
}

impl DateModifiedBucket {
    pub const ALL: [DateModifiedBucket; 8] = [
        DateModifiedBucket::Today,
        DateModifiedBucket::Yesterday,
        DateModifiedBucket::EarlierThisWeek,
        DateModifiedBucket::LastWeek,
        DateModifiedBucket::EarlierThisMonth,
        DateModifiedBucket::LastMonth,
        DateModifiedBucket::EarlierThisYear,
        DateModifiedBucket::LongAgo,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DateModifiedBucket::Today => "Today",
            DateModifiedBucket::Yesterday => "Yesterday",
            DateModifiedBucket::EarlierThisWeek => "Earlier this week",
            DateModifiedBucket::LastWeek => "Last week",
            DateModifiedBucket::EarlierThisMonth => "Earlier this month",
            DateModifiedBucket::LastMonth => "Last month",
            DateModifiedBucket::EarlierThisYear => "Earlier this year",
            DateModifiedBucket::LongAgo => "A long time ago",
        }
    }
}

pub fn date_modified_bucket(unix_secs: i64, now: i64, utc_offset: i64) -> DateModifiedBucket {
    if unix_secs <= 0 {
        return DateModifiedBucket::LongAgo;
    }
    let day = |t: i64| (t + utc_offset).div_euclid(86_400);
    let (today, d) = (day(now), day(unix_secs));
    if d >= today {
        DateModifiedBucket::Today
    } else if d == today - 1 {
        DateModifiedBucket::Yesterday
    } else {
        let weekday = (today + 3).rem_euclid(7);
        let week_start = today - weekday;
        if d >= week_start {
            DateModifiedBucket::EarlierThisWeek
        } else if d >= week_start - 7 {
            DateModifiedBucket::LastWeek
        } else {
            let (y, m, _) = civil_from_days(today);
            let (dy, dm, _) = civil_from_days(d);
            if dy == y && dm == m {
                DateModifiedBucket::EarlierThisMonth
            } else if (dy == y && dm + 1 == m) || (dy + 1 == y && m == 1 && dm == 12) {
                DateModifiedBucket::LastMonth
            } else if dy == y {
                DateModifiedBucket::EarlierThisYear
            } else {
                DateModifiedBucket::LongAgo
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HeaderSizeBucket {
    Empty,
    Tiny,
    Small,
    Medium,
    Large,
    Huge,
    Gigantic,
}

impl HeaderSizeBucket {
    pub const ALL: [HeaderSizeBucket; 7] = [
        HeaderSizeBucket::Empty,
        HeaderSizeBucket::Tiny,
        HeaderSizeBucket::Small,
        HeaderSizeBucket::Medium,
        HeaderSizeBucket::Large,
        HeaderSizeBucket::Huge,
        HeaderSizeBucket::Gigantic,
    ];

    pub fn label(self) -> &'static str {
        match self {
            HeaderSizeBucket::Empty => "Empty",
            HeaderSizeBucket::Tiny => "Tiny (0-16 KB)",
            HeaderSizeBucket::Small => "Small (16 KB-1 MB)",
            HeaderSizeBucket::Medium => "Medium (1-128 MB)",
            HeaderSizeBucket::Large => "Large (128 MB-1 GB)",
            HeaderSizeBucket::Huge => "Huge (1-4 GB)",
            HeaderSizeBucket::Gigantic => "Gigantic (>4 GB)",
        }
    }
}

pub fn header_size_bucket(size: u64) -> HeaderSizeBucket {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    match size {
        0 => HeaderSizeBucket::Empty,
        s if s <= 16 * KB => HeaderSizeBucket::Tiny,
        s if s <= MB => HeaderSizeBucket::Small,
        s if s <= 128 * MB => HeaderSizeBucket::Medium,
        s if s <= GB => HeaderSizeBucket::Large,
        s if s <= 4 * GB => HeaderSizeBucket::Huge,
        _ => HeaderSizeBucket::Gigantic,
    }
}

pub fn folder_type_buckets(entries: &[Entry]) -> (Vec<String>, bool) {
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for e in entries {
        if !e.is_dir {
            *counts.entry(e.type_name()).or_default() += 1;
        }
    }
    let mut list: Vec<(String, usize)> = counts.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| natural_cmp(&a.0, &b.0)));
    let has_others = list.len() > 12;
    let top: Vec<String> = list.into_iter().take(12).map(|(t, _)| t).collect();
    (top, has_others)
}

/// The Details columns that have a filter drop-down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterColumn {
    Name,
    Modified,
    Type,
    Size,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeaderFilters {
    pub names: HashSet<NameBucket>,
    pub dates: HashSet<DateModifiedBucket>,
    pub types: HashSet<String>,
    pub sizes: HashSet<HeaderSizeBucket>,
}

impl HeaderFilters {
    pub fn is_empty(&self) -> bool {
        self.names.is_empty() && self.dates.is_empty() && self.types.is_empty() && self.sizes.is_empty()
    }

    pub fn clear(&mut self) {
        self.names.clear();
        self.dates.clear();
        self.types.clear();
        self.sizes.clear();
    }

    /// Whether the column behind `kind` has any box checked.
    pub fn is_filtered(&self, kind: FilterColumn) -> bool {
        match kind {
            FilterColumn::Name => !self.names.is_empty(),
            FilterColumn::Modified => !self.dates.is_empty(),
            FilterColumn::Type => !self.types.is_empty(),
            FilterColumn::Size => !self.sizes.is_empty(),
        }
    }

    pub fn matches(&self, e: &Entry, now: i64, utc_offset: i64, top_types: &[String]) -> bool {
        if !self.names.is_empty() && !self.names.contains(&name_bucket(e.display())) {
            return false;
        }
        if !self.dates.is_empty() && !self.dates.contains(&date_modified_bucket(e.modified, now, utc_offset)) {
            return false;
        }
        if !self.types.is_empty() {
            if e.is_dir {
                return false;
            }
            let t = e.type_name();
            let is_top = top_types.iter().any(|tt| tt == &t);
            let ok = self.types.contains(&t) || (self.types.contains("Others") && !is_top);
            if !ok {
                return false;
            }
        }
        if !self.sizes.is_empty() {
            if e.is_dir {
                return false;
            }
            if !self.sizes.contains(&header_size_bucket(e.size)) {
                return false;
            }
        }
        true
    }
}

pub fn justified_layout(
    aspect_ratios: &[f32],
    container_width: f32,
    target_height: f32,
    gap: f32,
) -> Vec<Vec<(usize, f32)>> {
    if aspect_ratios.is_empty() || container_width <= 0.0 || target_height <= 0.0 {
        return Vec::new();
    }
    let mut rows: Vec<Vec<(usize, f32)>> = Vec::new();
    let mut row_start = 0;
    while row_start < aspect_ratios.len() {
        let mut row_end = row_start + 1;
        let mut sum_r: f32 = aspect_ratios[row_start].max(0.1);
        while row_end < aspect_ratios.len() {
            let next_r = aspect_ratios[row_end].max(0.1);
            let k = (row_end - row_start + 1) as f32;
            let total_natural = (sum_r + next_r) * target_height + (k - 1.0) * gap;
            if total_natural > container_width {
                let k_prev = (row_end - row_start) as f32;
                let h_prev = (container_width - (k_prev - 1.0) * gap).max(1.0) / sum_r;
                let h_with = (container_width - (k - 1.0) * gap).max(1.0) / (sum_r + next_r);
                if (h_prev - target_height).abs() <= (h_with - target_height).abs() {
                    break;
                } else {
                    sum_r += next_r;
                    row_end += 1;
                    break;
                }
            }
            sum_r += next_r;
            row_end += 1;
        }

        let is_last_row = row_end == aspect_ratios.len();
        let k = (row_end - row_start) as f32;
        let natural_width = sum_r * target_height + (k - 1.0) * gap;
        if is_last_row && natural_width < container_width {
            let row: Vec<(usize, f32)> = (row_start..row_end)
                .map(|i| {
                    let w = aspect_ratios[i].max(0.1) * target_height;
                    (i, w)
                })
                .collect();
            rows.push(row);
        } else {
            let avail_w = (container_width - (k - 1.0) * gap).max(1.0);
            let h = avail_w / sum_r;
            let mut row: Vec<(usize, f32)> = Vec::with_capacity(row_end - row_start);
            let mut allocated_w = 0.0;
            for (offset, &ar) in aspect_ratios[row_start..row_end - 1].iter().enumerate() {
                let i = row_start + offset;
                let w = (ar.max(0.1) * h).round();
                allocated_w += w;
                row.push((i, w));
            }
            let last_idx = row_end - 1;
            let last_w = (avail_w - allocated_w).max(1.0);
            row.push((last_idx, last_w));
            rows.push(row);
        }
        row_start = row_end;
    }
    rows
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimelineSegment {
    pub label: String,
    pub y_start: f32,
    pub y_end: f32,
    pub item_index: usize,
    pub count: usize,
}

pub fn timeline_positions(months: &[(&str, usize)], track_height: f32) -> Vec<TimelineSegment> {
    if months.is_empty() || track_height <= 0.0 {
        return Vec::new();
    }
    let total_items: usize = months.iter().map(|(_, c)| *c).sum();
    if total_items == 0 {
        return Vec::new();
    }
    let mut segments = Vec::with_capacity(months.len());
    let mut current_y = 0.0;
    let mut current_idx = 0;
    for (label, count) in months {
        let frac = *count as f32 / total_items as f32;
        let h = track_height * frac;
        let y_start = current_y;
        let y_end = current_y + h;
        segments.push(TimelineSegment {
            label: label.to_string(),
            y_start,
            y_end,
            item_index: current_idx,
            count: *count,
        });
        current_y = y_end;
        current_idx += count;
    }
    segments
}

pub fn timeline_hit(segments: &[TimelineSegment], y: f32) -> Option<&TimelineSegment> {
    if segments.is_empty() {
        return None;
    }
    if y <= segments[0].y_start {
        return segments.first();
    }
    if y >= segments.last().unwrap().y_end {
        return segments.last();
    }
    segments.iter().find(|s| y >= s.y_start && y <= s.y_end)
}

pub fn format_month_year(unix_secs: i64, utc_offset: i64) -> String {
    if unix_secs <= 0 {
        return "Unknown".to_string();
    }
    let days = (unix_secs + utc_offset).div_euclid(86_400);
    let (y, m, _) = civil_from_days(days);
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let month_name = if (1..=12).contains(&m) { MONTHS[(m - 1) as usize] } else { "Unknown" };
    format!("{month_name} {y}")
}

/// The Status column of a cloud-synced item (OneDrive and other Cloud Files providers), from
/// its attributes: online-only placeholders carry RECALL_ON_DATA_ACCESS / RECALL_ON_OPEN /
/// OFFLINE, "Always keep on this device" sets PINNED. `in_sync_root`: the item is inside a
/// synced folder, so an ordinary file there is "available on this device".
pub fn cloud_status(attrs: u32, in_sync_root: bool) -> &'static str {
    const RECALL_ON_OPEN: u32 = 0x4_0000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
    if attrs & attr::PINNED != 0 {
        "● Always available"
    } else if attrs & (RECALL_ON_DATA_ACCESS | RECALL_ON_OPEN | attr::OFFLINE) != 0 {
        "☁ Available when online"
    } else if in_sync_root {
        "✓ Available on this device"
    } else {
        ""
    }
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SortKey {
    #[default]
    Name,
    Modified,
    Type,
    Size,
    Created,
    Attributes,
    Prop(Prop),
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
            SortKey::Prop(_) => Ordering::Equal,
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
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
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

/// (year, month, day) to days since 1970-01-01; the inverse of `civil_from_days`.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
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
    pub fn clear(&mut self) {
        self.back.clear();
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

/// The virtual Home page: Quick access folders and recent files. Items are named by full path.
pub const HOME: &str = "::home";

/// The virtual This PC page: known folders and drives. Its items are named by full path.
pub const THIS_PC: &str = "::this-pc";

/// The Libraries page; `::libraries\\<name>` is one library, showing its folders merged.
/// Items are named by full path.
pub const LIBRARIES: &str = "::libraries";

/// The Gallery: pictures and videos from the Pictures library, newest first. Items are named
/// by full path.
pub const GALLERY: &str = "::gallery";

/// Prefix of a shell namespace item that isn't a file-system path: `::shell\\<parsing name>`
/// (a phone over MTP, Control Panel, a media server…). The shell lists, opens and copies
/// these; the app only shows them. Items in such a folder are named by full path too.
pub const SHELL: &str = "::shell";

/// The shell's parsing name of This PC, Network and Control Panel.
pub const THIS_PC_CLSID: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";
pub const NETWORK_CLSID: &str = "::{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}";
pub const CONTROL_PANEL_CLSID: &str = "::{26EE0668-A00A-44D7-9371-BEB064C98683}";

/// A parsing name split into its levels: the root (`::{CLSID}`, or a whole path), then one
/// item ID per level. A device ID such as `\\\\?\\usb#vid_…#{…}` stays one level.
pub fn shell_segments(parsing: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = parsing;
    let take = |rest: &mut &str, from: usize| -> String {
        let end = rest[from..].find('\\').map_or(rest.len(), |i| i + from);
        let seg = rest[..end].to_string();
        *rest = rest.get(end + 1..).unwrap_or("");
        seg
    };
    while !rest.is_empty() {
        let from = if rest.starts_with(r"\\?\") { 4 } else { 0 };
        let seg = take(&mut rest, from);
        if !seg.is_empty() {
            out.push(seg);
        }
    }
    out
}

/// The app path of the shell item `parsing`.
pub fn shell_path(parsing: &str) -> PathBuf {
    PathBuf::from(format!("{SHELL}\\{parsing}"))
}

/// The shell parsing name behind an app path: the part after `::shell\\`, or the path itself.
pub fn shell_parsing(p: &Path) -> std::borrow::Cow<'_, str> {
    let s = p.to_string_lossy();
    match s.strip_prefix(SHELL).and_then(|r| r.strip_prefix('\\')) {
        Some(rest) => std::borrow::Cow::Owned(rest.to_string()),
        None => s,
    }
}

/// What kind of location a path is, for the parts that are not plain folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// The list of computers.
    Network,
    RecycleBin,
    ThisPc,
    Home,
    Libraries,
    /// One library, by name.
    Library(String),
    Gallery,
    /// A shell namespace folder, by parsing name.
    Shell(String),
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
    if s == HOME {
        return Place::Home;
    }
    if s == LIBRARIES {
        return Place::Libraries;
    }
    if s == GALLERY {
        return Place::Gallery;
    }
    if let Some(rest) = s.strip_prefix(SHELL).and_then(|r| r.strip_prefix('\\')).filter(|r| !r.is_empty()) {
        return Place::Shell(rest.to_string());
    }
    if let Some(name) = s.strip_prefix(LIBRARIES).and_then(|r| r.strip_prefix(['\\', '/'])).filter(|n| !n.is_empty()) {
        return Place::Library(name.to_string());
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

impl Place {
    /// A page made up by the app rather than a folder on disk (or a share): nothing can be
    /// created, renamed or pasted in it, and it isn't watched.
    pub fn is_page(&self) -> bool {
        !matches!(self, Place::Unc { .. } | Place::Local)
    }

    /// A page whose items are named by their full path.
    pub fn names_are_paths(&self) -> bool {
        matches!(
            self,
            Place::ThisPc | Place::Home | Place::Libraries | Place::Library(_) | Place::Gallery | Place::Shell(_)
        )
    }
}

/// Whether `p` starts with a drive letter or `\\?\` prefix (Windows paths).
fn has_drive(p: &Path) -> bool {
    matches!(p.components().next(), Some(std::path::Component::Prefix(_)))
}

/// Path of the item `name` inside `cur` (handles the virtual Network and server levels).
pub fn child_path(cur: &Path, name: &str) -> PathBuf {
    // Shell items carry their whole path wherever they are listed.
    if name.starts_with(SHELL) {
        return PathBuf::from(name);
    }
    match classify(cur) {
        Place::Network => PathBuf::from(format!(r"\\{name}")),
        p if p.names_are_paths() => PathBuf::from(name),
        Place::Server(server) => PathBuf::from(format!(r"\\{server}\{name}")),
        _ => cur.join(name),
    }
}

/// Normalized comparison key for a path (case-insensitive on Windows, trims trailing slashes).
pub fn path_key(p: &Path) -> String {
    path_key_with(p, cfg!(windows))
}

pub fn path_key_with(p: &Path, case_insensitive: bool) -> String {
    let s = p.to_string_lossy();
    let trimmed = s.trim_end_matches(['\\', '/']);
    let base = if trimmed.is_empty() && !s.is_empty() { "/" } else { trimmed };
    if case_insensitive { base.to_lowercase() } else { base.to_string() }
}

pub fn path_eq(a: &Path, b: &Path) -> bool {
    path_eq_with(a, b, cfg!(windows))
}

pub fn path_eq_with(a: &Path, b: &Path, case_insensitive: bool) -> bool {
    let mut ac = a.components();
    let mut bc = b.components();
    loop {
        match (ac.next(), bc.next()) {
            (None, None) => return true,
            (Some(x), Some(y)) => {
                let xs = x.as_os_str().to_string_lossy();
                let ys = y.as_os_str().to_string_lossy();
                let same = if case_insensitive { xs.to_lowercase() == ys.to_lowercase() } else { xs == ys };
                if !same {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

/// True when `ancestor` is a strict parent/ancestor of `target` by path components.
pub fn path_is_ancestor(ancestor: &Path, target: &Path) -> bool {
    path_is_ancestor_with(ancestor, target, cfg!(windows))
}

pub fn path_is_ancestor_with(ancestor: &Path, target: &Path, case_insensitive: bool) -> bool {
    let mut ac = ancestor.components();
    let mut tc = target.components();
    for x in &mut ac {
        let Some(y) = tc.next() else { return false };
        let xs = x.as_os_str().to_string_lossy();
        let ys = y.as_os_str().to_string_lossy();
        let same = if case_insensitive { xs.to_lowercase() == ys.to_lowercase() } else { xs == ys };
        if !same {
            return false;
        }
    }
    tc.next().is_some()
}

/// Ranks frequent folders: most opens first, tie-broken by most recently opened (highest timestamp).
pub fn rank_frequent_folders<I>(items: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = (PathBuf, u64, i64)>,
{
    let mut vec: Vec<(PathBuf, u64, i64)> = items.into_iter().collect();
    vec.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.2.cmp(&a.2)).then_with(|| a.0.cmp(&b.0)));
    vec.into_iter().map(|(p, _, _)| p).collect()
}

/// One level up: share root -> server -> Network; drive root -> none.
pub fn parent_path(cur: &Path) -> Option<PathBuf> {
    match classify(cur) {
        Place::Network | Place::RecycleBin | Place::ThisPc | Place::Home | Place::Libraries | Place::Gallery => None,
        Place::Library(_) => Some(PathBuf::from(LIBRARIES)),
        Place::Shell(parsing) => {
            let segs = shell_segments(&parsing);
            match segs.len() {
                0 => None,
                1 => Some(PathBuf::from(THIS_PC)),
                2 if segs[0].eq_ignore_ascii_case(THIS_PC_CLSID) => Some(PathBuf::from(THIS_PC)),
                2 if segs[0].eq_ignore_ascii_case(NETWORK_CLSID) => Some(PathBuf::from(NETWORK)),
                n => Some(shell_path(&segs[..n - 1].join("\\"))),
            }
        }
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
        Place::Home => vec![("Home".into(), PathBuf::from(HOME))],
        Place::Gallery => vec![("Gallery".into(), PathBuf::from(GALLERY))],
        Place::Libraries => vec![("Libraries".into(), PathBuf::from(LIBRARIES))],
        // Labels are the raw IDs; the app swaps in the display names it has seen.
        Place::Shell(parsing) => {
            let segs = shell_segments(&parsing);
            let mut out = Vec::new();
            for i in 0..segs.len() {
                let path = shell_path(&segs[..=i].join("\\"));
                match segs[0].to_ascii_uppercase() {
                    r if i == 0 && r == THIS_PC_CLSID => out.push(("This PC".into(), PathBuf::from(THIS_PC))),
                    r if i == 0 && r == NETWORK_CLSID => out.push(("Network".into(), PathBuf::from(NETWORK))),
                    r if i == 0 && r == CONTROL_PANEL_CLSID => out.push(("Control Panel".into(), path)),
                    _ => out.push((segs[i].clone(), path)),
                }
            }
            out
        }
        Place::Library(name) => vec![
            ("Libraries".into(), PathBuf::from(LIBRARIES)),
            (name.clone(), PathBuf::from(format!("{LIBRARIES}\\{name}"))),
        ],
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

/// `ZoneId` from a `Zone.Identifier` stream (`[ZoneTransfer]` / `ZoneId=3` / …).
pub fn parse_zone_id(text: &str) -> Option<u32> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        k.trim().eq_ignore_ascii_case("ZoneId").then(|| v.trim().parse().ok()).flatten()
    })
}

/// Windows Explorer no longer previews files marked as downloaded from the Internet or the
/// Restricted zone (a preview handler can be made to leak NTLM credentials); it shows this text.
pub fn preview_blocked(zone: Option<u32>) -> bool {
    zone.is_some_and(|z| z >= 3)
}

pub const PREVIEW_BLOCKED_TEXT: &str = "This file came from the internet and could harm your computer. If you trust it and its source, open it to see its contents.";

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
        "home" => HOME,
        "libraries" => LIBRARIES,
        "controlpanelfolder" | "controlpanel" => "::shell\\::{26EE0668-A00A-44D7-9371-BEB064C98683}",
        "gallery" => GALLERY,
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

    #[test]
    fn zone_identifier() {
        let t = "[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl=https://example.com/\r\n";
        assert_eq!(parse_zone_id(t), Some(3));
        assert!(preview_blocked(parse_zone_id(t)));
        assert!(!preview_blocked(parse_zone_id("[ZoneTransfer]\nZoneId=2")));
        assert!(!preview_blocked(parse_zone_id("")));
    }

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

    #[test]
    fn library_pages() {
        let lib = Path::new(r"::libraries\Documents");
        assert_eq!(classify(lib), Place::Library("Documents".into()));
        assert_eq!(classify(Path::new(LIBRARIES)), Place::Libraries);
        assert_eq!(classify(Path::new(GALLERY)), Place::Gallery);
        assert_eq!(parent_path(lib), Some(PathBuf::from(LIBRARIES)));
        assert_eq!(parent_path(Path::new(GALLERY)), None);
        assert_eq!(location_crumbs(lib).len(), 2);
        assert_eq!(child_path(lib, r"C:\Users\a\Documents\x.txt"), PathBuf::from(r"C:\Users\a\Documents\x.txt"));
        assert!(Place::Gallery.is_page() && !Place::Local.is_page());
        assert_eq!(shell_folder_template("libraries"), Some(LIBRARIES));
    }

    #[test]
    fn cloud_status_from_attributes() {
        assert_eq!(cloud_status(attr::ARCHIVE | 0x40_0000, true), "☁ Available when online");
        assert_eq!(cloud_status(attr::ARCHIVE | attr::PINNED, true), "● Always available");
        assert_eq!(cloud_status(attr::ARCHIVE, true), "✓ Available on this device");
        assert_eq!(cloud_status(attr::ARCHIVE, false), "");
        assert_eq!(Prop::parse("status"), Some(Prop::Status));
    }

    #[test]
    fn shell_namespace_paths() {
        let phone = r"::{20D04FE0-3AEA-1069-A2D8-08002B30309D}\\\?\usb#vid_04e8&pid_6860#r58m#{6ac27878-a6fa-4155-ba85-f98f491d4f33}\SID-{10001,,1}\{0001}";
        let segs = shell_segments(phone);
        assert_eq!(segs.len(), 4, "{segs:?}");
        assert!(segs[1].starts_with(r"\\?\usb#") && segs[1].ends_with('}'));
        let p = shell_path(phone);
        assert_eq!(classify(&p), Place::Shell(phone.into()));
        assert_eq!(shell_parsing(&p), phone);
        assert_eq!(shell_parsing(Path::new(r"C:\x")), r"C:\x");
        let up = parent_path(&p).unwrap();
        assert_eq!(shell_parsing(&up), segs[..3].join("\\"));
        let device = shell_path(&segs[..2].join("\\"));
        assert_eq!(parent_path(&device), Some(PathBuf::from(THIS_PC)));
        let crumbs = location_crumbs(&p);
        assert_eq!(crumbs[0], ("This PC".to_string(), PathBuf::from(THIS_PC)));
        assert_eq!(crumbs.len(), 4);
        assert_eq!(child_path(&p, &shell_path("x").to_string_lossy()), shell_path("x"));
        assert!(Place::Shell("x".into()).is_page());
        assert_eq!(
            shell_folder_template("controlpanel").map(|t| classify(Path::new(t))),
            Some(Place::Shell(CONTROL_PANEL_CLSID.into()))
        );
    }

    #[test]
    fn path_ancestor_and_equality() {
        let p = Path::new;
        // Forward slashes parse as separators on every platform, so this runs on Linux too.
        assert!(path_eq_with(p("/Users/Alice/"), p("/users/alice"), true));
        assert!(!path_eq_with(p("/Users/Alice"), p("/users/alice"), false));
        assert!(path_is_ancestor_with(p("/Users"), p("/users/Alice/Docs"), true));
        assert!(!path_is_ancestor_with(p("/Users"), p("/Users"), true));
        assert!(!path_is_ancestor_with(p("/User"), p("/Users/Alice"), true));
        assert!(path_is_ancestor_with(p("/"), p("/home/alice"), false));
        assert_eq!(path_key_with(p("/"), false), "/");
        assert_eq!(path_key_with(p(r"C:\Foo\"), true), r"c:\foo");
    }

    #[cfg(windows)]
    #[test]
    fn path_ancestor_and_equality_with_drives() {
        let p = Path::new;
        assert!(path_eq_with(p(r"C:\Users\Alice\"), p(r"c:\users\alice"), true));
        assert!(path_is_ancestor_with(p(r"C:\Users"), p(r"c:\users\Alice\Docs"), true));
        assert!(!path_is_ancestor_with(p(r"C:\Users"), p(r"C:\Users"), true));
        assert!(!path_is_ancestor_with(p(r"C:\User"), p(r"C:\Users\Alice"), true));
    }

    #[test]
    fn frequent_folder_ranking() {
        let a = PathBuf::from("/home/user/docs");
        let b = PathBuf::from("/home/user/downloads");
        let c = PathBuf::from("/home/user/music");
        let d = PathBuf::from("/home/user/videos");
        // a: 10 opens, opened at 100
        // b: 5 opens, opened at 300
        // c: 5 opens, opened at 200 (less recent than b)
        // d: 1 open, opened at 500
        let items = vec![(d.clone(), 1, 500), (c.clone(), 5, 200), (a.clone(), 10, 100), (b.clone(), 5, 300)];
        let ranked = rank_frequent_folders(items);
        assert_eq!(ranked, vec![a, b, c, d]);
    }

    #[test]
    fn name_buckets() {
        assert_eq!(name_bucket("0file.txt"), NameBucket::Digits);
        assert_eq!(name_bucket("9items"), NameBucket::Digits);
        assert_eq!(name_bucket("apple.jpg"), NameBucket::AH);
        assert_eq!(name_bucket("Hat.png"), NameBucket::AH);
        assert_eq!(name_bucket("image.png"), NameBucket::IP);
        assert_eq!(name_bucket("Photos"), NameBucket::IP);
        assert_eq!(name_bucket("quick.rs"), NameBucket::QZ);
        assert_eq!(name_bucket("Zoo"), NameBucket::QZ);
        assert_eq!(name_bucket("_hidden"), NameBucket::Others);
        assert_eq!(name_bucket(""), NameBucket::Others);
    }

    #[test]
    fn date_modified_buckets() {
        // Wednesday 2026-11-25 12:00 UTC (week started Monday 2026-11-23).
        let now = days_from_civil(2026, 11, 25) * 86_400 + 12 * 3600;
        let day_sec = |y, m, d| days_from_civil(y, m, d) * 86_400 + 3600;

        assert_eq!(date_modified_bucket(day_sec(2026, 11, 25), now, 0), DateModifiedBucket::Today);
        assert_eq!(date_modified_bucket(day_sec(2026, 11, 24), now, 0), DateModifiedBucket::Yesterday);
        assert_eq!(date_modified_bucket(day_sec(2026, 11, 23), now, 0), DateModifiedBucket::EarlierThisWeek);
        assert_eq!(date_modified_bucket(day_sec(2026, 11, 20), now, 0), DateModifiedBucket::LastWeek);
        assert_eq!(date_modified_bucket(day_sec(2026, 11, 10), now, 0), DateModifiedBucket::EarlierThisMonth);
        assert_eq!(date_modified_bucket(day_sec(2026, 10, 15), now, 0), DateModifiedBucket::LastMonth);
        assert_eq!(date_modified_bucket(day_sec(2026, 5, 1), now, 0), DateModifiedBucket::EarlierThisYear);
        assert_eq!(date_modified_bucket(day_sec(2025, 11, 25), now, 0), DateModifiedBucket::LongAgo);
        assert_eq!(date_modified_bucket(0, now, 0), DateModifiedBucket::LongAgo);
    }

    #[test]
    fn header_size_buckets() {
        assert_eq!(header_size_bucket(0), HeaderSizeBucket::Empty);
        assert_eq!(header_size_bucket(1024), HeaderSizeBucket::Tiny);
        assert_eq!(header_size_bucket(16 * 1024), HeaderSizeBucket::Tiny);
        assert_eq!(header_size_bucket(16 * 1024 + 1), HeaderSizeBucket::Small);
        assert_eq!(header_size_bucket(1024 * 1024), HeaderSizeBucket::Small);
        assert_eq!(header_size_bucket(1024 * 1024 + 1), HeaderSizeBucket::Medium);
        assert_eq!(header_size_bucket(128 * 1024 * 1024), HeaderSizeBucket::Medium);
        assert_eq!(header_size_bucket(128 * 1024 * 1024 + 1), HeaderSizeBucket::Large);
        assert_eq!(header_size_bucket(1024 * 1024 * 1024), HeaderSizeBucket::Large);
        assert_eq!(header_size_bucket(1024 * 1024 * 1024 + 1), HeaderSizeBucket::Huge);
        assert_eq!(header_size_bucket(4 * 1024 * 1024 * 1024), HeaderSizeBucket::Huge);
        assert_eq!(header_size_bucket(4 * 1024 * 1024 * 1024 + 1), HeaderSizeBucket::Gigantic);
    }

    #[test]
    fn rating_mapping_and_tags() {
        assert_eq!(stars_to_rating(0), 0);
        assert_eq!(stars_to_rating(1), 1);
        assert_eq!(stars_to_rating(2), 25);
        assert_eq!(stars_to_rating(3), 50);
        assert_eq!(stars_to_rating(4), 75);
        assert_eq!(stars_to_rating(5), 99);

        assert_eq!(rating_to_stars(0), 0);
        assert_eq!(rating_to_stars(1), 1);
        assert_eq!(rating_to_stars(12), 1);
        assert_eq!(rating_to_stars(25), 2);
        assert_eq!(rating_to_stars(50), 3);
        assert_eq!(rating_to_stars(75), 4);
        assert_eq!(rating_to_stars(99), 5);

        let tags = parse_tags("vacation; family; 2026; ; scenic ");
        assert_eq!(tags, vec!["vacation", "family", "2026", "scenic"]);
        assert_eq!(format_tags(&tags), "vacation; family; 2026; scenic");

        assert_eq!(cmp_prop_value(Prop::Rating, "50", "25"), Ordering::Greater);
        assert_eq!(cmp_prop_value(Prop::Pages, "12", "120"), Ordering::Less);
        assert_eq!(cmp_prop_value(Prop::Title, "Beta", "alpha"), Ordering::Greater);
    }

    #[test]
    fn header_filters_rule() {
        let now = days_from_civil(2026, 11, 25) * 86_400 + 12 * 3600;
        let day_sec = |y, m, d| days_from_civil(y, m, d) * 86_400 + 3600;

        let file = Entry {
            name: "apple.txt".into(),
            is_dir: false,
            size: 500,
            modified: day_sec(2026, 11, 25),
            ..Default::default()
        };
        let folder = Entry {
            name: "Archive".into(),
            is_dir: true,
            size: 0,
            modified: day_sec(2026, 11, 25),
            ..Default::default()
        };

        let mut f = HeaderFilters::default();
        assert!(f.matches(&file, now, 0, &[]));
        assert!(f.matches(&folder, now, 0, &[]));

        // Name filter applies to all items
        f.names.insert(NameBucket::AH);
        assert!(f.matches(&file, now, 0, &[]));
        assert!(f.matches(&folder, now, 0, &[]));

        f.names.clear();
        f.names.insert(NameBucket::QZ);
        assert!(!f.matches(&file, now, 0, &[]));
        assert!(!f.matches(&folder, now, 0, &[]));

        // Size filter applies to files only (folder is excluded)
        f.names.clear();
        f.sizes.insert(HeaderSizeBucket::Tiny);
        assert!(f.matches(&file, now, 0, &[]));
        assert!(!f.matches(&folder, now, 0, &[]));

        // Type filter applies to files only
        f.sizes.clear();
        f.types.insert("Text Document".into());
        assert!(f.matches(&file, now, 0, &["Text Document".into()]));
        assert!(!f.matches(&folder, now, 0, &["Text Document".into()]));
    }

    #[test]
    fn justified_layout_rows() {
        let aspect_ratios = vec![1.0, 1.5, 1.0, 1.0, 1.5, 1.0];
        let rows = justified_layout(&aspect_ratios, 1000.0, 160.0, 4.0);
        assert_eq!(rows.len(), 2);
        // Row 1 fills 1000.0 width
        let row1_total: f32 = rows[0].iter().map(|(_, w)| *w).sum::<f32>() + (rows[0].len() - 1) as f32 * 4.0;
        assert!((row1_total - 1000.0).abs() < 1.0, "row 1 width {row1_total}");
        // Row 2 is left-aligned at target height 160.0 * 1.0 = 160.0
        assert_eq!(rows[1].len(), 1);
        assert_eq!(rows[1][0].1, 160.0);
    }

    #[test]
    fn timeline_scrubber_mapping() {
        let months = vec![("November 2026", 10), ("October 2026", 30)];
        let segs = timeline_positions(&months, 100.0);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].label, "November 2026");
        assert_eq!(segs[0].y_start, 0.0);
        assert_eq!(segs[0].y_end, 25.0);
        assert_eq!(segs[1].y_start, 25.0);
        assert_eq!(segs[1].y_end, 100.0);

        assert_eq!(timeline_hit(&segs, 10.0).unwrap().label, "November 2026");
        assert_eq!(timeline_hit(&segs, 50.0).unwrap().label, "October 2026");
        assert_eq!(timeline_hit(&segs, -5.0).unwrap().label, "November 2026");
        assert_eq!(timeline_hit(&segs, 150.0).unwrap().label, "October 2026");
    }
}
