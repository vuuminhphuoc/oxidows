//! Turning a sender's file name into a safe path under the receive folder.

use std::path::{Path, PathBuf};

const ILLEGAL: &[char] = &['<', '>', ':', '"', '\\', '|', '?', '*'];
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2",
    "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// One path component as Windows accepts it: no reserved characters or device names, no
/// trailing dots or spaces, at most 255 UTF-16 units.
pub fn sanitize_component(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_control() || ILLEGAL.contains(&c) { '_' } else { c }).collect();
    while out.ends_with(['.', ' ']) {
        out.pop();
    }
    let out = out.trim_start().to_string();
    let stem = out.split('.').next().unwrap_or("").to_ascii_lowercase();
    let mut out = if RESERVED.contains(&stem.as_str()) { format!("_{out}") } else { out };
    while out.encode_utf16().count() > 255 {
        // Keep the extension when shortening.
        match out.rfind('.') {
            Some(dot) if dot > 0 && out.len() - dot <= 16 => {
                let ext = out[dot..].to_string();
                let mut stem = out[..dot].to_string();
                stem.pop();
                out = stem + &ext;
            }
            _ => {
                out.pop();
            }
        }
    }
    out
}

/// `name` may carry folders ("Album/IMG_1.jpg") when a folder is sent. Every part is cleaned, and
/// `.`/`..`/empty parts are dropped, so the result always stays inside `root`.
pub fn safe_relative(name: &str) -> PathBuf {
    let mut p = PathBuf::new();
    for part in name.split(['/', '\\']) {
        let part = part.trim();
        if part.is_empty() || part == "." || part == ".." {
            continue;
        }
        let c = sanitize_component(part);
        if !c.is_empty() {
            p.push(c);
        }
    }
    if p.as_os_str().is_empty() {
        p.push("file");
    }
    p
}

/// `path`, or `name (1).ext`, `name (2).ext`... for the first that does not exist yet and is not in
/// `taken` (files of the same transfer still in flight).
pub fn unique_path(path: &Path, taken: &dyn Fn(&Path) -> bool) -> PathBuf {
    if !path.exists() && !taken(path) {
        return path.to_path_buf();
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    for n in 1.. {
        let cand = path.with_file_name(format!("{stem} ({n}){ext}"));
        if !cand.exists() && !taken(&cand) {
            return cand;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_inside_the_folder() {
        assert_eq!(safe_relative("../../Windows/system32/x.dll"), PathBuf::from("Windows/system32/x.dll"));
        assert_eq!(safe_relative("C:\\boot.ini"), PathBuf::from("C_/boot.ini"));
        assert_eq!(safe_relative("/etc/passwd"), PathBuf::from("etc/passwd"));
        assert_eq!(safe_relative(".."), PathBuf::from("file"));
        assert_eq!(safe_relative("Album/Ảnh 1.jpg"), PathBuf::from("Album/Ảnh 1.jpg"));
    }

    #[test]
    fn windows_rules() {
        assert_eq!(sanitize_component("a:b?.txt"), "a_b_.txt");
        assert_eq!(sanitize_component("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_component("name. . "), "name");
        let long = format!("{}.jpeg", "x".repeat(300));
        let s = sanitize_component(&long);
        assert!(s.ends_with(".jpeg") && s.encode_utf16().count() == 255);
    }

    #[test]
    fn numbers_duplicates() {
        let dir = std::env::temp_dir().join(format!("oxi-share-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "").unwrap();
        let taken = |p: &Path| p.ends_with("a (1).txt");
        assert_eq!(unique_path(&dir.join("a.txt"), &taken), dir.join("a (2).txt"));
        assert_eq!(unique_path(&dir.join("b"), &|_| false), dir.join("b"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
