//! Exercises the real Win32 layer. Runs on the `windows-latest` CI runner.
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use explorer_sys::{Entry, FileOp, file_op_silent, list_dir};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oxi-explorer-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn list(dir: &Path) -> Vec<Entry> {
    let mut all = Vec::new();
    list_dir(dir, &AtomicBool::new(false), &mut |b| all.extend(b)).unwrap();
    all.sort_by(|a, b| a.name.cmp(&b.name));
    all
}

fn names(dir: &Path) -> Vec<String> {
    list(dir).into_iter().map(|e| e.name).collect()
}

#[test]
fn lists_names_sizes_dirs_and_unicode() {
    let d = scratch("list");
    fs::write(d.join("a.txt"), vec![b'x'; 5000]).unwrap();
    fs::write(d.join("Tiếng Việt ✓.md"), b"hi").unwrap();
    fs::create_dir(d.join("sub")).unwrap();
    let e = list(&d);
    assert_eq!(e.len(), 3, "{e:?}");
    let a = e.iter().find(|x| x.name == "a.txt").unwrap();
    assert_eq!(a.size, 5000);
    assert!(!a.is_dir);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    assert!((a.modified - now).abs() < 600, "modified {} vs now {now}", a.modified);
    assert!(e.iter().any(|x| x.name == "Tiếng Việt ✓.md"));
    assert!(e.iter().find(|x| x.name == "sub").unwrap().is_dir);
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn hidden_attribute_is_read() {
    let d = scratch("hidden");
    let f = d.join("secret.txt");
    fs::write(&f, b"x").unwrap();
    let st = std::process::Command::new("attrib").arg("+h").arg(&f).status().unwrap();
    assert!(st.success());
    assert!(list(&d)[0].hidden);
    let _ = std::process::Command::new("attrib").arg("-h").arg(&f).status();
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn large_folder_arrives_in_batches() {
    let d = scratch("many");
    let n = explorer_sys::BATCH * 2 + 17;
    for i in 0..n {
        fs::write(d.join(format!("f{i}")), b"").unwrap();
    }
    let mut batches = 0;
    let mut total = 0;
    let t = Instant::now();
    list_dir(&d, &AtomicBool::new(false), &mut |b| {
        batches += 1;
        total += b.len();
    })
    .unwrap();
    println!("listed {n} files in {:?}", t.elapsed());
    assert_eq!(total, n);
    assert_eq!(batches, 3);
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn missing_folder_is_an_error() {
    assert!(list_dir(Path::new(r"C:\definitely\not\here"), &AtomicBool::new(false), &mut |_| {}).is_err());
}

#[test]
fn drives_and_known_folders() {
    let drives = explorer_sys::drives();
    let c = drives.iter().find(|d| d.root == Path::new(r"C:\")).expect("C: drive");
    assert!(c.total > 0 && c.free > 0 && c.free <= c.total, "{c:?}");
    let places = explorer_sys::known_folders();
    assert!(places.iter().any(|(n, _)| *n == "Documents" || *n == "Desktop"), "{places:?}");
    assert!(explorer_sys::home_dir().is_dir());
    let off = explorer_sys::utc_offset_secs();
    assert!(off.abs() <= 14 * 3600);
}

#[test]
fn copy_move_recycle_delete() {
    let d = scratch("ops");
    let (src, dst) = (d.join("src"), d.join("dst"));
    fs::create_dir_all(src.join("inner")).unwrap();
    fs::write(src.join("one.txt"), b"1").unwrap();
    fs::write(src.join("inner").join("two.txt"), b"22").unwrap();
    fs::create_dir(&dst).unwrap();

    let items = vec![src.join("one.txt"), src.join("inner")];
    assert!(file_op_silent(FileOp::Copy, &items, Some(&dst)).unwrap());
    assert_eq!(names(&dst), ["inner", "one.txt"]);
    assert_eq!(fs::read(dst.join("inner").join("two.txt")).unwrap(), b"22");

    // Copy into the same folder makes "one - Copy.txt" rather than failing.
    assert!(file_op_silent(FileOp::Copy, &[dst.join("one.txt")], Some(&dst)).unwrap());
    assert_eq!(names(&dst).len(), 3, "{:?}", names(&dst));

    let moved = d.join("moved");
    fs::create_dir(&moved).unwrap();
    assert!(file_op_silent(FileOp::Move, &items, Some(&moved)).unwrap());
    assert_eq!(names(&src), Vec::<String>::new());
    assert_eq!(names(&moved), ["inner", "one.txt"]);

    assert!(file_op_silent(FileOp::Recycle, &[moved.join("one.txt")], None).unwrap());
    assert!(!moved.join("one.txt").exists());
    assert!(file_op_silent(FileOp::Delete, &[moved.join("inner")], None).unwrap());
    assert!(!moved.join("inner").exists());
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn watcher_sees_new_file() {
    let d = scratch("watch");
    let cancel = Arc::new(AtomicBool::new(false));
    let hits = Arc::new(AtomicUsize::new(0));
    let (d2, c2, h2) = (d.clone(), cancel.clone(), hits.clone());
    let t = std::thread::spawn(move || {
        explorer_sys::watch_dir(&d2, &c2, &mut || {
            h2.fetch_add(1, Ordering::Relaxed);
        })
    });
    std::thread::sleep(Duration::from_millis(200));
    fs::write(d.join("new.txt"), b"x").unwrap();
    let start = Instant::now();
    while hits.load(Ordering::Relaxed) == 0 && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    cancel.store(true, Ordering::Relaxed);
    t.join().unwrap().unwrap();
    assert!(hits.load(Ordering::Relaxed) > 0, "no change notification");
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn clipboard_round_trip() {
    let files = vec![PathBuf::from(r"C:\Windows\notepad.exe"), PathBuf::from(r"C:\Windows\System32")];
    explorer_sys::clipboard_set_files(&files, true).unwrap();
    let (got, cut) = explorer_sys::clipboard_get_files().expect("files on clipboard");
    assert_eq!(got, files);
    assert!(cut);
    explorer_sys::clipboard_set_files(&files[..1], false).unwrap();
    let (got, cut) = explorer_sys::clipboard_get_files().unwrap();
    assert_eq!(got, &files[..1]);
    assert!(!cut);
}

#[test]
fn shell_icons() {
    let txt = explorer_sys::shell_icon(Path::new("x.txt"), false, false).expect("txt icon");
    assert!(txt.width >= 16 && txt.height >= 16);
    assert_eq!(txt.rgba.len(), txt.width * txt.height * 4);
    assert!(txt.rgba.chunks_exact(4).any(|p| p[3] > 0), "icon is fully transparent");
    assert!(explorer_sys::shell_icon(Path::new("folder"), true, false).is_some());
    assert!(explorer_sys::shell_icon(Path::new(r"C:\Windows\notepad.exe"), false, true).is_some());
}
