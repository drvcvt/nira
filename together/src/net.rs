//! iroh transport: endpoint lifecycle, framing, and the per-connection loop.
//!
//! Runs on its own thread with its own current-thread runtime, the same shape
//! `mpris_bridge` uses — a long-lived network task has no business sharing the
//! runtime that drives the UI, and a scope-tied `spawn` would die the moment
//! the settings page unmounts.

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr};

use crate::{ALPN, BEAT, Msg, RemoteNow, Role, Together};

/// Largest frame we will allocate for. A `Now` is a few hundred bytes; this is
/// four orders of magnitude of headroom and still refuses a peer that claims a
/// 4 GiB message, which is the only thing the check is guarding against.
const MAX_FRAME: u32 = 1 << 20;

/// Encode an address as a share string. `EndpointAddr` is serde-serialisable
/// but has no string form of its own, so we pick one: postcard + URL-safe
/// base64, which survives being pasted into any chat window.
pub fn encode_ticket(addr: &EndpointAddr) -> Result<String> {
    Ok(B64.encode(postcard::to_allocvec(addr)?))
}

pub fn decode_ticket(s: &str) -> Result<EndpointAddr> {
    let raw = B64
        .decode(s.trim())
        .context("that does not look like a nira session code")?;
    postcard::from_bytes(&raw).context("session code is malformed or from another version")
}

async fn send(w: &mut iroh::endpoint::SendStream, msg: &Msg) -> Result<()> {
    let body = postcard::to_allocvec(msg)?;
    w.write_all(&(body.len() as u32).to_le_bytes()).await?;
    w.write_all(&body).await?;
    Ok(())
}

async fn recv(r: &mut iroh::endpoint::RecvStream) -> Result<Msg> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let len = u32::from_le_bytes(len);
    if len > MAX_FRAME {
        return Err(anyhow!("frame of {len} bytes refused"));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body).await?;
    Ok(postcard::from_bytes(&body)?)
}

impl Together {
    /// Start hosting. Returns once the endpoint is bound and the share string
    /// is available from [`Together::snapshot`]; guests are accepted in the
    /// background until [`Together::leave`].
    pub fn host(&self, display_name: String) {
        self.spawn_session(move |t, rt| {
            rt.block_on(async move {
                let ep = match bind().await {
                    Ok(ep) => ep,
                    Err(e) => return t.fail(format!("could not open the network endpoint: {e}")),
                };
                match encode_ticket(&ep.addr()) {
                    Ok(code) => {
                        *t.inner.ticket.write().unwrap_or_else(|p| p.into_inner()) = Some(code);
                        t.set_status("waiting for someone to join");
                    }
                    Err(e) => return t.fail(format!("could not build a session code: {e}")),
                }

                while let Some(connecting) = ep.accept().await {
                    let t = t.clone();
                    let name = display_name.clone();
                    tokio::spawn(async move {
                        match connecting.await {
                            Ok(conn) => {
                                if let Err(e) = t.serve_guest(conn, name).await {
                                    tracing::info!(error = %e, "together: guest disconnected");
                                }
                            }
                            Err(e) => tracing::warn!(error = %e, "together: handshake failed"),
                        }
                    });
                }
            })
        });
    }

    /// Join a session from a share string.
    pub fn join(&self, code: String, display_name: String) {
        self.spawn_session(move |t, rt| {
            rt.block_on(async move {
                let addr = match decode_ticket(&code) {
                    Ok(a) => a,
                    Err(e) => return t.fail(e.to_string()),
                };
                let ep = match bind().await {
                    Ok(ep) => ep,
                    Err(e) => return t.fail(format!("could not open the network endpoint: {e}")),
                };
                t.set_status("connecting…");
                let conn = match ep.connect(addr, ALPN).await {
                    Ok(c) => c,
                    Err(e) => return t.fail(format!("could not reach the host: {e}")),
                };
                if let Err(e) = t.follow_host(conn, display_name).await {
                    t.fail(format!("connection to the host ended: {e}"));
                }
            })
        });
    }

