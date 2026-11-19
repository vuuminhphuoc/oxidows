//! "Record and diff" parity checks: the real Windows is the oracle.
//!
//! `explorer-core` re-implements, in plain Rust, text that Windows itself produces: the natural
//! sort order of names, the Size column, file times. Here the same inputs go through both the
//! Windows API and our code, and any difference fails CI. Edge cases (leading zeros, leap days,
//! sizes at the 1 KB boundary) are listed in the corpora below; to guard a new function, add a
//! corpus and a test. Runs on the `windows-latest` CI runner (its locale is en-US).
#![cfg(windows)]

use std::cmp::Ordering;

use explorer_core::{format_bytes, format_size_kb, format_time, natural_cmp};
use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows_sys::Win32::System::Time::FileTimeToSystemTime;
use windows_sys::Win32::UI::Shell::{StrCmpLogicalW, StrFormatByteSizeW, StrFormatKBSizeW};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// What Explorer itself uses to order names.
fn windows_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (wide(a), wide(b));
    // SAFETY: both buffers are NUL-terminated and outlive the call.
    unsafe { StrCmpLogicalW(a.as_ptr(), b.as_ptr()) }.cmp(&0)
}

fn buf_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Names made of letters, digits, spaces and dots: the part of the order that must match exactly.
const NAMES: &[&str] = &[
    "a",
    "A",
    "b",
    "B",
    "abc",
    "abd",
    "ab",
    "ab c",
    "1",
    "01",
    "001",
    "2",
    "10",
    "100",
    "a1",
    "a01",
    "a 1",
    "a.1",
    "file1",
    "File2",
    "file02",
    "file2",
    "file10",
    "file010",
    "file1.txt",
    "file10.txt",
    "file1a",
    "file1b",
    "IMG 0001.jpg",
    "IMG 0002.jpg",
    "IMG 0010.jpg",
    "IMG 10.jpg",
    "report.docx",
    "report 2.docx",
    "report 10.docx",
    "x9y10",
    "x9y9",
    "x10y1",
    "1 a",
    "1a",
    "12 345",
    "12345",
    "v1.2.3",
    "v1.10.0",
    "v1.2",
    "v1.2.10",
    ".hidden",
    "Z",
    "z",
    "z9",
    "Z10",
    "Zz",
];

/// Punctuation and non-ASCII: reported by `divergence_report`, not enforced (Windows' ordering of
/// these depends on the locale's collation tables).
const WIDE_NAMES: &[&str] = &[
    "a_1",
    "a-1",
    "_under",
    "~tmp",
    "(1)",
    "report (2).docx",
    "report (10).docx",
    "Report - final.docx",
    "é",
    "e",
    "f",
    "Ärger",
    "Zebra",
    "日本",
    "ñ",
    "n",
    "o",
    "ß",
    "ss",
];

fn mismatches(names: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for a in names {
        for b in names {
            let (ours, theirs) = (natural_cmp(a, b), windows_cmp(a, b));
            if ours != theirs {
                out.push(format!("{a:?} vs {b:?}: oxidows {ours:?}, Windows {theirs:?}"));
            }
        }
    }
    out
}

#[test]
fn natural_order_matches_explorer() {
    let bad = mismatches(NAMES);
    assert!(bad.is_empty(), "{} of {} pairs differ:\n{}", bad.len(), NAMES.len() * NAMES.len(), bad.join("\n"));
}

#[test]
fn size_column_matches_explorer() {
    let sizes: Vec<u64> = vec![
        0,
        1,
        2,
        1023,
        1024,
        1025,
        2047,
        2048,
        1_000_000,
        1_048_575,
        1_048_576,
        4_294_967_295,
        4_294_967_296,
        1 << 40,
        123_456_789_012,
        1 << 50, // not i64::MAX: Windows prints that one as "(9,007,199,254,740,991) KB" (f64 overflow)
    ];
    let mut bad = Vec::new();
    for n in sizes {
        let mut buf = [0u16; 64];
        // SAFETY: `buf` is writable for 64 u16 and the length passed is 64.
        unsafe { StrFormatKBSizeW(n as i64, buf.as_mut_ptr(), buf.len() as u32) };
        let theirs = buf_to_string(&buf);
        let ours = format_size_kb(n);
        if ours != theirs {
            bad.push(format!("{n} bytes: oxidows {ours:?}, Windows {theirs:?}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// Dates around leap days and century rules, then a sweep over 1970-2600.
#[test]
fn file_time_text_matches_windows() {
    let mut secs: Vec<i64> = vec![
        1,
        86_399,
        86_400,
        78_796_800,      // 1972-02-29
        951_782_399,     // 2000-02-28 23:59:59
        951_782_400,     // 2000-02-29 00:00
        1_582_934_400,   // 2020-02-29
        2_147_483_647,   // i32::MAX
        2_147_483_648,   // 2038-01-19 03:14:08
        4_102_444_800,   // 2100-01-01
        4_107_542_400,   // 2100-03-01 (2100 is not a leap year)
        32_503_680_000,  // 3000-01-01
        253_402_300_799, // 9999-12-31 23:59:59
    ];
    let mut t = 1;
    while t < 20_000_000_000 {
        secs.push(t);
        t += 3 * 86_400 + 12_345;
    }
    let mut bad = Vec::new();
    for s in secs {
        // FILETIME counts 100 ns ticks since 1601-01-01.
        let ticks = (s as u64 + 11_644_473_600) * 10_000_000;
        let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
        // SAFETY: SYSTEMTIME is plain integers, so all-zero is valid; both pointers are valid.
        let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
        let ok = unsafe { FileTimeToSystemTime(&ft, &mut st) };
        assert_ne!(ok, 0, "FileTimeToSystemTime failed for {s}");
        let theirs = format!("{:04}-{:02}-{:02} {:02}:{:02}", st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute);
        let ours = format_time(s, 0);
        if ours != theirs {
            bad.push(format!("{s}: oxidows {ours:?}, Windows {theirs:?}"));
            if bad.len() >= 20 {
                break;
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// Never fails: prints where oxidows still differs from Windows, for the CI log
/// (`cargo test -p explorer-sys --test oracle divergence_report -- --nocapture`).
#[test]
fn divergence_report() {
    let all: Vec<&str> = NAMES.iter().chain(WIDE_NAMES).copied().collect();
    let bad = mismatches(&all);
    println!("natural order, punctuation and non-ASCII names: {} of {} pairs differ", bad.len(), all.len() * all.len());
    for line in bad.iter().take(40) {
        println!("  {line}");
    }

    let mut differ = 0;
    for n in [0u64, 1, 500, 1000, 1023, 1024, 1536, 10_240, 1_048_576, 1_500_000, 123_456_789, 5_000_000_000] {
        let mut buf = [0u16; 64];
        // SAFETY: `buf` is writable for 64 u16 and the length passed is 64.
        unsafe { StrFormatByteSizeW(n as i64, buf.as_mut_ptr(), buf.len() as u32) };
        let theirs = buf_to_string(&buf);
        let ours = format_bytes(n);
        if ours != theirs {
            differ += 1;
            println!("  size {n}: oxidows {ours:?}, Windows {theirs:?}");
        }
    }
    println!("short sizes (format_bytes): {differ} of 12 differ");
}
