//! A Wi-Fi Direct group owned by this PC, for sharing with a phone when there is no Wi-Fi network
//! in common. `WiFiDirectAdvertisementPublisher` with legacy settings makes the group accept
//! ordinary Wi-Fi clients: the phone joins it like any network (by scanning a QR code), and
//! LocalSend then runs over it. The PC stays on its own Wi-Fi meanwhile.

use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use windows::Devices::WiFiDirect::{
    WiFiDirectAdvertisementListenStateDiscoverability, WiFiDirectAdvertisementPublisher,
    WiFiDirectAdvertisementPublisherStatus, WiFiDirectAdvertisementPublisherStatusChangedEventArgs as StatusArgs,
    WiFiDirectError,
};
use windows::Foundation::TypedEventHandler;
use windows::Security::Credentials::PasswordCredential;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::HSTRING;

use crate::HotspotStatus;

pub struct Hotspot {
    stop: Sender<()>,
    status: Arc<Mutex<HotspotStatus>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Hotspot {
    pub fn start(ssid: &str, passphrase: &str) -> io::Result<Self> {
        let status = Arc::new(Mutex::new(HotspotStatus::Starting));
        let (stop, rx) = channel();
        let (ssid, pass, st) = (ssid.to_string(), passphrase.to_string(), status.clone());
        let thread = thread::Builder::new().name("wifi-direct".into()).spawn(move || {
            if let Err(e) = run(&ssid, &pass, &st, &rx) {
                *st.lock().unwrap_or_else(|e| e.into_inner()) = HotspotStatus::Failed(describe(&e));
            }
        })?;
        Ok(Self { stop, status, thread: Some(thread) })
    }

    pub fn status(&self) -> HotspotStatus {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Drop for Hotspot {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn describe(e: &windows::core::Error) -> String {
    let msg = e.message();
    if msg.is_empty() { format!("{:#010X}", e.code().0) } else { msg }
}

fn run(ssid: &str, pass: &str, status: &Arc<Mutex<HotspotStatus>>, stop: &Receiver<()>) -> windows::core::Result<()> {
    // WinRT objects need a COM apartment; this thread owns the publisher for its whole life.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let publisher = WiFiDirectAdvertisementPublisher::new()?;
    let adv = publisher.Advertisement()?;
    adv.SetListenStateDiscoverability(WiFiDirectAdvertisementListenStateDiscoverability::Normal)?;
    let legacy = adv.LegacySettings()?;
    legacy.SetIsEnabled(true)?;
    legacy.SetSsid(&HSTRING::from(ssid))?;
    let cred = PasswordCredential::new()?;
    cred.SetPassword(&HSTRING::from(pass))?;
    legacy.SetPassphrase(&cred)?;

    let st = status.clone();
    let handler = TypedEventHandler::<WiFiDirectAdvertisementPublisher, StatusArgs>::new(move |_, args| {
        let args = args.ok()?;
        let s = args.Status()?;
        let next = if s == WiFiDirectAdvertisementPublisherStatus::Started {
            HotspotStatus::Started
        } else if s == WiFiDirectAdvertisementPublisherStatus::Aborted {
            let err = args.Error().unwrap_or(WiFiDirectError::Success);
            HotspotStatus::Failed(
                if err == WiFiDirectError::RadioNotAvailable {
                    "Wi-Fi is off or this adapter has no Wi-Fi Direct support"
                } else if err == WiFiDirectError::ResourceInUse {
                    "Wi-Fi Direct is in use (Mobile hotspot, a wireless display or another app)"
                } else {
                    "Wi-Fi Direct stopped"
                }
                .into(),
            )
        } else if s == WiFiDirectAdvertisementPublisherStatus::Stopped {
            HotspotStatus::Stopped
        } else {
            return Ok(());
        };
        *st.lock().unwrap_or_else(|e| e.into_inner()) = next;
        Ok(())
    });
    let token = publisher.StatusChanged(&handler)?;
    publisher.Start()?;
    loop {
        match stop.recv_timeout(Duration::from_millis(500)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                if matches!(*status.lock().unwrap_or_else(|e| e.into_inner()), HotspotStatus::Failed(_)) {
                    break;
                }
            }
        }
    }
    let _ = publisher.RemoveStatusChanged(token);
    let _ = publisher.Stop();
    let mut s = status.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(*s, HotspotStatus::Failed(_)) {
        *s = HotspotStatus::Stopped;
    }
    Ok(())
}

const RULE: &str = "oxi-explorer nearby sharing";

/// Whether Windows Firewall already has our inbound rule (needs no elevation to ask).
pub fn firewall_rule_exists() -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "show", "rule", &format!("name={RULE}")])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Adds an inbound allow rule for this exe on every network profile. The Wi-Fi Direct group is
/// a "public" network, which the first-run firewall prompt leaves blocked. Shows the UAC prompt.
pub fn allow_in_firewall() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let cmd = format!(
        "/c netsh advfirewall firewall delete rule name=\"{RULE}\" >nul & \
         netsh advfirewall firewall add rule name=\"{RULE}\" dir=in action=allow program=\"{}\" enable=yes profile=any",
        exe.display()
    );
    crate::imp::shell_execute_hidden("runas", "cmd.exe", &cmd)
}
