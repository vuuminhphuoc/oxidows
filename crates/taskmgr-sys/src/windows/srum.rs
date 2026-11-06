//! App history from the System Resource Usage Monitor database (SRUM), the same data Task
//! Manager's App history page shows.
//!
//! `%SystemRoot%\System32\sru\SRUDB.dat` is an ESE database the DPS service keeps open, so it is
//! copied through a shadow copy with `esentutl /y /vss` (needs administrator), brought to a clean
//! state with `esentutl /r`, then read with the ESE API from esent.dll.

use std::collections::HashMap;
use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr::{null, null_mut};

use taskmgr_core::{AppHistory, AppUsage};
use windows_sys::Win32::Storage::Jet::{
    JET_COLUMNDEF, JET_DbInfoPageSize, JET_INSTANCE, JET_MoveFirst, JET_SESID, JET_bitDbReadOnly, JET_bitTableReadOnly,
    JET_errNoCurrentRecord, JET_paramDatabasePageSize, JET_paramLogFilePath, JET_paramNoInformationEvent,
    JET_paramRecovery, JET_paramSystemPath, JET_paramTempPath, JET_wrnBufferTruncated, JetAttachDatabase2W,
    JetBeginSessionW, JetCloseDatabase, JetCloseTable, JetCreateInstanceW, JetDetachDatabaseW, JetEndSession,
    JetGetDatabaseFileInfoW, JetGetTableColumnInfoW, JetInit, JetMove, JetOpenDatabaseW, JetOpenTableW,
    JetRetrieveColumn, JetSetSystemParameterW, JetTerm,
};
use windows_sys::Win32::Storage::StructuredStorage::JET_TABLEID;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const JET_MOVE_NEXT: i32 = 1;
const JET_COL_INFO: u32 = 0;

const ID_MAP: &str = "SruDbIdMapTable";
/// Application Resource Usage: CPU cycles and disk bytes per app and user, per hour.
const APP_USAGE: &str = "{D10CA2FE-6FCF-4F6D-848E-B2E99266FA89}";
/// Network Data Usage: bytes sent and received per app, user and interface.
const NET_USAGE: &str = "{973F5D5C-1D90-4944-BE8E-24B94231A174}";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn esentutl(args: &[&str], dir: &Path) -> io::Result<()> {
    let out = Command::new("esentutl.exe").args(args).current_dir(dir).creation_flags(CREATE_NO_WINDOW).output()?;
    if out.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
    Err(io::Error::other(format!("esentutl {} failed: {last}", args.first().unwrap_or(&""))))
}

/// Copies the live SRUM files into `dir` and replays their logs so the copy opens cleanly.
fn snapshot(dir: &Path) -> io::Result<PathBuf> {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let sru = Path::new(&root).join(r"System32\sru");
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)?;
    for entry in std::fs::read_dir(&sru)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let lower = name.to_lowercase();
        if !(lower.ends_with(".dat") || lower.ends_with(".log") || lower.ends_with(".chk") || lower.ends_with(".jrs")) {
            continue;
        }
        let dest = dir.join(&name);
        esentutl(&["/y", &path.display().to_string(), "/vss", "/d", &dest.display().to_string(), "/o"], dir)?;
    }
    let db = dir.join("SRUDB.dat");
    if !db.exists() {
        return Err(io::Error::other("SRUDB.dat was not copied"));
    }
    // Soft recovery with the copied logs ("sru" is the log base name). If logs are missing this
    // fails; the attach below then reports the real problem.
    let _ = esentutl(&["/r", "sru", "/i", "/o"], dir);
    Ok(db)
}

struct Jet {
    instance: JET_INSTANCE,
    sesid: JET_SESID,
    dbid: u32,
    db: Vec<u16>,
}

impl Drop for Jet {
    fn drop(&mut self) {
        // SAFETY: handles opened in `open`; zero means "not opened" and is skipped.
        unsafe {
            if self.dbid != 0 {
                JetCloseDatabase(self.sesid, self.dbid, 0);
                JetDetachDatabaseW(self.sesid, self.db.as_ptr());
            }
            if self.sesid != 0 {
                JetEndSession(self.sesid, 0);
            }
            if self.instance != 0 {
                JetTerm(self.instance);
            }
        }
    }
}

fn check(r: i32, what: &str) -> io::Result<()> {
    if r < 0 { Err(io::Error::other(format!("{what} failed: JET error {r}"))) } else { Ok(()) }
}

