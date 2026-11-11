//! Addresses and Wi-Fi network name of an adapter, for the Performance tab's network page.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::ptr::{null, null_mut};

use taskmgr_core::AdapterInfo;
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
    IP_ADAPTER_ADDRESSES_LH,
};
use windows_sys::Win32::NetworkManagement::WiFi::{
    WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST, WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory,
    WlanOpenHandle, WlanQueryInterface, wlan_intf_opcode_current_connection,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6};

const ERROR_BUFFER_OVERFLOW: u32 = 111;

/// # Safety
/// `p` is null or a NUL-terminated UTF-16 string.
unsafe fn pwstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    // SAFETY: caller guarantees termination.
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }
}

fn text(w: &[u16]) -> String {
    let n = w.iter().position(|c| *c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..n])
}

/// The connected SSID and signal quality of the Wi-Fi interface whose description is `description`.
fn wifi_info(description: &str) -> (Option<String>, Option<u32>) {
    let mut client = null_mut();
    let mut ver = 0u32;
    // SAFETY: out-pointers to locals; every WLAN allocation is freed and the handle closed.
    unsafe {
        if WlanOpenHandle(2, null(), &mut ver, &mut client) != 0 {
            return (None, None);
        }
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = null_mut();
        let mut out = (None, None);
        if WlanEnumInterfaces(client, null(), &mut list) == 0 && !list.is_null() {
            let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), (*list).dwNumberOfItems as usize);
            for i in items {
                if text(&i.strInterfaceDescription) != description {
                    continue;
                }
                let mut size = 0u32;
                let mut data = null_mut();
                if WlanQueryInterface(
                    client,
                    &i.InterfaceGuid,
                    wlan_intf_opcode_current_connection,
                    null(),
                    &mut size,
                    &mut data,
                    null_mut(),
                ) == 0
                    && !data.is_null()
                {
                    let c = &*data.cast::<WLAN_CONNECTION_ATTRIBUTES>();
                    let s = &c.wlanAssociationAttributes.dot11Ssid;
                    let n = (s.uSSIDLength as usize).min(32);
                    let ssid = String::from_utf8_lossy(&s.ucSSID[..n]).into_owned();
                    let quality = c.wlanAssociationAttributes.wlanSignalQuality;
                    out = (Some(ssid), Some(quality));
                    WlanFreeMemory(data);
                }
            }
            WlanFreeMemory(list.cast());
        }
        WlanCloseHandle(client, null());
        out
    }
}

/// Facts about the adapter whose friendly name (alias) is `name`.
pub fn adapter_info(name: &str) -> AdapterInfo {
    let mut info = AdapterInfo::default();
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size = 16 * 1024u32;
    let mut buf: Vec<u64> = Vec::new();
    // SAFETY: an 8-byte aligned buffer of `size` bytes; the list it fills lives inside it.
    unsafe {
        let mut r = ERROR_BUFFER_OVERFLOW;
        for _ in 0..3 {
            buf = vec![0u64; (size as usize).div_ceil(8)];
            r = GetAdaptersAddresses(AF_UNSPEC as u32, flags, null(), buf.as_mut_ptr().cast(), &mut size);
            if r != ERROR_BUFFER_OVERFLOW {
                break;
            }
        }
        if r != 0 {
            return info;
        }
        let mut a = buf.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !a.is_null() {
            let ad = &*a;
            if pwstr(ad.FriendlyName) == name {
                info.dns_suffix = pwstr(ad.DnsSuffix);
                info.connection_type = match ad.IfType {
                    6 => "Ethernet",
                    71 => "Wi-Fi",
                    243 | 244 => "Cellular",
                    251 => "Bluetooth",
                    _ => "Other",
                }
                .to_string();
                let description = pwstr(ad.Description);
                let mut u = ad.FirstUnicastAddress;
                while !u.is_null() {
                    let sa = (*u).Address.lpSockaddr;
                    if !sa.is_null() {
                        match (*sa).sa_family {
                            AF_INET => {
                                let v4 = &*sa.cast::<SOCKADDR_IN>();
                                info.ipv4.push(Ipv4Addr::from(u32::from_be(v4.sin_addr.S_un.S_addr)).to_string());
                            }
                            AF_INET6 => {
                                let v6 = &*sa.cast::<SOCKADDR_IN6>();
                                info.ipv6.push(Ipv6Addr::from(v6.sin6_addr.u.Byte).to_string());
                            }
                            _ => {}
                        }
                    }
                    u = (*u).Next;
                }
                let (ssid, signal) = wifi_info(&description);
                info.ssid = ssid;
                info.signal_strength = signal;
                break;
            }
            a = ad.Next;
        }
    }
    info
}
