//! Where the daemon listens and whom it answers: private IPv4 interfaces only, peers from the
//! listener's own subnet, and no foreign browser origins (ADR-0011: LAN only, no auth).

use std::net::{IpAddr, Ipv4Addr};

/// A listen address and the subnet its peers must come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subnet {
    pub addr: Ipv4Addr,
    pub mask: Ipv4Addr,
}

impl Subnet {
    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        let m = u32::from(self.mask);
        u32::from(ip) & m == u32::from(self.addr) & m
    }

    pub fn prefix_len(&self) -> u32 {
        u32::from(self.mask).count_ones()
    }
}

/// An IPv4 address of a network interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Iface {
    pub name: String,
    pub addr: Ipv4Addr,
    pub mask: Ipv4Addr,
    pub up: bool,
    pub loopback: bool,
}

/// The machine's IPv4 interface addresses (`getifaddrs`).
pub fn interfaces() -> std::io::Result<Vec<Iface>> {
    let mut out = Vec::new();
    // SAFETY: getifaddrs fills `head` with a list freed by freeifaddrs below; every pointer read is
    // checked for null and only dereferenced while the list is alive.
    unsafe {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut head) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut cur = head;
        while !cur.is_null() {
            let ifa = &*cur;
            cur = ifa.ifa_next;
            if ifa.ifa_addr.is_null()
                || ifa.ifa_netmask.is_null()
                || i32::from((*ifa.ifa_addr).sa_family) != libc::AF_INET
            {
                continue;
            }
            let ip = |sa: *const libc::sockaddr| {
                let sin = &*(sa as *const libc::sockaddr_in);
                Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr))
            };
            // `ifa_flags` is unsigned and the `IFF_*` constants signed, on Linux and macOS alike.
            let flags = ifa.ifa_flags;
            out.push(Iface {
                name: std::ffi::CStr::from_ptr(ifa.ifa_name)
                    .to_string_lossy()
                    .into_owned(),
                addr: ip(ifa.ifa_addr),
                mask: ip(ifa.ifa_netmask),
                up: flags & libc::IFF_UP as libc::c_uint != 0,
                loopback: flags & libc::IFF_LOOPBACK as libc::c_uint != 0,
            });
        }
        libc::freeifaddrs(head);
    }
    Ok(out)
}

/// Interfaces of VPNs, virtual machines, containers and Apple's peer-to-peer links, by name prefix: private
/// addresses there are not the LAN, so they are not listened on by default.
const VIRTUAL_IFACES: &[&str] = &[
    "utun",
    "bridge",
    "vmnet",
    "vboxnet",
    "docker",
    "veth",
    "virbr",
    "br-",
    "tun",
    "tap",
    "wg",
    "tailscale",
    "ipsec",
    "ppp",
    "feth",
    "llw",
    "awdl",
    "anpi",
];

fn is_virtual(name: &str) -> bool {
    VIRTUAL_IFACES.iter().any(|p| name.starts_with(p))
}

/// What to listen on: each of `explicit` (a loopback address answers 127.0.0.0/8, another one must be an
/// interface's), or by default every up, non-loopback, private (RFC 1918) address of a physical interface
/// (not [`VIRTUAL_IFACES`]).
pub fn listeners(explicit: &[Ipv4Addr], ifaces: &[Iface]) -> Result<Vec<Subnet>, String> {
    let mut out: Vec<Subnet> = Vec::new();
    if explicit.is_empty() {
        for i in ifaces {
            if i.up
                && !i.loopback
                && i.addr.is_private()
                && !is_virtual(&i.name)
                && !out.iter().any(|s| s.addr == i.addr)
            {
                out.push(Subnet {
                    addr: i.addr,
                    mask: i.mask,
                });
            }
        }
        if out.is_empty() {
            return Err("没有可监听的私网 IPv4 地址，请用 --listen 指定".into());
        }
        return Ok(out);
    }
    for &addr in explicit {
        let subnet = if addr.is_loopback() {
            Subnet {
                addr,
                mask: Ipv4Addr::new(255, 0, 0, 0),
            }
        } else {
            let i = ifaces
                .iter()
                .find(|i| i.addr == addr)
                .ok_or_else(|| format!("{addr} 不是本机网络接口的 IPv4 地址"))?;
            Subnet { addr, mask: i.mask }
        };
        if !out.contains(&subnet) {
            out.push(subnet);
        }
    }
    Ok(out)
}

