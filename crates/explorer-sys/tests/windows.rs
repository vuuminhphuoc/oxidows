//! Exercises the real Win32 layer. Runs on the `windows-latest` CI runner.
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use explorer_sys::{Entry, FileOp, file_op_silent, list_dir};

static RUNNING: std::sync::Mutex<Vec<(&'static str, Instant)>> = std::sync::Mutex::new(Vec::new());

/// Every test holds one of these; a single thread kills the binary (naming the stuck tests)
/// if any runs longer than 120 s, so CI fails with a reason instead of hanging.
struct Guard(&'static str);

fn guard(name: &'static str) -> Guard {
    static WATCH: std::sync::Once = std::sync::Once::new();
    WATCH.call_once(|| {
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let stuck: Vec<String> = RUNNING
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(_, t)| t.elapsed() > Duration::from_secs(120))
                    .map(|(n, t)| format!("{n} ({:?})", t.elapsed()))
                    .collect();
                if !stuck.is_empty() {
                    use std::io::Write;
                    let _ = writeln!(std::io::stderr(), "TIMEOUT: stuck tests: {stuck:?}");
                    std::process::exit(98);
                }
            }
        });
    });
    RUNNING.lock().unwrap().push((name, Instant::now()));
    Guard(name)
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut r) = RUNNING.lock() {
            r.retain(|(n, _)| *n != self.0);
        }
    }
}

