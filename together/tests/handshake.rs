//! End-to-end check over a real iroh connection: two endpoints in one
//! process, connected by the share code, exchanging state and clock probes.
//!
//! This is the only test that proves the pieces fit — the unit tests cover the
//! offset maths, but nothing else exercises the handshake, the framing, or the
//! translation of the host's timestamp onto the guest's clock.
//!
//! Both endpoints are on loopback, so they find each other through their
//! direct addresses and never touch a relay. That keeps the test hermetic
//! apart from binding a UDP socket.

use std::time::{Duration, Instant};

use together::{RemoteNow, Role, Together};

/// Poll `f` until it yields a value or the deadline passes. The session API is
/// fire-and-forget by design — the UI polls it too — so a test polls as well
/// rather than inventing a completion channel that production never uses.
fn wait_for<T>(what: &str, timeout: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return v;
        }
        if Instant::now() > deadline {
            panic!("timed out waiting for {what}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn sample_now() -> RemoteNow {
    RemoteNow {
        track_uri: "local:track:/music/a.flac".into(),
        artist: "Boards of Canada".into(),
        title: "Roygbiv".into(),
        album_title: Some("Music Has the Right to Children".into()),
        cover_url: Some("https://img.example/roygbiv.jpg".into()),
        duration_ns: Duration::from_secs(151).as_nanos() as u64,
        pos_ns: Duration::from_secs(42).as_nanos() as u64,
        at_ns: 0, // stamped by Together::publish
        playing: true,
        playback_id: 7,
    }
}

#[test]
fn guest_receives_host_state_on_a_translated_clock() {
    let host = Together::new();
    let guest = Together::new();

    host.host("host".into());
    let code = wait_for("the host's session code", Duration::from_secs(20), || {
        host.snapshot().ticket
    });

    host.publish(Some(sample_now()));
    guest.join(code, "guest".into());

    // The first Now can only arrive after a clock probe has landed, because
    // an untranslated timestamp is deliberately dropped.
    let target = wait_for("the host's state to reach the guest", Duration::from_secs(30), || {
        guest.snapshot().target
    });

    assert_eq!(target.title, "Roygbiv");
    assert_eq!(target.artist, "Boards of Canada");
    assert_eq!(
        target.album_title.as_deref(),
        Some("Music Has the Right to Children")
    );
    assert_eq!(
        target.cover_url.as_deref(),
        Some("https://img.example/roygbiv.jpg")
    );
    assert_eq!(target.playback_id, 7);
    assert!(target.playing);

    // Translation landed us on *our* timeline: the host stamped `at_ns` on its
    // own clock, and after translation it must sit at or before our own now.
    // A raw host timestamp would be off by the difference between two
    // unrelated process epochs, which is unbounded in either direction.
    let now = guest.now_ns();
    assert!(
        target.at_ns <= now,
        "translated timestamp {} is in our future (now {now})",
        target.at_ns
    );
    assert!(
        now - target.at_ns < Duration::from_secs(15).as_nanos() as u64,
        "translated timestamp is implausibly old"
    );

    host.publish(None);
    wait_for(
        "the host's stopped state to reach the guest",
        Duration::from_secs(10),
        || guest.snapshot().stopped.then_some(()),
    );

    let snap = guest.snapshot();
    assert_eq!(snap.role, Role::Guest);
    assert!(snap.rtt_ms.is_some(), "no clock probe completed");
    assert!(
        snap.peers.iter().any(|p| p == "host"),
        "guest did not record the host: {:?}",
        snap.peers
    );

    // Host side saw the guest arrive.
    let peers = wait_for("the host to see its guest", Duration::from_secs(10), || {
        let p = host.snapshot().peers;
        (!p.is_empty()).then_some(p)
    });
    assert!(peers.iter().any(|p| p == "guest"), "host peers: {peers:?}");

    guest.leave();
    host.leave();
}

#[test]
fn a_malformed_code_is_rejected_without_panicking() {
    let t = Together::new();
    t.join("not-a-real-code".into(), "guest".into());
    let status = wait_for("a failure to surface", Duration::from_secs(10), || {
        let s = t.snapshot();
        (!s.status.is_empty()).then_some(s)
    });
    assert_eq!(status.role, Role::Off, "a bad code must not leave us joined");
}