    /// Host side of one guest connection: announce state on every beat, and
    /// answer clock probes as they arrive.
    async fn serve_guest(&self, conn: iroh::endpoint::Connection, name: String) -> Result<()> {
        let (mut w, mut r) = conn.accept_bi().await?;
        let peer = match recv(&mut r).await? {
            Msg::Hello { name } => name,
            other => return Err(anyhow!("expected Hello, got {other:?}")),
        };
        let key = self.add_peer(peer.clone());
        self.set_status(format!("{peer} is listening along"));
        send(&mut w, &Msg::Hello { name }).await?;

        // Probes arrive on their own cadence; answering them must not wait for
        // the next beat, so the reader gets its own task and the writer owns
        // the interval.
        let replies = {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(async move {
                loop {
                    match recv(&mut r).await {
                        Ok(Msg::Ping { t1 }) => {
                            if tx.send(t1).is_err() {
                                break;
                            }
                        }
                        Ok(Msg::Bye) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            });
            rx
        };
        let mut replies = replies;

        let mut beat = tokio::time::interval(BEAT);
        let result = loop {
            tokio::select! {
                Some(t1) = replies.recv() => {
                    if let Err(e) = send(&mut w, &Msg::Pong { t1, t2: self.now_ns() }).await {
                        break Err(e);
                    }
                }
                _ = beat.tick() => {
                    let state = self.inner.publish.read().unwrap_or_else(|p| p.into_inner()).clone();
                    if let Some(mut now) = state {
                        // Stamp at send time, not at publish time — the queue
                        // watcher's sample can be up to its own tick old, and a
                        // stale `at_ns` reads to the guest as drift.
                        now.at_ns = self.now_ns();
                        if let Err(e) = send(&mut w, &Msg::Now(Box::new(now))).await {
                            break Err(e);
                        }
                    }
                }
                else => break Ok(()),
            }
        };
        self.remove_peer(key);
        result
    }

    /// Guest side: probe the clock on every beat and translate whatever the
    /// host announces onto our own timeline.
    async fn follow_host(&self, conn: iroh::endpoint::Connection, name: String) -> Result<()> {
        let (mut w, mut r) = conn.open_bi().await?;
        send(&mut w, &Msg::Hello { name }).await?;
        let host = match recv(&mut r).await? {
            Msg::Hello { name } => name,
            other => return Err(anyhow!("expected Hello, got {other:?}")),
        };
        self.add_peer(host.clone());
        *self.inner.role.write().unwrap_or_else(|p| p.into_inner()) = Role::Guest;
        self.set_status(format!("listening along with {host}"));

        let probe = {
            let t = self.clone();
            tokio::spawn(async move {
                let mut beat = tokio::time::interval(BEAT);
                loop {
                    beat.tick().await;
                    if send(&mut w, &Msg::Ping { t1: t.now_ns() }).await.is_err() {
                        break;
                    }
                }
            })
        };

        let outcome = loop {
            match recv(&mut r).await {
                Ok(Msg::Pong { t1, t2 }) => {
                    let t3 = self.now_ns();
                    self.inner
                        .sync
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .record(t1, t2, t3);
                }
                Ok(Msg::Now(now)) => self.absorb(*now),
                Ok(Msg::Bye) => break Ok(()),
                Ok(_) => {}
                Err(e) => break Err(e),
            }
        };
        probe.abort();
        outcome
    }

    /// Rewrite the host's timestamp onto our clock and store it. Dropped
    /// entirely until the first probe lands — an untranslated `at_ns` would be
    /// off by the whole inter-process clock difference, which is unbounded.
    fn absorb(&self, mut now: RemoteNow) {
        let translated = self
            .inner
            .sync
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .peer_to_local(now.at_ns);
        let Some(at) = translated else { return };
        now.at_ns = at;
        *self.inner.target.write().unwrap_or_else(|p| p.into_inner()) = Some(now);
    }

    /// Tear the session down. The runtime thread exits when its shutdown flag
    /// is observed; state is reset immediately so the UI never shows a session
    /// that is on its way out.
    pub fn leave(&self) {
        *self.inner.role.write().unwrap_or_else(|p| p.into_inner()) = Role::Off;
        *self.inner.ticket.write().unwrap_or_else(|p| p.into_inner()) = None;
        *self.inner.target.write().unwrap_or_else(|p| p.into_inner()) = None;
        *self.inner.publish.write().unwrap_or_else(|p| p.into_inner()) = None;
        self.inner
            .peers
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.set_status(String::new());
    }

    fn spawn_session<F>(&self, f: F)
    where
        F: FnOnce(Together, tokio::runtime::Runtime) + Send + 'static,
    {
        self.leave();
        *self.inner.role.write().unwrap_or_else(|p| p.into_inner()) = Role::Host;
        let t = self.clone();
        std::thread::Builder::new()
            .name("nira-together".into())
            .spawn(move || match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => f(t, rt),
                Err(e) => t.fail(format!("could not start the network runtime: {e}")),
            })
            .expect("spawn together thread");
    }

    fn fail(&self, msg: String) {
        tracing::warn!(error = %msg, "together");
        self.set_status(msg);
        *self.inner.role.write().unwrap_or_else(|p| p.into_inner()) = Role::Off;
    }

    fn add_peer(&self, name: String) -> u64 {
        let mut peers = self.inner.peers.write().unwrap_or_else(|p| p.into_inner());
        let key = peers.keys().max().copied().unwrap_or(0) + 1;
        peers.insert(key, name);
        key
    }

    fn remove_peer(&self, key: u64) {
        self.inner
            .peers
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
    }
}

async fn bind() -> Result<Endpoint> {
    Ok(Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await?)
}
