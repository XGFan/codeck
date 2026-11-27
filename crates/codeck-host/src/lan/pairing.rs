//! In-memory single-use pairing offers (5-minute TTL on a monotonic clock).

use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use super::devices::{PairedDevice, hex_lower, random_bytes};
use super::security::LanSecurity;

/// How long a pairing QR code / key remains valid.
pub const PAIRING_TTL: Duration = Duration::from_secs(5 * 60);
/// Maximum number of candidate LAN addresses included in a pairing URL (matches iOS parser limit).
pub const MAX_PAIRING_HOSTS: usize = 4;

/// A freshly generated pairing offer for the desktop settings modal.
#[derive(Serialize, Deserialize, specta::Type, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PairingOffer {
    pub id: String,
    pub url: String,
    pub svg: String,
    /// Wall-clock expiration timestamp in epoch milliseconds.
    #[specta(type = specta_typescript::Number)]
    pub expires_at: f64,
}

/// Filters `"ip:port"` listener strings down to RFC 1918 IPv4 addresses, preserving listening order.
pub fn private_ipv4_addrs(addresses: &[String]) -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    for raw in addresses {
        let ip = raw
            .parse::<SocketAddrV4>()
            .map(|a| *a.ip())
            .or_else(|_| raw.parse::<Ipv4Addr>());
        if let Ok(ip) = ip
            && ip.is_private()
            && !out.contains(&ip)
        {
            out.push(ip);
        }
    }
    out
}

/// Builds `codeck://pair?h=<IPv4>[,…]&p=<port>&fp=<fingerprint>&k=<key>`, keeping at most
/// [`MAX_PAIRING_HOSTS`] distinct RFC 1918 addresses in their given order. Returns `None` when no
/// RFC 1918 address is available.
pub fn build_pairing_url(
    hosts: &[Ipv4Addr],
    port: u16,
    fingerprint: &str,
    key: &str,
) -> Option<String> {
    let mut private = Vec::new();
    for &ip in hosts {
        if ip.is_private() && !private.contains(&ip) {
            private.push(ip);
            if private.len() == MAX_PAIRING_HOSTS {
                break;
            }
        }
    }
    if private.is_empty() {
        return None;
    }
    let h = private
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    Some(format!(
        "codeck://pair?h={h}&p={port}&fp={fingerprint}&k={key}"
    ))
}

