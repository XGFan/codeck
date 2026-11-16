//! The Holder connection against a fake Holder speaking the toh frame protocol on a unix socket.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc;
use std::time::Duration;

use codeck_term::holder::{self, Link};
use toh_proto::{Decoder, Frame};

/// Reads frames until one arrives (or the connection ends).
fn next(s: &mut UnixStream, dec: &mut Decoder) -> Option<Frame> {
    let mut buf = [0u8; 4096];
    loop {
        if let Some(f) = dec.next_frame().expect("frame") {
            return Some(f);
        }
        let n = s.read(&mut buf).ok()?;
        if n == 0 {
            return None;
        }
        dec.push(&buf[..n]);
    }
}

fn recv(rx: &mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    rx.recv_timeout(Duration::from_secs(5)).expect("message")
}

#[test]
fn forwards_the_holders_frames_and_never_resizes() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("h.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let (tx, rx) = mpsc::channel();
    let link = Link::attach(&sock, move |m| tx.send(m).is_ok()).unwrap();
    let (mut h, _) = listener.accept().unwrap();
    h.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut dec = Decoder::new();

    let Some(Frame::Attach(info)) = next(&mut h, &mut dec) else {
        panic!("first frame is not Attach");
    };
    assert!(info.size_events, "asks for the PTY size");
    assert!(!info.hosting);

    // What a Holder sends on attach: size, history, the current screen, then output.
    let frames = [
        Frame::Size {
            cols: 120,
            rows: 40,
        },
        Frame::Scrollback {
            start_line: 0,
            data: b"old line\r\n".to_vec(),
        },
        Frame::Snapshot(b"\x1b[2J\x1b[Hscreen".to_vec()),
        Frame::Output(b"more".to_vec()),
    ];
    for f in &frames {
        h.write_all(&f.encode()).unwrap();
    }
    let mut size = vec![holder::SIZE];
    size.extend_from_slice(&120u16.to_le_bytes());
    size.extend_from_slice(&40u16.to_le_bytes());
    assert_eq!(recv(&rx), size);
    assert_eq!(recv(&rx), b"Bold line\r\n");
    assert_eq!(recv(&rx), b"P\x1b[2J\x1b[Hscreen");
    assert_eq!(recv(&rx), b"Omore");

    // Another size: Claude Code redraws, so a fresh Snapshot is asked for.
    h.write_all(
        &Frame::Size {
            cols: 100,
            rows: 30,
        }
        .encode(),
    )
    .unwrap();
    assert_eq!(recv(&rx)[0], holder::SIZE);
    assert_eq!(next(&mut h, &mut dec), Some(Frame::SnapshotReq));

    // Typing reaches the PTY as it is.
    link.input("中文\x1b".as_bytes().to_vec()).unwrap();
    assert_eq!(
        next(&mut h, &mut dec),
        Some(Frame::Input("中文\x1b".as_bytes().to_vec()))
    );

    // Dropping the link detaches; nothing on this connection ever was a Resize.
    drop(link);
    let mut rest = vec![];
    while let Some(f) = next(&mut h, &mut dec) {
        rest.push(f);
    }
    assert_eq!(rest, vec![Frame::Detach]);
    // The reader saw the connection end.
    assert_eq!(recv(&rx), vec![holder::CLOSED]);
}

#[test]
fn reports_the_shell_exit_and_the_end_of_the_connection() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("h.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let (tx, rx) = mpsc::channel();
    let _link = Link::attach(&sock, move |m| tx.send(m).is_ok()).unwrap();
    let (mut h, _) = listener.accept().unwrap();
    h.write_all(&Frame::Exit { code: 3 }.encode()).unwrap();
    drop(h);
    let mut exit = vec![holder::EXIT];
    exit.extend_from_slice(&3i32.to_le_bytes());
    assert_eq!(recv(&rx), exit);
    assert_eq!(recv(&rx), vec![holder::CLOSED]);
}

#[test]
fn attaching_to_a_missing_socket_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let err = Link::attach(&dir.path().join("none.sock"), |_| true)
        .err()
        .unwrap();
    assert!(err.starts_with("连不上 Pane"), "{err}");
}

#[test]
fn finds_the_socket_in_toh_ls() {
    let list = br#"[{"backend":"clay","cwd":"/x","hidden":true,"id":"m1","shell_pid":1,"socket":"/s/m1.sock"},
                    {"backend":"clay","cwd":"/y","id":"m2","shell_pid":2,"socket":"/s/m2.sock"}]"#;
    assert_eq!(
        holder::socket_in(list, "m2").unwrap(),
        std::path::PathBuf::from("/s/m2.sock")
    );
    assert_eq!(holder::socket_in(list, "m3").unwrap_err(), "找不到 Pane m3");
    assert!(
        holder::socket_in(b"ID HOLDER", "m1")
            .unwrap_err()
            .contains("不是 JSON")
    );
}
