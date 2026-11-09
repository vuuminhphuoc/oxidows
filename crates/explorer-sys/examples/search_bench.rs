use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn run_backend<F>(name: &str, timeout: Duration, f: F)
where
    F: FnOnce(&AtomicBool) -> Result<usize, String>,
{
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_timer = cancel.clone();
    let timer = std::thread::spawn(move || {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if cancel_timer.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        cancel_timer.store(true, Ordering::Relaxed);
    });

    let start = Instant::now();
    let res = f(&cancel);
    let elapsed = start.elapsed();
    cancel.store(true, Ordering::Relaxed);
    let _ = timer.join();

    if elapsed >= timeout || cancel.load(Ordering::Relaxed) && res.is_err() && res.as_ref().unwrap_err() == "timeout" {
        println!("{name}: timeout");
    } else {
        match res {
            Ok(count) => println!("{name}: {count} hits in {} ms", elapsed.as_millis()),
            Err(e) => println!("{name}: {e}"),
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("usage: search_bench <root> <word>"));
    let word = args.next().expect("usage: search_bench <root> <word>");
    let timeout = Duration::from_secs(180);

    // 1. everything_search
    let cancel_ev = Arc::new(AtomicBool::new(false));
    let cancel_timer = cancel_ev.clone();
    let timer = std::thread::spawn(move || {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if cancel_timer.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        cancel_timer.store(true, Ordering::Relaxed);
    });

    let start = Instant::now();
    #[cfg(windows)]
    let ev_res = {
        let mut count = 0usize;
        match explorer_sys::everything_search_timed(&root, std::slice::from_ref(&word), &cancel_ev, &mut |_| count += 1)
        {
            Ok(reply_took) => Ok((count, reply_took)),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Err("timeout".into()),
            Err(e) if e.kind() == std::io::ErrorKind::Unsupported => Err("unsupported".into()),
            Err(e) => Err(format!("failed ({e})")),
        }
    };
    #[cfg(not(windows))]
    let ev_res: Result<(usize, Duration), String> = Err("unsupported".into());

    let elapsed = start.elapsed();
    cancel_ev.store(true, Ordering::Relaxed);
    let _ = timer.join();

    if elapsed >= timeout {
        println!("everything_search: timeout");
    } else {
        match ev_res {
            Ok((count, reply_took)) => println!(
                "everything_search: {count} hits in {} ms (reply in {} ms)",
                elapsed.as_millis(),
                reply_took.as_millis()
            ),
            Err(e) => println!("everything_search: {e}"),
        }
    }
    // 2. shell_search
    run_backend("shell_search", timeout, |cancel| {
        #[cfg(windows)]
        {
            let mut count = 0usize;
            match explorer_sys::shell_search(&root, &word, cancel, &mut |_| count += 1) {
                Ok(()) => Ok(count),
                Err(e) if e.kind() == std::io::ErrorKind::Unsupported => Err("unsupported".into()),
                Err(e) => Err(format!("failed ({e})")),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = cancel;
            Err("unsupported".into())
        }
    });

    // 3. walk_parallel (8 threads)
    let word_lower = word.to_lowercase();
    run_backend("walk_parallel(8 threads, name contains word, case-insensitive)", timeout, |cancel| {
        let count = Mutex::new(0usize);
        explorer_sys::walk_parallel(&root, 8, cancel, &|_rel, entries| {
            let hits = entries.into_iter().filter(|e| e.name.to_lowercase().contains(&word_lower)).count();
            if hits > 0 {
                let mut lk = count.lock().unwrap();
                *lk += hits;
            }
        });
        if cancel.load(Ordering::Relaxed) {
            Err("timeout".into())
        } else {
            let total = *count.lock().unwrap();
            Ok(total)
        }
    });

    // 4. walk_parallel (1 thread)
    run_backend("walk_parallel(1 thread)", timeout, |cancel| {
        let count = Mutex::new(0usize);
        explorer_sys::walk_parallel(&root, 1, cancel, &|_rel, entries| {
            let hits = entries.into_iter().filter(|e| e.name.to_lowercase().contains(&word_lower)).count();
            if hits > 0 {
                let mut lk = count.lock().unwrap();
                *lk += hits;
            }
        });
        if cancel.load(Ordering::Relaxed) {
            Err("timeout".into())
        } else {
            let total = *count.lock().unwrap();
            Ok(total)
        }
    });
}
