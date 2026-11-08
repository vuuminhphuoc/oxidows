//! A stand-in device for trying the sharing UI: announces itself, accepts everything into a temp
//! folder, and, given paths, sends them to the first device it finds.
//!
//! `cargo run -p oxi-share --example peer -- "Test phone" [file...]`

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn main() {
    let mut args = std::env::args().skip(1);
    let alias = args.next().unwrap_or_else(|| "Test peer".into());
    let files: Vec<PathBuf> = args.map(PathBuf::from).collect();
    let root = std::env::temp_dir().join(format!("oxi-share-peer-{}", std::process::id()));
    let share = oxi_share::Share::start(
        oxi_share::Config {
            alias,
            device_model: Some("Example".into()),
            port: oxi_share::DEFAULT_PORT,
            identity_dir: root.join("id"),
            receive_dir: root.join("in"),
            quick_save: true,
            multicast: true,
        },
        Arc::new(|| {}),
    )
    .expect("start");
    println!("listening on {} as {}", share.port(), share.fingerprint());
    for (name, ip, prefix) in oxi_share::local_ipv4() {
        println!("interface {name}: {ip}/{prefix}");
    }
    println!("received files go to {}", root.join("in").display());
    share.refresh();
    let mut sent = files.is_empty();
    let mut seen = String::new();
    for tick in 1u64.. {
        std::thread::sleep(Duration::from_secs(1));
        if tick % 15 == 0 {
            share.refresh();
        }
        let s = share.snapshot();
        let now: String = s
            .devices
            .iter()
            .map(|d| {
                format!("  {} ({:?}, {:?}) at {} {:?}\n", d.alias, d.device_model, d.device_type, d.addr(), d.protocol)
            })
            .collect();
        if now != seen {
            println!("devices found: {}\n{now}", s.devices.len());
            seen = now;
        }
        if !sent && let Some(d) = s.devices.first() {
            println!("sending to {} at {}", d.alias, d.addr());
            share.send(d, files.clone());
            sent = true;
        }
        for t in &s.transfers {
            println!("{:?} {} {:?} {}/{}", t.direction, t.peer, t.state, t.done(), t.total());
        }
    }
}