impl Jet {
    fn open(db: &Path, work: &Path) -> io::Result<Jet> {
        let dbw = wide(&db.display().to_string());
        let mut page = 0u32;
        // SAFETY: NUL-terminated path, 4-byte out value.
        let r = unsafe { JetGetDatabaseFileInfoW(dbw.as_ptr(), (&mut page as *mut u32).cast(), 4, JET_DbInfoPageSize) };
        check(r, "reading the SRUM page size")?;
        let mut jet = Jet { instance: 0, sesid: 0, dbid: 0, db: dbw };
        let name = wide("oxidows-srum");
        let mut path = work.display().to_string();
        path.push('\\');
        let pathw = wide(&path);
        let off = wide("Off");
        // SAFETY: ESE calls in the documented order with NUL-terminated strings.
        unsafe {
            JetSetSystemParameterW(null_mut(), 0, JET_paramDatabasePageSize, page as usize, null());
            check(JetCreateInstanceW(&mut jet.instance, name.as_ptr()), "JetCreateInstance")?;
            let inst = &mut jet.instance as *mut JET_INSTANCE;
            JetSetSystemParameterW(inst, 0, JET_paramRecovery, 0, off.as_ptr());
            JetSetSystemParameterW(inst, 0, JET_paramNoInformationEvent, 1, null());
            for p in [JET_paramSystemPath, JET_paramTempPath, JET_paramLogFilePath] {
                JetSetSystemParameterW(inst, 0, p, 0, pathw.as_ptr());
            }
            check(JetInit(inst), "JetInit")?;
            check(JetBeginSessionW(jet.instance, &mut jet.sesid, null(), null()), "JetBeginSession")?;
            check(JetAttachDatabase2W(jet.sesid, jet.db.as_ptr(), 0, JET_bitDbReadOnly), "attaching SRUDB.dat")?;
            check(
                JetOpenDatabaseW(jet.sesid, jet.db.as_ptr(), null(), &mut jet.dbid, JET_bitDbReadOnly),
                "JetOpenDatabase",
            )?;
        }
        Ok(jet)
    }

    /// Calls `row` for every record of `table` with a reader for the named columns.
    fn scan(&self, table: &str, cols: &[&str], mut row: impl FnMut(&[Option<Vec<u8>>])) -> io::Result<()> {
        let t = wide(table);
        let mut tid: JET_TABLEID = 0;
        // SAFETY: open database; table closed below.
        check(
            unsafe { JetOpenTableW(self.sesid, self.dbid, t.as_ptr(), null(), 0, JET_bitTableReadOnly, &mut tid) },
            table,
        )?;
        let result = self.scan_rows(tid, cols, &mut row);
        // SAFETY: table opened above.
        unsafe { JetCloseTable(self.sesid, tid) };
        result
    }

    fn scan_rows(&self, tid: JET_TABLEID, cols: &[&str], row: &mut impl FnMut(&[Option<Vec<u8>>])) -> io::Result<()> {
        let mut ids = Vec::with_capacity(cols.len());
        for c in cols {
            let cw = wide(c);
            // SAFETY: plain struct out-parameter.
            let mut def: JET_COLUMNDEF = unsafe { std::mem::zeroed() };
            def.cbStruct = size_of::<JET_COLUMNDEF>() as u32;
            let r = unsafe {
                JetGetTableColumnInfoW(
                    self.sesid,
                    tid,
                    cw.as_ptr(),
                    (&mut def as *mut JET_COLUMNDEF).cast(),
                    def.cbStruct,
                    JET_COL_INFO,
                )
            };
            ids.push((r >= 0).then_some(def.columnid));
        }
        // SAFETY: valid cursor.
        let mut r = unsafe { JetMove(self.sesid, tid, JET_MoveFirst as i32, 0) };
        let mut values: Vec<Option<Vec<u8>>> = vec![None; cols.len()];
        while r >= 0 {
            for (v, id) in values.iter_mut().zip(&ids) {
                *v = id.and_then(|id| self.retrieve(tid, id));
            }
            row(&values);
            // SAFETY: valid cursor.
            r = unsafe { JetMove(self.sesid, tid, JET_MOVE_NEXT, 0) };
        }
        if r == JET_errNoCurrentRecord { Ok(()) } else { check(r, "reading SRUM rows") }
    }