/// Fails the whole test binary (instead of hanging CI for hours) if `stage` takes too long.
fn watchdog(stage: &'static str, secs: u64) -> Arc<AtomicBool> {
    let done = Arc::new(AtomicBool::new(false));
    let d = done.clone();
    eprintln!("[start] {stage}");
    std::thread::spawn(move || {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(secs) {
            if d.load(Ordering::Relaxed) {
                eprintln!("[done]  {stage} in {:?}", t.elapsed());
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // Straight to stderr: the test harness would swallow eprintln! output on exit.
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "TIMEOUT: {stage} still running after {secs} s");
        std::process::exit(99);
    });
    done
}

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
    let _g = guard("lists_names_sizes_dirs_and_unicode");
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
    let _g = guard("hidden_attribute_is_read");
    let d = scratch("hidden");
    let f = d.join("secret.txt");
    fs::write(&f, b"x").unwrap();
    // One call: attrib refuses to change a hidden file's other attributes without +h.
    let st = std::process::Command::new("attrib").args(["+h", "+r"]).arg(&f).status().unwrap();
    assert!(st.success());
    let e = &list(&d)[0];
    assert!(e.hidden);
    // Attributes column and Date created.
    let letters = explorer_core::format_attributes(e.attrs);
    assert!(letters.starts_with("RH") && letters.contains('A'), "attrs {letters} ({:#x})", e.attrs);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    assert!((now - e.created).abs() < 600, "created {} vs now {now}", e.created);
    let _ = std::process::Command::new("attrib").arg("-h").arg("-r").arg(&f).status();
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn large_folder_arrives_in_batches() {
    let _g = guard("large_folder_arrives_in_batches");
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
    let _g = guard("missing_folder_is_an_error");
    assert!(list_dir(Path::new(r"C:\definitely\not\here"), &AtomicBool::new(false), &mut |_| {}).is_err());
}

#[test]
fn drives_and_known_folders() {
    let _g = guard("drives_and_known_folders");
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
    let _g = guard("copy_move_recycle_delete");
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
    let _g = guard("watcher_sees_new_file");
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
    let _g = guard("clipboard_round_trip");
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
    let _g = guard("shell_icons");
    let txt = explorer_sys::shell_icon(Path::new("x.txt"), false, false).expect("txt icon");
    assert!(txt.width >= 16 && txt.height >= 16);
    assert_eq!(txt.rgba.len(), txt.width * txt.height * 4);
    assert!(txt.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0), "icon is fully transparent");
    let png = explorer_sys::shell_icon(Path::new("file.png"), false, false).expect("png icon");
    assert!(png.width >= 16 && png.height >= 16);
    assert!(png.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
    let jpg = explorer_sys::shell_icon(Path::new("file.jpg"), false, false).expect("jpg icon");
    assert!(jpg.width >= 16 && jpg.height >= 16);
    assert!(explorer_sys::shell_icon(Path::new(r"C:\Windows\notepad.exe"), false, true).is_some());
}

/// Real SMB against the runner's own server: share a temp folder, browse, copy, map a drive.
#[test]
fn smb_share_on_localhost() {
    let _g = guard("smb_share_on_localhost");
    let d = scratch("smb");
    fs::write(d.join("hello.txt"), b"over smb").unwrap();
    let share = format!("oxitest{}", std::process::id());
    let w = watchdog("net share", 60);
    let ok = std::process::Command::new("net")
        .args(["share", &format!("{share}={}", d.display()), "/grant:Everyone,FULL"])
        .status()
        .unwrap()
        .success();
    if !ok {
        // `net share` needs admin; the CI runner has it. Say so rather than pass silently.
        panic!("could not create test share (not elevated?)");
    }
    w.store(true, Ordering::Relaxed);
    let cleanup = || {
        let _ = std::process::Command::new("net").args(["share", &share, "/delete", "/y"]).status();
    };

    let w = watchdog("list_shares(localhost)", 60);
    let shares = explorer_sys::list_shares("localhost").unwrap();
    w.store(true, Ordering::Relaxed);
    assert!(shares.iter().any(|s| s.name.eq_ignore_ascii_case(&share)), "{shares:?}");
    assert!(!shares.iter().any(|s| s.name.ends_with('$')), "hidden shares leaked: {shares:?}");

    let unc = PathBuf::from(format!(r"\\localhost\{share}"));
    let w = watchdog("connect + list UNC", 60);
    explorer_sys::connect(&unc, false).unwrap();
    assert_eq!(names(&unc), ["hello.txt"]);
    w.store(true, Ordering::Relaxed);
    let local = scratch("smb-dst");
    assert!(file_op_silent(FileOp::Copy, &[unc.join("hello.txt")], Some(&local)).unwrap());
    assert_eq!(fs::read(local.join("hello.txt")).unwrap(), b"over smb");

    let letter = *explorer_sys::free_drive_letters().last().expect("a free drive letter");
    let w = watchdog("map drive", 60);
    explorer_sys::map_drive(letter, &unc, false, false).unwrap();
    w.store(true, Ordering::Relaxed);
    let mapped = PathBuf::from(format!("{letter}:\\"));
    let listed = names(&mapped);
    let net = explorer_sys::drives().into_iter().find(|x| x.root == mapped).map(|x| x.kind);
    explorer_sys::unmap_drive(letter).unwrap();
    cleanup();
    assert_eq!(listed, ["hello.txt"]);
    assert_eq!(net, Some(explorer_sys::DriveKind::Network));
    let _ = fs::remove_dir_all(&d);
    let _ = fs::remove_dir_all(&local);
}

#[test]
fn network_enumeration_does_not_hang() {
    let _g = guard("network_enumeration_does_not_hang");
    // The browse list can be slow without a network; it runs on a worker thread in the app,
    // so here it only must finish within a bound.
    let w = watchdog("network_computers", 120);
    let t = Instant::now();
    let found = explorer_sys::network_computers();
    w.store(true, Ordering::Relaxed);
    println!("network computers: {found:?} in {:?}", t.elapsed());
}

#[test]
fn shell_context_menu_has_standard_verbs() {
    let _g = guard("shell_context_menu_has_standard_verbs");
    let d = scratch("menu");
    let f = d.join("doc.txt");
    fs::write(&f, b"x").unwrap();
    let w = watchdog("shell menu verbs", 60);
    let verbs = explorer_sys::shell_menu_verbs(&d, std::slice::from_ref(&f)).unwrap();
    println!("file verbs: {verbs:?}");
    for v in ["open", "properties", "delete"] {
        assert!(verbs.iter().any(|x| x == v), "missing {v} in {verbs:?}");
    }
    let bg = explorer_sys::shell_menu_verbs(&d, &[]).unwrap();
    w.store(true, Ordering::Relaxed);
    println!("background verbs: {bg:?}");
    assert!(bg.iter().any(|x| x == "properties"), "{bg:?}");
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn recycle_bin_list_and_restore() {
    let _g = guard("recycle_bin_list_and_restore");
    let d = scratch("bin");
    // The bin records the long path; %TEMP% on the runner is an 8.3 one (RUNNER~1).
    let long = fs::canonicalize(&d).unwrap();
    let long = PathBuf::from(long.to_string_lossy().trim_start_matches(r"\\?\"));
    let f = long.join(format!("binned-{}.txt", std::process::id()));
    fs::write(&f, b"come back").unwrap();
    let w = watchdog("recycle + list bin", 60);
    assert!(file_op_silent(FileOp::Recycle, std::slice::from_ref(&f), None).unwrap());
    assert!(!f.exists());
    let items = explorer_sys::recycle_bin_items();
    w.store(true, Ordering::Relaxed);
    let it = items.iter().find(|i| i.original == f).unwrap_or_else(|| panic!("{f:?} not in bin: {items:?}"));
    assert_eq!(it.size, 9);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    assert!((it.deleted - now).abs() < 600);
    explorer_sys::restore(it).unwrap();
    assert_eq!(fs::read(&f).unwrap(), b"come back");

    // Recycle again and purge it for good.
    assert!(file_op_silent(FileOp::Recycle, std::slice::from_ref(&f), None).unwrap());
    let items = explorer_sys::recycle_bin_items();
    let it = items.iter().find(|i| i.original == f).unwrap();
    explorer_sys::purge(it).unwrap();
    assert!(!explorer_sys::recycle_bin_items().iter().any(|i| i.original == f));
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn thumbnails() {
    let _g = guard("thumbnails");
    let w = watchdog("thumbnail", 60);
    let t = explorer_sys::thumbnail(Path::new(r"C:\Windows\notepad.exe"), 96).expect("thumbnail");
    w.store(true, Ordering::Relaxed);
    assert_eq!(t.rgba.len(), t.width * t.height * 4);
}

#[test]
fn shortcut_target_is_read() {
    let _g = guard("shortcut_target_is_read");
    let d = scratch("lnk");
    // Long form: the shell may store either spelling of the runner's 8.3 %TEMP%.
    let long = fs::canonicalize(&d).unwrap();
    let d = PathBuf::from(long.to_string_lossy().trim_start_matches(r"\\?\"));
    let target = d.join("target.txt");
    fs::write(&target, b"x").unwrap();
    let lnk = d.join("target.lnk");
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.Save()",
        lnk.display(),
        target.display()
    );
    let st = std::process::Command::new("powershell").args(["-NoProfile", "-Command", &script]).status().unwrap();
    assert!(st.success());
    let got = explorer_sys::resolve_link(&lnk).expect("target");
    let same = |a: &Path, b: &Path| fs::canonicalize(a).ok() == fs::canonicalize(b).ok();
    assert!(same(&got, &target), "{got:?} vs {target:?}");
    // Its own icon, with the shortcut-arrow overlay.
    let icon = explorer_sys::shell_icon(&lnk, false, true).expect("shortcut icon");
    assert_eq!(icon.rgba.len(), icon.width * icon.height * 4);
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn preview_handler_hosts_a_font() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_OVERLAPPEDWINDOW};
    let _g = guard("preview_handler_hosts_a_font");
    assert!(!explorer_sys::has_preview_handler(""));
    let found: Vec<&str> = ["ttf", "txt", "xml", "rtf", "pdf", "docx", "eml", "jpg", "mp4"]
        .into_iter()
        .filter(|e| explorer_sys::has_preview_handler(e))
        .collect();
    println!("preview handlers registered for: {found:?}");
    // Windows ships the font previewer on every edition, Server included.
    assert!(found.contains(&"ttf"), "no font preview handler");
    let d = scratch("preview");
    let font = d.join("arial.ttf");
    fs::copy(r"C:\Windows\Fonts\arial.ttf", &font).unwrap();
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let parent = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            640,
            480,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    assert!(!parent.is_null());
    let w = watchdog("preview handler", 60);
    let mut host = explorer_sys::PreviewHost::open(parent as isize, &font, [10, 10, 400, 300]).expect("open preview");
    host.place(Some([20, 20, 500, 400]));
    host.place(None);
    drop(host);
    w.store(true, Ordering::Relaxed);
    unsafe { DestroyWindow(parent) };
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn shell_search_finds_by_name_and_stat_reads_entries() {
    let _g = guard("shell_search_finds_by_name_and_stat_reads_entries");
    let d = scratch("search");
    let long = fs::canonicalize(&d).unwrap();
    let d = PathBuf::from(long.to_string_lossy().trim_start_matches(r"\\?\"));
    fs::create_dir_all(d.join("sub")).unwrap();
    fs::write(d.join("sub").join("needle-oxi.txt"), b"hello").unwrap();
    fs::write(d.join("other.txt"), b"x").unwrap();
    let e = explorer_sys::stat(&d.join("sub").join("needle-oxi.txt")).expect("stat");
    assert_eq!((e.name.as_str(), e.size, e.is_dir), ("needle-oxi.txt", 5, false));
    assert!(explorer_sys::stat(&d.join("missing")).is_none());
    let w = watchdog("shell search", 90);
    let mut found = Vec::new();
    let r = explorer_sys::shell_search(&d, "needle-oxi", &AtomicBool::new(false), &mut |p| found.push(p));
    w.store(true, Ordering::Relaxed);
    println!("shell search: {r:?} {found:?}");
    // Without the Windows Search service (not installed on Windows Server by default) the
    // shell can't run the query and the app walks the folders itself instead.
    let service = std::process::Command::new("sc").args(["query", "WSearch"]).output().unwrap();
    let service = String::from_utf8_lossy(&service.stdout).into_owned();
    println!("WSearch: {service}");
    if r.is_err() && !service.contains("RUNNING") {
        fs::remove_dir_all(&d).unwrap();
        return;
    }
    r.expect("shell search");
    let same = |a: &Path, b: &Path| fs::canonicalize(a).ok() == fs::canonicalize(b).ok();
    assert!(found.iter().any(|p| same(p, &d.join("sub").join("needle-oxi.txt"))), "{found:?}");
    assert!(!found.iter().any(|p| same(p, &d.join("other.txt"))));
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn property_columns_are_read() {
    use explorer_core::Prop;
    let _g = guard("property_columns_are_read");
    let d = scratch("props");
    let png = d.join("pic.png");
    let script = format!(
        "Add-Type -AssemblyName System.Drawing; $b=New-Object System.Drawing.Bitmap 37,21; $b.Save('{}'); $b.Dispose()",
        png.display()
    );
    let st = std::process::Command::new("powershell").args(["-NoProfile", "-Command", &script]).status().unwrap();
    assert!(st.success());
    let w = watchdog("property store", 60);
    let v = explorer_sys::read_props(&png, &[Prop::Dimensions, Prop::Owner, Prop::Authors]);
    w.store(true, Ordering::Relaxed);
    println!("props: {v:?}");
    assert!(v[0].contains("37") && v[0].contains("21"), "{v:?}");
    assert!(!v[1].is_empty(), "owner: {v:?}");
    assert!(v[2].is_empty(), "authors: {v:?}");
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn libraries_list_their_folders() {
    let _g = guard("libraries_list_their_folders");
    let libs = explorer_sys::libraries();
    println!("libraries: {libs:?}");
    assert!(!libs.is_empty());
    assert!(libs.iter().all(|(n, _)| !n.is_empty() && !n.ends_with(".library-ms")));
    assert!(libs.iter().flat_map(|l| &l.1).any(|p| p.is_dir()), "{libs:?}");
}

#[test]
fn default_file_manager_round_trip() {
    let _g = guard("default_file_manager_round_trip");
    let reg = |key: &str| {
        let out = std::process::Command::new("reg").args(["query", key, "/ve"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert!(!explorer_sys::is_default_file_manager());
    let exe = Path::new(r"C:\Tools\oxi-explorer.exe");
    explorer_sys::set_default_file_manager(Some(exe)).unwrap();
    assert!(explorer_sys::is_default_file_manager());
    let cmd = reg(r"HKCU\Software\Classes\Directory\shell\oxi-explorer\command");
    assert!(cmd.contains(r#""C:\Tools\oxi-explorer.exe" "%1""#), "{cmd}");
    let win_e = reg(r"HKCU\Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow\command");
    assert!(win_e.contains("oxi-explorer.exe"), "{win_e}");
    explorer_sys::set_default_file_manager(None).unwrap();
    assert!(!explorer_sys::is_default_file_manager());
    assert!(!reg(r"HKCU\Software\Classes\Directory\shell\oxi-explorer\command").contains("oxi-explorer.exe"));
    assert!(
        !reg(r"HKCU\Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow\command")
            .contains("oxi")
    );
    assert_eq!(explorer_sys::folder_handler(), explorer_sys::FolderHandler::BuiltIn);
    // Nothing we made is left: not even the empty keys.
    let exists = |key: &str| std::process::Command::new("reg").args(["query", key]).output().unwrap().status.success();
    assert!(!exists(r"HKCU\Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}"));
    assert!(!exists(r"HKCU\Software\Classes\Directory\shell\oxi-explorer"));

    // A renamed exe is still ours, and the state says which copy.
    let renamed = Path::new(r"C:\Tools\files-rs.exe");
    explorer_sys::set_default_file_manager(Some(renamed)).unwrap();
    assert_eq!(explorer_sys::folder_handler(), explorer_sys::FolderHandler::Ours(renamed.display().to_string()));
    explorer_sys::set_default_file_manager(None).unwrap();
    assert!(!exists(r"HKCU\Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}"));

    // Another file manager's Win+E comes back on undo.
    let win_e = r"HKCU\Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow\command";
    let add = std::process::Command::new("reg")
        .args(["add", win_e, "/ve", "/d", r"C:\Files\files.exe", "/f"])
        .output()
        .unwrap();
    assert!(add.status.success());
    explorer_sys::set_default_file_manager(Some(exe)).unwrap();
    assert!(reg(win_e).contains("oxi-explorer.exe"));
    explorer_sys::set_default_file_manager(None).unwrap();
    let back = reg(win_e);
    assert!(back.contains(r"C:\Files\files.exe") && !back.contains("oxi"), "{back}");
    let _ = std::process::Command::new("reg")
        .args(["delete", r"HKCU\Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}", "/f"])
        .output();
}

#[test]
fn shell_namespace_lists_control_panel_and_this_pc() {
    let _g = guard("shell_namespace_lists_control_panel_and_this_pc");
    let w = watchdog("shell namespace", 60);
    let mut pc = Vec::new();
    explorer_sys::shell_list(Path::new(explorer_core::THIS_PC_CLSID), &AtomicBool::new(false), &mut |b| pc.extend(b))
        .expect("This PC");
    // Drives come back as real paths, labelled with their display names.
    assert!(pc.iter().any(|e| e.name.eq_ignore_ascii_case(r"C:\") && e.is_dir && e.label.is_some()), "{pc:?}");
    let cp = explorer_core::shell_path(explorer_core::CONTROL_PANEL_CLSID);
    assert!(explorer_sys::shell_is_folder(&cp));
    assert!(explorer_sys::shell_display_name(&cp).is_some_and(|n| !n.is_empty()));
    let mut items = Vec::new();
    explorer_sys::shell_list(&cp, &AtomicBool::new(false), &mut |b| items.extend(b)).expect("Control Panel");
    w.store(true, Ordering::Relaxed);
    println!("control panel: {:?}", items.iter().map(|e| e.display()).collect::<Vec<_>>());
    assert!(!items.is_empty());
    let first = items.iter().find(|e| e.name.starts_with(explorer_core::SHELL)).expect("a shell item");
    // Breadcrumb parent and icon work on the shell path.
    assert_eq!(
        explorer_core::classify(Path::new(&first.name)),
        explorer_core::Place::Shell(explorer_core::shell_parsing(Path::new(&first.name)).into_owned())
    );
    assert!(explorer_sys::shell_icon(Path::new(&first.name), first.is_dir, true).is_some(), "{first:?}");
    assert!(explorer_sys::shell_menu_verbs(&cp, &[PathBuf::from(&first.name)]).is_ok());
}

/// The Wi-Fi Direct group goes through WinRT on its own thread. The CI runner has no Wi-Fi
/// adapter, so it must end in a clear failure (not hang or crash); on a PC with Wi-Fi it starts.
#[test]
fn wifi_direct_group_starts_or_reports_why_not() {
    let _g = guard("wifi_direct_group_starts_or_reports_why_not");
    let t = Instant::now();
    let h = match explorer_sys::Hotspot::start("DIRECT-oxi-ci", "k3x9q2mz") {
        Ok(h) => h,
        Err(e) => {
            eprintln!("start refused: {e}");
            return;
        }
    };
    let status = loop {
        let s = h.status();
        if s != explorer_sys::HotspotStatus::Starting || t.elapsed() > Duration::from_secs(20) {
            break s;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    eprintln!("Wi-Fi Direct after {:?}: {status:?}", t.elapsed());
    assert!(
        matches!(status, explorer_sys::HotspotStatus::Started | explorer_sys::HotspotStatus::Failed(_)),
        "{status:?}"
    );
    // Stopping joins the thread promptly.
    let t = Instant::now();
    drop(h);
    assert!(t.elapsed() < Duration::from_secs(5));
}

#[test]
fn firewall_rule_lookup_needs_no_elevation() {
    let _g = guard("firewall_rule_lookup_needs_no_elevation");
    // Just must not hang or panic; the answer depends on the machine.
    eprintln!("firewall: {:?}", explorer_sys::firewall_state());
}

#[test]
fn shell_new_items_and_create() {
    let _g = guard("shell_new_items_and_create");
    let items = explorer_sys::new_items();
    let txt = items.iter().find(|i| i.ext == ".txt").expect("new_items() must contain .txt");
    assert_eq!(txt.name, "Text Document");

    let dir = scratch("shell-new");
    let file1 = explorer_sys::create_new(&dir, txt).expect("creates first file");
    assert_eq!(file1, dir.join("New Text Document.txt"));
    assert!(file1.is_file());

    let file2 = explorer_sys::create_new(&dir, txt).expect("creates second file");
    assert_eq!(file2, dir.join("New Text Document (2).txt"));
    assert!(file2.is_file());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn everything_search_if_running() {
    let _g = guard("everything_search_if_running");
    let cancel = AtomicBool::new(false);
    let probe = explorer_sys::everything_search(
        Path::new(r"C:\"),
        &["nonexistent_probe_file_oxi".into()],
        &cancel,
        &mut |_| {},
    );
    if matches!(probe, Err(e) if e.kind() == std::io::ErrorKind::NotFound) {
        return; // CI has no Everything
    }

    assert!(explorer_sys::everything_can_scope(Path::new(r"C:\")));

    let temp = std::env::temp_dir();
    let unique_name = format!(
        "oxi-ev-test-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
    );
    let file_path = temp.join(&unique_name);
    let content = b"hello everything search test data";
    fs::write(&file_path, content).expect("write temp file");

    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(file_path.clone());

    let start = Instant::now();
    let mut found_entry: Option<Entry> = None;
    while start.elapsed() < Duration::from_secs(5) {
        let mut results = Vec::new();
        let res = explorer_sys::everything_search(&temp, std::slice::from_ref(&unique_name), &cancel, &mut |e| {
            results.push(e);
        });
        if res.is_ok() && !results.is_empty() {
            found_entry = results.into_iter().find(|e| {
                Path::new(&e.name).file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(&unique_name))
            });
            if found_entry.is_some() {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let entry = found_entry.expect("Everything did not find temp file within 5 s");
    assert_eq!(Path::new(&entry.name).canonicalize().ok(), file_path.canonicalize().ok().or(Some(file_path)));
    assert_eq!(entry.size, content.len() as u64);
    // Read from disk after Everything answered (it doesn't index these by default).
    assert!(entry.created > 0 && !entry.hidden);
}
