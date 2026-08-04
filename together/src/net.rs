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
use std::{future::Future, sync::Arc};

use crate::{ALPN, BEAT, Msg, RemoteNow, Role, SessionToken, Together};

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

async fn cancelled(shutdown: &mut tokio::sync::watch::Receiver<bool>) {
    if !*shutdown.borrow() {
        let _ = shutdown.changed().await;
    }
}

impl Together {
    /// Start hosting. Returns once the endpoint is bound and the share string
    /// is available from [`Together::snapshot`]; guests are accepted in the
    /// background until [`Together::leave`].
    pub fn host(&self, display_name: String) {
        self.spawn_session(Role::Host, move |t, session| async move {
            let ep = match bind().await {
                Ok(ep) => ep,
                Err(e) => {
                    return t.fail(
                        &session,
                        format!("could not open the network endpoint: {e}"),
                    );
                }
            };
            match encode_ticket(&ep.addr()) {
                Ok(code) => {
                    if !t.set_ticket(&session, code)
                        || !t.set_status(&session, "waiting for someone to join")
                    {
                        return;
                    }
                }
                Err(e) => {
                    return t.fail(&session, format!("could not build a session code: {e}"));
                }
            }

            while let Some(connecting) = ep.accept().await {
                let t = t.clone();
                let name = display_name.clone();
                let session = session.clone();
                tokio::spawn(async move {
                    match connecting.await {
                        Ok(conn) => {
                            if let Err(e) = t.serve_guest(session, conn, name).await {
                                tracing::info!(error = %e, "together: guest disconnected");
                            }
                        }
                        Err(e) => tracing::warn!(error = %e, "together: handshake failed"),
                    }
                });
            }
        });
    }

    /// Join a session from a share string.
    pub fn join(&self, code: String, display_name: String) {
        self.spawn_session(Role::Guest, move |t, session| async move {
            let addr = match decode_ticket(&code) {
                Ok(a) => a,
                Err(e) => return t.fail(&session, e.to_string()),
            };
            let ep = match bind().await {
                Ok(ep) => ep,
                Err(e) => {
                    return t.fail(
                        &session,
                        format!("could not open the network endpoint: {e}"),
                    );
                }
            };
            if !t.set_status(&session, "connecting…") {
                return;
            }
            let conn = match ep.connect(addr, ALPN).await {
                Ok(c) => c,
                Err(e) => return t.fail(&session, format!("could not reach the host: {e}")),
            };
            if let Err(e) = t.follow_host(session.clone(), conn, display_name).await {
                t.fail(&session, format!("connection to the host ended: {e}"));
            }
        });
    }