    fn retrieve(&self, tid: JET_TABLEID, id: u32) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; 512];
        let mut actual = 0u32;
        // SAFETY: buffer of the given size; retried once at the reported size when truncated.
        unsafe {
            let mut r = JetRetrieveColumn(
                self.sesid,
                tid,
                id,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut actual,
                0,
                null_mut(),
            );
            if r == JET_wrnBufferTruncated as i32 {
                buf = vec![0u8; actual as usize];
                r = JetRetrieveColumn(
                    self.sesid,
                    tid,
                    id,
                    buf.as_mut_ptr().cast(),
                    buf.len() as u32,
                    &mut actual,
                    0,
                    null_mut(),
                );
            }
            if r < 0 || r == 1004 {
                // 1004 = JET_wrnColumnNull
                return None;
            }
        }
        buf.truncate(actual as usize);
        Some(buf)
    }
}

fn int(v: &Option<Vec<u8>>) -> u64 {
    match v.as_deref() {
        Some(b) if !b.is_empty() => {
            let mut a = [0u8; 8];
            let n = b.len().min(8);
            a[..n].copy_from_slice(&b[..n]);
            u64::from_le_bytes(a)
        }
        _ => 0,
    }
}

/// OLE automation date (days since 1899-12-30) as a double.
fn ole_date(v: &Option<Vec<u8>>) -> Option<f64> {
    let b = v.as_deref()?;
    (b.len() == 8).then(|| f64::from_le_bytes(b.try_into().unwrap_or_default()))
}

/// Friendly name for an SRUM app id blob: a path's file name, or the package / service name.
fn app_name(blob: &[u8]) -> String {
    let u: Vec<u16> = blob.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let s = String::from_utf16_lossy(&u).trim_end_matches('\0').to_string();
    let s = s.rsplit('\\').next().unwrap_or(&s).to_string();
    if s.is_empty() { "(unknown)".into() } else { s }
}

/// Reads App history. `cycles_per_sec` turns CPU cycles into time (the processor's base clock).
pub fn app_history(cycles_per_sec: f64) -> io::Result<AppHistory> {
    let work = std::env::temp_dir().join("oxidows-srum");
    let db = snapshot(&work)?;
    let jet = Jet::open(&db, &work)?;

    let mut names: HashMap<u32, String> = HashMap::new();
    jet.scan(ID_MAP, &["IdType", "IdIndex", "IdBlob"], |v| {
        // IdType 3 is a user SID; 0, 1 and 2 are apps (path, package, service).
        if int(&v[0]) != 3
            && let Some(blob) = &v[2]
        {
            names.insert(int(&v[1]) as u32, app_name(blob));
        }
    })?;

    // (app id, cpu cycles, bytes sent, bytes received), aggregated below.
    let mut rows: Vec<(u64, u64, u64, u64)> = Vec::new();
    let mut since: Option<f64> = None;
    let mut first = |d: Option<f64>| {
        if let Some(d) = d {
            since = Some(since.map_or(d, |s: f64| s.min(d)));
        }
    };
    jet.scan(APP_USAGE, &["TimeStamp", "AppId", "ForegroundCycleTime", "BackgroundCycleTime"], |v| {
        first(ole_date(&v[0]));
        rows.push((int(&v[1]), int(&v[2]) + int(&v[3]), 0, 0));
    })?;
    jet.scan(NET_USAGE, &["TimeStamp", "AppId", "BytesSent", "BytesRecvd"], |v| {
        first(ole_date(&v[0]));
        rows.push((int(&v[1]), 0, int(&v[2]), int(&v[3])));
    })?;
    let mut apps: HashMap<String, AppUsage> = HashMap::new();
    for (id, cycles, sent, recv) in rows {
        let name = names.get(&(id as u32)).cloned().unwrap_or_else(|| format!("App {id}"));
        let a = apps.entry(name.clone()).or_insert_with(|| AppUsage { name, ..Default::default() });
        a.cpu_cycles += cycles;
        a.bytes_sent += sent;
        a.bytes_received += recv;
    }
    drop(jet);
    let _ = std::fs::remove_dir_all(&work);

    let mut apps: Vec<AppUsage> = apps.into_values().collect();
    if cycles_per_sec > 0.0 {
        for a in &mut apps {
            a.cpu_time_100ns = (a.cpu_cycles as f64 / cycles_per_sec * 1e7) as u64;
        }
    }
    Ok(AppHistory { since_ole_date: since, apps })
}
