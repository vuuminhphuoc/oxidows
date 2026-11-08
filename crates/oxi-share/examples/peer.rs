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
    share.refresh();
    let mut sent = files.is_empty();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let s = share.snapshot();
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