    /// Host side of one guest connection: announce state on every beat, and
    /// answer clock probes as they arrive.
    async fn serve_guest(
        &self,
        session: Arc<SessionToken>,
        conn: iroh::endpoint::Connection,
        name: String,
    ) -> Result<()> {
        let (mut w, mut r) = conn.accept_bi().await?;
        let peer = match recv(&mut r).await? {
            Msg::Hello { name } => name,
            other => return Err(anyhow!("expected Hello, got {other:?}")),
        };
        let Some(key) = self.add_peer(&session, peer.clone()) else {
            return Ok(());
        };
        if !self.set_status(&session, format!("{peer} is listening along")) {
            return Ok(());
        }
        if let Err(e) = send(&mut w, &Msg::Hello { name }).await {
            self.remove_peer(&session, key);
            return Err(e);
        }

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
                    let Some(state) = self.with_session(&session, || {
                        self.inner.publish.read().unwrap_or_else(|p| p.into_inner()).clone()
                    }) else {
                        break Ok(());
                    };
                    let msg = match state {
                        Some(now) => Msg::Now(Box::new(now)),
                        None => Msg::Stopped,
                    };
                    if let Err(e) = send(&mut w, &msg).await {
                        break Err(e);
                    }
                }
                else => break Ok(()),
            }
        };
        self.remove_peer(&session, key);
        result
    }

    /// Guest side: probe the clock on every beat and translate whatever the
    /// host announces onto our own timeline.
    async fn follow_host(
        &self,
        session: Arc<SessionToken>,
        conn: iroh::endpoint::Connection,
        name: String,
    ) -> Result<()> {
        let (mut w, mut r) = conn.open_bi().await?;
        send(&mut w, &Msg::Hello { name }).await?;
        let host = match recv(&mut r).await? {
            Msg::Hello { name } => name,
            other => return Err(anyhow!("expected Hello, got {other:?}")),
        };
        let Some(key) = self.add_peer(&session, host.clone()) else {
            return Ok(());
        };
        if !self.set_status(&session, format!("listening along with {host}")) {
            return Ok(());
        }

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
                    if self
                        .with_session(&session, || {
                            self.inner
                                .sync
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .record(t1, t2, t3);
                        })
                        .is_none()
                    {
                        break Ok(());
                    }
                }
                Ok(Msg::Now(now)) => {
                    if !self.absorb(&session, *now) {
                        break Ok(());
                    }
                }
                Ok(Msg::Stopped) => {
                    if self
                        .with_session(&session, || {
                            *self.inner.target.write().unwrap_or_else(|p| p.into_inner()) = None;
                            *self.inner.stopped.write().unwrap_or_else(|p| p.into_inner()) = true;
                        })
                        .is_none()
                    {
                        break Ok(());
                    }
                }
                Ok(Msg::Bye) => break Ok(()),
                Ok(_) => {}
                Err(e) => break Err(e),
            }
        };
        probe.abort();
        self.remove_peer(&session, key);
        outcome
    }

    /// Rewrite the host's timestamp onto our clock and store it. Dropped
    /// entirely until the first probe lands — an untranslated `at_ns` would be
    /// off by the whole inter-process clock difference, which is unbounded.
    fn absorb(&self, session: &Arc<SessionToken>, mut now: RemoteNow) -> bool {
        self.with_session(session, || {
            *self.inner.stopped.write().unwrap_or_else(|p| p.into_inner()) = false;
            let translated = self
                .inner
                .sync
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .peer_to_local(now.at_ns);
            let Some(at) = translated else { return };
            now.at_ns = at;
            *self.inner.target.write().unwrap_or_else(|p| p.into_inner()) = Some(now);
        })
        .is_some()
    }

    /// Tear the session down. The runtime thread exits when its shutdown flag
    /// is observed; state is reset immediately so the UI never shows a session
    /// that is on its way out.
    pub fn leave(&self) {
        let mut current = self.inner.session.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(session) = current.take() {
            session.shutdown.send_replace(true);
        }
        self.clear_session_state();
    }

    fn clear_session_state(&self) {
        *self.inner.role.write().unwrap_or_else(|p| p.into_inner()) = Role::Off;
        *self.inner.ticket.write().unwrap_or_else(|p| p.into_inner()) = None;
        *self.inner.target.write().unwrap_or_else(|p| p.into_inner()) = None;
        *self.inner.stopped.write().unwrap_or_else(|p| p.into_inner()) = false;
        *self.inner.publish.write().unwrap_or_else(|p| p.into_inner()) = None;
        self.inner
            .peers
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        *self.inner.status.write().unwrap_or_else(|p| p.into_inner()) = String::new();
        *self.inner.sync.lock().unwrap_or_else(|p| p.into_inner()) = crate::clock::ClockSync::new();
    }

    fn spawn_session<F, Fut>(&self, role: Role, f: F)
    where
        F: FnOnce(Together, Arc<SessionToken>) -> Fut + Send + 'static,
        Fut: Future<Output = ()>,
    {
        let (shutdown, _) = tokio::sync::watch::channel(false);
        let session = Arc::new(SessionToken { shutdown });
        {
            let mut current = self.inner.session.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(old) = current.take() {
                old.shutdown.send_replace(true);
            }
            self.clear_session_state();
            *self.inner.role.write().unwrap_or_else(|p| p.into_inner()) = role;
            *current = Some(session.clone());
        }
        let t = self.clone();
        std::thread::Builder::new()
            .name("nira-together".into())
            .spawn(move || match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(async move {
                    let mut shutdown = session.shutdown.subscribe();
                    let task = f(t, session);
                    tokio::select! {
                        biased;
                        _ = cancelled(&mut shutdown) => {}
                        _ = task => {}
                    }
                }),
                Err(e) => t.fail(&session, format!("could not start the network runtime: {e}")),
            })
            .expect("spawn together thread");
    }

    fn with_session<T>(&self, session: &Arc<SessionToken>, f: impl FnOnce() -> T) -> Option<T> {
        let current = self.inner.session.lock().unwrap_or_else(|p| p.into_inner());
        current
            .as_ref()
            .filter(|current| Arc::ptr_eq(current, session))
            .map(|_| f())
    }

    fn fail(&self, session: &Arc<SessionToken>, msg: String) {
        let mut current = self.inner.session.lock().unwrap_or_else(|p| p.into_inner());
        let active = current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, session));
        if !active {
            return;
        }
        let active = current.take().expect("active session checked");
        active.shutdown.send_replace(true);
        self.clear_session_state();
        *self.inner.status.write().unwrap_or_else(|p| p.into_inner()) = msg.clone();
        tracing::warn!(error = %msg, "together");
    }

    fn set_status(&self, session: &Arc<SessionToken>, status: impl Into<String>) -> bool {
        self.with_session(session, || {
            *self.inner.status.write().unwrap_or_else(|p| p.into_inner()) = status.into();
        })
        .is_some()
    }

    fn set_ticket(&self, session: &Arc<SessionToken>, ticket: String) -> bool {
        self.with_session(session, || {
            *self.inner.ticket.write().unwrap_or_else(|p| p.into_inner()) = Some(ticket);
        })
        .is_some()
    }

    fn add_peer(&self, session: &Arc<SessionToken>, name: String) -> Option<u64> {
        self.with_session(session, || {
            let mut peers = self.inner.peers.write().unwrap_or_else(|p| p.into_inner());
            let key = peers.keys().max().copied().unwrap_or(0) + 1;
            peers.insert(key, name);
            key
        })
    }

    fn remove_peer(&self, session: &Arc<SessionToken>, key: u64) {
        self.with_session(session, || {
            self.inner
                .peers
                .write()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&key);
        });
    }
}

async fn bind() -> Result<Endpoint> {
    Ok(Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await?)
}
