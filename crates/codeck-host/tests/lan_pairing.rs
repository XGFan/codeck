//! Host-level tests for the four desktop LAN pairing commands (`lan_pair_start`, `lan_pair_cancel`,
//! `lan_devices`, `lan_remove_device`), URL/QR generation, and event emission.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;

use codeck_host::lan::LanConfig;
use codeck_host::lan::net::Subnet;
use codeck_host::lan::pairing::create_pairing_offer;
use codeck_host::lan::security::RedeemError;
use codeck_host::lan::server::event_channel;
use codeck_host::{Host, HostConfig, HostEvent, dispatch};
use parking_lot::Mutex;
use serde_json::{Value, json};

fn parse_query(url: &str) -> HashMap<String, String> {
    let q = url.strip_prefix("codeck://pair?").expect("pairing scheme");
    q.split('&')
        .map(|kv| {
            let (k, v) = kv.split_once('=').expect("k=v");
            (k.to_owned(), v.to_owned())
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn desktop_pairing_commands_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let web = dir.path().join("web");
    std::fs::create_dir_all(&web).unwrap();

    let free = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = free.local_addr().unwrap().port();
    drop(free);

    let (events, lan_sink) = event_channel();
    let captured: Arc<Mutex<Vec<HostEvent>>> = Arc::default();
    let sink = {
        let captured = Arc::clone(&captured);
        Arc::new(move |e: HostEvent| {
            lan_sink(e.clone());
            captured.lock().push(e);
        })
    };

    let host = Host::start(
        HostConfig {
            cache_dir: dir.path().join("cache"),
            settings_path: dir.path().join("settings.json"),
            ccrpc_logs: Some(dir.path().join("logs")),
            images: Some(dir.path().join("images")),
            data_root: Some(dir.path().join("root")),
            agy_root: dir.path().join("__agy"),
            haa_home: dir.path().join("__haa"),
            refresh_prices: false,
            lan: Some(LanConfig {
                events,
                web_dir: web,
                port,
                subnets: Arc::new(|| {
                    Ok(vec![Subnet {
                        addr: Ipv4Addr::LOCALHOST,
                        mask: Ipv4Addr::new(255, 0, 0, 0),
                    }])
                }),
            }),
            ..HostConfig::desktop()
        },
        sink,
    )
    .unwrap();

    // 1. Reading `lan_devices` while LAN access is off works and never creates a TLS certificate.
    assert!(host.lan_devices().await.unwrap().is_empty());
    assert!(!dir.path().join("tls").exists());

    // 2. Starting a pairing offer while LAN access is off reports the spec message.
    let err = host.lan_pair_start().await.unwrap_err();
    assert_eq!(err.message, "先打开局域网访问。");

    // 3. With LAN access on but only a loopback listener, `lan_pair_start` reports no LAN address.
    host.lan_set_enabled(true).await.unwrap();
    let err = host.lan_pair_start().await.unwrap_err();
    assert_eq!(err.message, "没有可用的局域网地址。");

    // 4. `create_pairing_offer` (the core of `lan_pair_start`) filters RFC 1918 addresses, builds the
    //    exact URL, matches HostCert fingerprint, and generates an `<svg` with viewBox.
    let sec = host.lan_security().await.unwrap();
    let cert_fp = sec.ensure_cert().unwrap().fingerprint.clone();
    let err = create_pairing_offer(
        &sec,
        port,
        &["127.0.0.1:38471".into(), "100.64.0.2:38471".into()],
    )
    .unwrap_err();
    assert_eq!(err, "没有可用的局域网地址。");

    let offer = create_pairing_offer(
        &sec,
        port,
        &[
            "127.0.0.1:38471".into(),
            "100.64.0.2:38471".into(),
            format!("192.168.1.20:{port}"),
            format!("10.0.0.5:{port}"),
        ],
    )
    .unwrap();
    assert!(offer.svg.starts_with("<svg"), "{}", offer.svg);
    assert!(offer.svg.contains("viewBox="), "{}", offer.svg);
    let params = parse_query(&offer.url);
    assert_eq!(
        params.get("h").map(String::as_str),
        Some("192.168.1.20,10.0.0.5")
    );
    assert_eq!(
        params.get("p").map(String::as_str),
        Some(port.to_string().as_str())
    );
    assert_eq!(params.get("fp").map(String::as_str), Some(cert_fp.as_str()));
    let key = params.get("k").unwrap();
    assert_eq!(key.len(), 22);

    // Five private listening addresses are capped to the first four in interface order.
    let offer_capped = create_pairing_offer(
        &sec,
        port,
        &[
            format!("192.168.1.20:{port}"),
            format!("10.0.0.5:{port}"),
            format!("172.16.0.9:{port}"),
            format!("192.168.50.2:{port}"),
            format!("10.2.3.4:{port}"),
        ],
    )
    .unwrap();
    let capped_params = parse_query(&offer_capped.url);
    assert_eq!(
        capped_params.get("h").map(String::as_str),
        Some("192.168.1.20,10.0.0.5,172.16.0.9,192.168.50.2")
    );
    // Re-create the original offer so step 5 tests cancelling `offer.id`.
    let offer = create_pairing_offer(
        &sec,
        port,
        &[
            "127.0.0.1:38471".into(),
            "100.64.0.2:38471".into(),
            format!("192.168.1.20:{port}"),
            format!("10.0.0.5:{port}"),
        ],
    )
    .unwrap();
    let key = parse_query(&offer.url).remove("k").unwrap();

    // 5. `lan_pair_cancel` with a wrong id does NOT cancel; with the matching id it DOES cancel.
    host.lan_pair_cancel("wrong-id".into()).await.unwrap();
    host.lan_pair_cancel(offer.id.clone()).await.unwrap();
    assert!(matches!(
        sec.redeem(&key, "iPhone"),
        Err(RedeemError::Expired)
    ));

    // Wrong id leaves an active offer redeemable.
    let offer2 = create_pairing_offer(&sec, port, &[format!("192.168.1.20:{port}")]).unwrap();
    let key2 = parse_query(&offer2.url).remove("k").unwrap();
    host.lan_pair_cancel("still-wrong".into()).await.unwrap();
    let paired = sec.redeem(&key2, "My iPhone").unwrap();

    // 6. Turning LAN access off invalidates any active pairing offer.
    let offer3 = create_pairing_offer(&sec, port, &[format!("192.168.1.20:{port}")]).unwrap();
    let key3 = parse_query(&offer3.url).remove("k").unwrap();
    host.lan_set_enabled(false).await.unwrap();
    assert!(matches!(
        sec.redeem(&key3, "iPhone"),
        Err(RedeemError::Expired)
    ));

    // 7. `lan_devices` remains readable when LAN access is off; `lan_remove_device` ignores unknown
    //    ids and emits `HostEvent::LanDevices` with null offer/device ids when removing a real one.
    let list = host.lan_devices().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, paired.device.id);

    captured.lock().clear();
    host.lan_remove_device("no-such-device".into())
        .await
        .unwrap();
    assert!(
        !captured
            .lock()
            .iter()
            .any(|e| matches!(e, HostEvent::LanDevices { .. }))
    );

    host.lan_remove_device(paired.device.id.clone())
        .await
        .unwrap();
    assert!(host.lan_devices().await.unwrap().is_empty());
    let ev = captured
        .lock()
        .iter()
        .find_map(|e| match e {
            HostEvent::LanDevices {
                devices,
                paired_offer,
                paired_device,
            } => Some((devices.clone(), paired_offer.clone(), paired_device.clone())),
            _ => None,
        })
        .expect("HostEvent::LanDevices was not emitted on remove");
    assert!(ev.0.is_empty());
    assert_eq!(ev.1, None);
    assert_eq!(ev.2, None);

    // 8. All four methods are local-only in the dispatch table.
    for (method, args) in [
        ("lan_pair_start", Value::Null),
        ("lan_pair_cancel", json!({ "id": "x" })),
        ("lan_devices", Value::Null),
        ("lan_remove_device", json!({ "id": "x" })),
    ] {
        let err = dispatch(&host, method, args, true).await.unwrap_err();
        assert_eq!(
            err.code,
            codeck_core::model::ErrorCode::InvalidQuery,
            "{method}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn desktop_lan_reports_lock_conflict_and_does_not_listen() {
    let dir = tempfile::tempdir().unwrap();
    let web = dir.path().join("web");
    std::fs::create_dir_all(&web).unwrap();

    let free = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = free.local_addr().unwrap().port();
    drop(free);

    // Simulate another `codeck-host` instance holding `daemon.lock` on the same directory.
    let other_lock = codeck_host::lan::control::lock_state_dir(dir.path()).unwrap();

    let (events, sink) = event_channel();
    let host = Host::start(
        HostConfig {
            cache_dir: dir.path().join("cache"),
            settings_path: dir.path().join("settings.json"),
            ccrpc_logs: Some(dir.path().join("logs")),
            images: Some(dir.path().join("images")),
            data_root: Some(dir.path().join("root")),
            agy_root: dir.path().join("__agy"),
            haa_home: dir.path().join("__haa"),
            refresh_prices: false,
            lan: Some(LanConfig {
                events,
                web_dir: web,
                port,
                subnets: Arc::new(|| {
                    Ok(vec![Subnet {
                        addr: Ipv4Addr::LOCALHOST,
                        mask: Ipv4Addr::new(255, 0, 0, 0),
                    }])
                }),
            }),
            ..HostConfig::desktop()
        },
        sink,
    )
    .unwrap();

    let status = host.lan_set_enabled(true).await.unwrap();
    assert!(status.enabled);
    assert!(status.addresses.is_empty());
    assert_eq!(
        status.error.as_deref(),
        Some(format!("另一个 codeck-host 正在使用 {}", dir.path().display()).as_str())
    );
    assert!(
        tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .is_err()
    );

    // Once the other instance releases the lock, turning LAN access off and on succeeds.
    drop(other_lock);
    host.lan_set_enabled(false).await.unwrap();
    let ok = host.lan_set_enabled(true).await.unwrap();
    assert_eq!(ok.error, None);
    assert_eq!(ok.addresses, [format!("127.0.0.1:{port}")]);
    host.lan_set_enabled(false).await.unwrap();
}