/// Renders a pairing URL as a standalone SVG starting with `<svg` (with `viewBox`).
pub fn render_qr_svg(url: &str) -> Result<String, String> {
    let code =
        qrcode::QrCode::new(url.as_bytes()).map_err(|e| format!("生成配对二维码失败：{e}"))?;
    let raw = code
        .render::<qrcode::render::svg::Color<'_>>()
        .min_dimensions(240, 240)
        .dark_color(qrcode::render::svg::Color("#111111"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();
    // Strip the XML declaration so callers get a `<svg …>` root directly.
    Ok(raw.find("<svg").map(|i| raw[i..].to_owned()).unwrap_or(raw))
}

/// Starts a new pairing offer on `security` for the RFC 1918 addresses currently in `listening`.
pub fn create_pairing_offer(
    security: &LanSecurity,
    port: u16,
    listening: &[String],
) -> Result<PairingOffer, String> {
    let hosts = private_ipv4_addrs(listening);
    if hosts.is_empty() {
        return Err("没有可用的局域网地址。".into());
    }
    let cert = security.ensure_cert()?;
    let started = security.start_pairing()?;
    let url = build_pairing_url(&hosts, port, &cert.fingerprint, &started.key)
        .ok_or_else(|| "没有可用的局域网地址。".to_string())?;
    let svg = render_qr_svg(&url)?;
    Ok(PairingOffer {
        id: started.id,
        url,
        svg,
        expires_at: started.expires_at_ms,
    })
}

/// Result delivered to a waiter (e.g. `codeck-host pair`) for a specific pairing offer.
#[derive(Clone, Debug, PartialEq)]
pub enum PairOutcome {
    Paired(PairedDevice),
    Expired,
}

/// Public details of a freshly started pairing offer, plus its oneshot completion channel.
pub struct StartedOffer {
    pub id: String,
    pub key: String,
    pub deadline: Instant,
    /// Wall-clock expiration timestamp in epoch milliseconds (display only).
    pub expires_at_ms: f64,
    pub outcome: oneshot::Receiver<PairOutcome>,
}

struct ActiveOffer {
    id: String,
    key: String,
    deadline: Instant,
    notify: Option<oneshot::Sender<PairOutcome>>,
}

impl ActiveOffer {
    fn expire(mut self) {
        if let Some(tx) = self.notify.take() {
            let _ = tx.send(PairOutcome::Expired);
        }
    }
}

/// Holds at most one active pairing offer in memory.
#[derive(Default)]
pub struct Pairing {
    current: Option<ActiveOffer>,
}

impl Pairing {
    /// Starts a new offer, invalidating any previous one (notifying its waiter that it expired).
    pub fn start(
        &mut self,
        now: Instant,
        now_ms: i64,
        ttl: Duration,
    ) -> Result<StartedOffer, String> {
        if let Some(old) = self.current.take() {
            old.expire();
        }
        let id = hex_lower(&random_bytes::<6>()?);
        let key_bytes = random_bytes::<16>()?;
        let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key_bytes);
        let deadline = now + ttl;
        let expires_at_ms = (now_ms + ttl.as_millis() as i64) as f64;
        let (tx, rx) = oneshot::channel();
        self.current = Some(ActiveOffer {
            id: id.clone(),
            key: key.clone(),
            deadline,
            notify: Some(tx),
        });
        Ok(StartedOffer {
            id,
            key,
            deadline,
            expires_at_ms,
            outcome: rx,
        })
    }

    /// Cancels the current offer only if its id matches `id`.
    pub fn cancel(&mut self, id: &str) -> bool {
        if self.current.as_ref().is_some_and(|o| o.id == id) {
            if let Some(old) = self.current.take() {
                old.expire();
            }
            return true;
        }
        false
    }

    /// Cancels whichever offer is currently active (e.g. when LAN access is turned off).
    pub fn cancel_any(&mut self) {
        if let Some(old) = self.current.take() {
            old.expire();
        }
    }

    #[cfg(test)]
    pub(crate) fn is_active(&self) -> bool {
        self.current.is_some()
    }

    /// Checks whether `key` matches the current unexpired offer without consuming it yet, so the
    /// caller can persist `devices.json` before committing the redemption.
    pub fn check(&mut self, key: &str, now: Instant) -> bool {
        let Some(offer) = self.current.as_ref() else {
            return false;
        };
        if now >= offer.deadline {
            if let Some(expired) = self.current.take() {
                expired.expire();
            }
            return false;
        }
        eq_fixed_bytes(offer.key.as_bytes(), key.as_bytes())
    }

    /// Consumes the active offer after device persistence succeeded, notifying its waiter and
    /// returning the redeemed offer id.
    pub fn complete(&mut self, device: PairedDevice) -> Option<String> {
        let mut offer = self.current.take()?;
        if let Some(tx) = offer.notify.take() {
            let _ = tx.send(PairOutcome::Paired(device));
        }
        Some(offer.id)
    }
}

fn eq_fixed_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_device() -> PairedDevice {
        PairedDevice {
            id: "0123456789ab".into(),
            name: "iPhone".into(),
            paired_at: 1000.0,
            last_seen_at: None,
        }
    }

    #[test]
    fn offer_expires_replaces_cancels_by_id_and_redeems_once() {
        let mut p = Pairing::default();
        let t0 = Instant::now();

        let mut o1 = p.start(t0, 10_000, Duration::from_secs(300)).unwrap();
        assert_eq!(o1.key.len(), 22);
        assert_eq!(o1.expires_at_ms, 310_000.0);

        // Wrong id does not cancel.
        assert!(!p.cancel("not-this-id"));
        assert!(p.check(&o1.key, t0 + Duration::from_secs(299)));

        // Starting a new offer expires the old one and notifies its waiter.
        let mut o2 = p.start(t0, 10_000, Duration::from_secs(300)).unwrap();
        assert_eq!(o1.outcome.try_recv().unwrap(), PairOutcome::Expired);
        assert!(!p.check(&o1.key, t0));
        assert!(!p.check("wrong-key", t0));

        // Cancel matching id expires o2.
        assert!(p.cancel(&o2.id));
        assert_eq!(o2.outcome.try_recv().unwrap(), PairOutcome::Expired);
        assert!(!p.check(&o2.key, t0));

        // Expiration by monotonic clock.
        let mut o3 = p.start(t0, 10_000, Duration::from_secs(300)).unwrap();
        assert!(!p.check(&o3.key, t0 + Duration::from_secs(300)));
        assert_eq!(o3.outcome.try_recv().unwrap(), PairOutcome::Expired);

        // Single-use redemption notifies waiter with device.
        let mut o4 = p.start(t0, 10_000, Duration::from_secs(300)).unwrap();
        assert!(p.check(&o4.key, t0 + Duration::from_secs(1)));
        let dev = dummy_device();
        assert_eq!(p.complete(dev.clone()), Some(o4.id.clone()));
        assert_eq!(o4.outcome.try_recv().unwrap(), PairOutcome::Paired(dev));
        assert!(!p.check(&o4.key, t0 + Duration::from_secs(2)));
    }

    #[test]
    fn pairing_url_filters_rfc1918_preserves_order_and_renders_svg() {
        let listening: Vec<String> = [
            "127.0.0.1:38471",
            "100.64.0.2:38471",
            "192.168.1.20:38471",
            "8.8.8.8:38471",
            "10.0.0.5:38471",
            "172.16.2.3:38471",
            "192.168.1.20:38471",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let hosts = private_ipv4_addrs(&listening);
        assert_eq!(
            hosts,
            [
                Ipv4Addr::new(192, 168, 1, 20),
                Ipv4Addr::new(10, 0, 0, 5),
                Ipv4Addr::new(172, 16, 2, 3),
            ]
        );
        assert!(build_pairing_url(&[Ipv4Addr::LOCALHOST], 38471, "fp", "key").is_none());

        let url = build_pairing_url(&hosts, 38471, "fp_43", "key_22").unwrap();
        assert_eq!(
            url,
            "codeck://pair?h=192.168.1.20,10.0.0.5,172.16.2.3&p=38471&fp=fp_43&k=key_22"
        );
        let svg = render_qr_svg(&url).unwrap();
        assert!(svg.starts_with("<svg"), "{svg}");
        assert!(svg.contains("viewBox="), "{svg}");
        assert!(svg.ends_with("</svg>"), "{svg}");
    }

    #[test]
    fn pairing_url_caps_hosts_at_four_preserving_interface_order() {
        let hosts = [
            Ipv4Addr::LOCALHOST,
            Ipv4Addr::new(192, 168, 10, 1),
            Ipv4Addr::new(10, 0, 0, 2),
            Ipv4Addr::new(192, 168, 10, 1), // duplicate
            Ipv4Addr::new(172, 16, 0, 3),
            Ipv4Addr::new(192, 168, 20, 4),
            Ipv4Addr::new(10, 1, 2, 5), // 5th distinct private address -> dropped
        ];
        let url = build_pairing_url(&hosts, 38471, "fp_43", "key_22").unwrap();
        assert_eq!(
            url,
            "codeck://pair?h=192.168.10.1,10.0.0.2,172.16.0.3,192.168.20.4&p=38471&fp=fp_43&k=key_22"
        );
    }
}
