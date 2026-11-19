//! Speed of the pure-Rust pieces of `explorer-core` next to the Windows functions they replace,
//! plus a loose time budget for sorting a big folder. Run with
//! `cargo bench -p explorer-sys --bench vs_windows` (Windows only; prints nothing elsewhere).
//! Prints a Markdown table, and appends it to the job summary when `GITHUB_STEP_SUMMARY` is set.

fn main() {
    #[cfg(windows)]
    windows_only::run();
}

#[cfg(windows)]
mod windows_only {
    use std::hint::black_box;
    use std::io::Write;
    use std::time::{Duration, Instant};

    use explorer_core::{Entry, SortKey, format_size_kb, natural_cmp_portable, sorted_view};
    use windows_sys::Win32::UI::Shell::{StrCmpLogicalW, StrFormatKBSizeW};

    /// Folder size for the sort budget.
    const FILES: usize = 100_000;
    /// Generous: a release build sorts 100k names in well under a second; this only trips on a
    /// slowdown of an order of magnitude, which shared CI runners can't fake.
    const SORT_BUDGET: Duration = Duration::from_secs(5);

    /// Deterministic pseudo-random numbers, so every run sorts the same names.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }
    }

    fn names(n: usize) -> Vec<String> {
        let mut r = Lcg(42);
        (0..n)
            .map(|_| match r.next() % 3 {
                0 => format!("IMG_{:04}.jpg", r.next() % 10_000),
                1 => format!("Report {} (v{}).docx", r.next() % 500, r.next() % 20),
                _ => format!("file{}", r.next() % 100_000),
            })
            .collect()
    }

    /// Median of 5 runs.
    fn time(mut f: impl FnMut()) -> Duration {
        let mut runs: Vec<Duration> = (0..5)
            .map(|_| {
                let t = Instant::now();
                f();
                t.elapsed()
            })
            .collect();
        runs.sort();
        runs[2]
    }

    fn ms(d: Duration) -> String {
        format!("{:.1} ms", d.as_secs_f64() * 1000.0)
    }

    pub fn run() {
        let names = names(FILES);
        let wide: Vec<Vec<u16>> = names.iter().map(|n| n.encode_utf16().chain(Some(0)).collect()).collect();
        let mut rows: Vec<(String, Duration, Option<Duration>)> = Vec::new();

        // Name order: our `natural_cmp_portable` on &str against StrCmpLogicalW on UTF-16 prepared once
        // (the best case for Windows).
        let ours = time(|| {
            let mut v: Vec<&String> = names.iter().collect();
            v.sort_by(|a, b| natural_cmp_portable(a, b));
            black_box(v);
        });
        let theirs = time(|| {
            let mut v: Vec<&Vec<u16>> = wide.iter().collect();
            // SAFETY: every buffer is NUL-terminated and outlives the call.
            v.sort_by(|a, b| unsafe { StrCmpLogicalW(a.as_ptr(), b.as_ptr()) }.cmp(&0));
            black_box(v);
        });
        rows.push((format!("sort {FILES} names (natural order)"), ours, Some(theirs)));

        // Size column text.
        let sizes: Vec<u64> = (0..FILES as u64).map(|i| i * 7919 + 1).collect();
        let ours = time(|| {
            for &s in &sizes {
                black_box(format_size_kb(s));
            }
        });
        let theirs = time(|| {
            let mut buf = [0u16; 64];
            for &s in &sizes {
                // SAFETY: `buf` is writable for 64 u16 and the length passed is 64.
                unsafe { StrFormatKBSizeW(s as i64, buf.as_mut_ptr(), buf.len() as u32) };
                black_box(&buf);
            }
        });
        rows.push((format!("format {FILES} sizes (Size column)"), ours, Some(theirs)));

        // A whole folder view: filter + folders first + sort, no Windows counterpart to compare.
        let entries: Vec<Entry> = names
            .iter()
            .enumerate()
            .map(|(i, n)| Entry { name: n.clone(), is_dir: i % 10 == 0, size: sizes[i], ..Default::default() })
            .collect();
        let mut worst = Duration::ZERO;
        let mut view = |label: &str, key: SortKey| {
            let d = time(|| {
                black_box(sorted_view(&entries, "", true, key, true));
            });
            worst = worst.max(d);
            rows.push((format!("folder view of {FILES} entries sorted by {label}"), d, None));
        };
        view("Name (portable order)", SortKey::Name);
        // What the app does: Windows' own comparer behind `natural_cmp`.
        explorer_sys::use_windows_name_order();
        view("Name (Windows order, as in the app)", SortKey::Name);
        view("Size", SortKey::Size);

        let mut table = String::from(
            "### explorer-core vs Windows\n\n| Task | oxidows | Windows | Speed-up |\n| --- | --- | --- | --- |\n",
        );
        for (task, ours, theirs) in &rows {
            let (w, x) = match theirs {
                Some(t) => (ms(*t), format!("{:.1}x", t.as_secs_f64() / ours.as_secs_f64().max(1e-9))),
                None => ("-".into(), "-".into()),
            };
            table.push_str(&format!("| {task} | {} | {w} | {x} |\n", ms(*ours)));
        }
        println!("{table}");
        if let Ok(path) = std::env::var("GITHUB_STEP_SUMMARY")
            && let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(path)
        {
            let _ = writeln!(f, "{table}");
        }
        assert!(worst < SORT_BUDGET, "sorting {FILES} entries took {worst:?}, over the {SORT_BUDGET:?} budget");
    }
}