/// Only IPv4 peers in the listener's subnet.
pub fn peer_allowed(peer: IpAddr, subnet: &Subnet) -> bool {
    match peer {
        IpAddr::V4(ip) => subnet.contains(ip),
        IpAddr::V6(_) => false,
    }
}

/// The `Host` header names this listener: its IPv4 address (a loopback listener: any 127.x or
/// `localhost`), with or without a port. Any other name may be a DNS rebinding (a page of another site
/// whose name now points here), so it is refused, as is a request without `Host`.
pub fn host_allowed(host: Option<&str>, subnet: &Subnet) -> bool {
    let Some(host) = host else {
        return false;
    };
    let name = match host.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => name,
        Some(_) => return false,
        None => host,
    };
    let loopback = subnet.addr.is_loopback();
    match name.parse::<Ipv4Addr>() {
        Ok(ip) => ip == subnet.addr || (loopback && ip.is_loopback()),
        Err(_) => loopback && name.eq_ignore_ascii_case("localhost"),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum OriginCheck {
    /// No `Origin` (not a browser) or this daemon's own page.
    Allow,
    /// An origin given with `--allow-origin`: allowed, with CORS headers.
    Cors,
    Deny,
}

/// `origin` is the request's `Origin` header, `host` its `Host` header (already checked by [`host_allowed`],
/// so an origin equal to it is this daemon's own page).
pub fn check_origin(origin: Option<&str>, host: Option<&str>, allowed: &[String]) -> OriginCheck {
    let Some(origin) = origin else {
        return OriginCheck::Allow;
    };
    if origin == "null" {
        return OriginCheck::Deny;
    }
    if host.is_some_and(|h| origin.strip_prefix("http://") == Some(h)) {
        return OriginCheck::Allow;
    }
    if allowed.iter().any(|a| a == origin) {
        return OriginCheck::Cors;
    }
    OriginCheck::Deny
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(name: &str, addr: [u8; 4], mask: [u8; 4], up: bool, loopback: bool) -> Iface {
        Iface {
            name: name.into(),
            addr: addr.into(),
            mask: mask.into(),
            up,
            loopback,
        }
    }

    #[test]
    fn default_listeners_are_up_private_non_loopback_interfaces() {
        let ifaces = [
            iface("lo0", [127, 0, 0, 1], [255, 0, 0, 0], true, true),
            iface("en0", [192, 168, 1, 20], [255, 255, 255, 0], true, false),
            iface("en1", [10, 0, 0, 5], [255, 0, 0, 0], false, false),
            iface("utun", [100, 64, 0, 2], [255, 192, 0, 0], true, false),
            iface("en2", [172, 20, 3, 4], [255, 255, 0, 0], true, false),
        ];
        let got = listeners(&[], &ifaces).unwrap();
        assert_eq!(
            got,
            [
                Subnet {
                    addr: [192, 168, 1, 20].into(),
                    mask: [255, 255, 255, 0].into()
                },
                Subnet {
                    addr: [172, 20, 3, 4].into(),
                    mask: [255, 255, 0, 0].into()
                },
            ]
        );
        assert!(listeners(&[], &ifaces[..1]).is_err());
    }

    #[test]
    fn default_listeners_skip_vpn_vm_and_container_interfaces() {
        let mut ifaces: Vec<Iface> = [
            "utun4",
            "bridge100",
            "vmnet8",
            "vboxnet0",
            "docker0",
            "veth12ab",
            "virbr0",
            "br-1a2b",
            "tun0",
            "tap0",
            "wg0",
            "tailscale0",
            "ipsec0",
            "ppp0",
            "feth0",
            "llw0",
            "awdl0",
            "anpi0",
        ]
        .iter()
        .enumerate()
        .map(|(n, name)| iface(name, [10, 9, n as u8, 1], [255, 255, 255, 0], true, false))
        .collect();
        assert!(listeners(&[], &ifaces).is_err());
        ifaces.push(iface(
            "en0",
            [192, 168, 1, 20],
            [255, 255, 255, 0],
            true,
            false,
        ));
        ifaces.push(iface("eth0", [10, 0, 0, 5], [255, 0, 0, 0], true, false));
        let got: Vec<Ipv4Addr> = listeners(&[], &ifaces)
            .unwrap()
            .iter()
            .map(|s| s.addr)
            .collect();
        assert_eq!(
            got,
            [Ipv4Addr::new(192, 168, 1, 20), Ipv4Addr::new(10, 0, 0, 5)]
        );
        // Explicitly, a virtual interface's address is still allowed.
        let got = listeners(&[[10, 9, 1, 1].into()], &ifaces).unwrap();
        assert_eq!(got[0].addr, Ipv4Addr::new(10, 9, 1, 1));
    }

    #[test]
    fn host_must_name_the_listener() {
        let lan = Subnet {
            addr: [192, 168, 1, 20].into(),
            mask: [255, 255, 255, 0].into(),
        };
        for ok in ["192.168.1.20", "192.168.1.20:38471"] {
            assert!(host_allowed(Some(ok), &lan), "{ok}");
        }
        for bad in [
            "rebind.attacker.example:38471",
            "localhost:38471",
            "127.0.0.1:38471",
            "192.168.1.21:38471",
            "192.168.1.20:",
            "192.168.1.20:x",
            "192.168.1.20.evil.example",
            "",
        ] {
            assert!(!host_allowed(Some(bad), &lan), "{bad}");
        }
        assert!(!host_allowed(None, &lan));
        let lo = Subnet {
            addr: Ipv4Addr::LOCALHOST,
            mask: [255, 0, 0, 0].into(),
        };
        for ok in ["127.0.0.1:1", "127.4.5.6", "localhost", "LocalHost:38471"] {
            assert!(host_allowed(Some(ok), &lo), "{ok}");
        }
        for bad in [
            "evil.example:38471",
            "localhost.evil.example",
            "192.168.1.20",
        ] {
            assert!(!host_allowed(Some(bad), &lo), "{bad}");
        }
    }

    #[test]
    fn explicit_listeners_take_the_interface_mask_and_loopback_is_slash_8() {
        let ifaces = [iface(
            "en0",
            [192, 168, 1, 20],
            [255, 255, 255, 0],
            true,
            false,
        )];
        let got = listeners(&[[127, 0, 0, 1].into(), [192, 168, 1, 20].into()], &ifaces).unwrap();
        assert_eq!(got[0].prefix_len(), 8);
        assert_eq!(got[1].prefix_len(), 24);
        assert!(listeners(&[[192, 168, 9, 9].into()], &ifaces).is_err());
        assert!(listeners(&[Ipv4Addr::UNSPECIFIED], &ifaces).is_err());
    }

    #[test]
    fn peers_must_be_ipv4_in_the_listener_subnet() {
        let lan = Subnet {
            addr: [192, 168, 1, 20].into(),
            mask: [255, 255, 255, 0].into(),
        };
        assert!(peer_allowed("192.168.1.77".parse().unwrap(), &lan));
        assert!(!peer_allowed("192.168.2.77".parse().unwrap(), &lan));
        assert!(!peer_allowed("127.0.0.1".parse().unwrap(), &lan));
        assert!(!peer_allowed("::1".parse().unwrap(), &lan));
        assert!(!peer_allowed("::ffff:192.168.1.77".parse().unwrap(), &lan));
        let lo = Subnet {
            addr: Ipv4Addr::LOCALHOST,
            mask: [255, 0, 0, 0].into(),
        };
        assert!(peer_allowed("127.3.4.5".parse().unwrap(), &lo));
        assert!(!peer_allowed("192.168.1.77".parse().unwrap(), &lo));
    }

    #[test]
    fn origins() {
        let allowed = vec!["http://localhost:1420".to_string()];
        let host = Some("192.168.1.20:38471");
        assert_eq!(check_origin(None, host, &allowed), OriginCheck::Allow);
        assert_eq!(
            check_origin(Some("http://192.168.1.20:38471"), host, &allowed),
            OriginCheck::Allow
        );
        assert_eq!(
            check_origin(Some("http://localhost:1420"), host, &allowed),
            OriginCheck::Cors
        );
        for bad in [
            "null",
            "https://192.168.1.20:38471",
            "http://192.168.1.20:38471.evil.example",
            "http://evil.example",
        ] {
            assert_eq!(
                check_origin(Some(bad), host, &allowed),
                OriginCheck::Deny,
                "{bad}"
            );
        }
        // Without a Host header only the allow list helps.
        assert_eq!(
            check_origin(Some("http://192.168.1.20:38471"), None, &allowed),
            OriginCheck::Deny
        );
    }
}
