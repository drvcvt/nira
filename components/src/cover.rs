//! Fullscreen "now playing" cover overlay with a physics-driven vinyl disc.
//!
//! Opened from the bottombar's mini cover. Dioxus owns the DOM (art, copy,
//! seek, transport) and mirrors play/pause into a `data-playing` attribute;
//! an injected script owns everything per-frame — disc rotation physics,
//! drag/flick, glow intensity and the background crossfade — because a
//! 250 Hz signal loop through the Rust↔webview bridge is a non-starter.
//! Same teardown pattern as the visualizer: generation token + isConnected.

use std::time::{Duration, Instant};

use dioxus::document;
use dioxus::prelude::*;
use hooks::{RepeatMode, fmt_time, use_player, use_queue};

/// Open/close state, provided by the shell so the bottombar art button, the
/// Escape bridge and the overlay itself share it.
#[derive(Clone, Copy)]
pub struct CoverOpen(pub Signal<bool>);

pub fn use_cover_open() -> CoverOpen {
    use_context::<CoverOpen>()
}

/// How long the close animation runs before the overlay unmounts — must
/// cover the longest `.closing` animation in cover.css (400 ms overlay fade).
const CLOSE_MS: u64 = 400;

#[component]
pub fn CoverOverlay() -> Element {
    let mut open = use_cover_open().0;
    let player = use_player();
    let queue = use_queue();
    let mut closing = use_signal(|| false);

    // Boot the physics script on every open. The script waits for the disc
    // element itself (the overlay may not be mounted yet when this runs) and
    // is synchronous top-to-bottom, so dropping the eval handle can't cancel
    // the rAF loop it schedules.
    use_effect(move || {
        let is_open = *open.read();
        if is_open {
            document::eval(VINYL_JS);
        }
        // Trap entry / hand-back: the overlay is fully opaque, so without
        // this Tab walked onto the sidebar and bottombar behind it.
        crate::overlay_focus(is_open, ".cover-overlay .cover-play");
    });

    // Scrub-hold state — same pattern as the bottombar seek: paint from
    // `scrub` while it exists and clear once the engine converges on the
    // drop point or the 3 s backstop expires.
    let mut scrub: Signal<Option<(String, f64)>> = use_signal(|| None);
    let mut scrub_dragging = use_signal(|| false);
    let mut scrub_committed: Signal<Option<Instant>> = use_signal(|| None);
    {
        let player = player.clone();
        use_effect(move || {
            let snap = player.snapshot();
            if *scrub_dragging.peek() {
                return;
            }
            let Some((_, target)) = scrub.peek().clone() else {
                return;
            };
            let live = snap
                .duration
                .filter(|d| d.as_secs() > 0)
                .map(|d| (snap.position.as_secs_f64() / d.as_secs_f64()) * 100.0)
                .unwrap_or(0.0);
            let expired = (*scrub_committed.peek())
                .map(|t0| t0.elapsed() > Duration::from_secs(3))
                .unwrap_or(true);
            if (target - live).abs() <= 1.5 || expired {
                scrub.set(None);
                scrub_committed.set(None);
            }
        });
    }

    if !*open.read() {
        return rsx! {};
    }

    let request_close = move |_: Event<MouseData>| {
        if *closing.peek() {
            return;
        }
        closing.set(true);
        spawn(async move {
            tokio::time::sleep(Duration::from_millis(CLOSE_MS)).await;
            open.set(false);
            closing.set(false);
        });
    };

    let snap = player.snapshot();
    let np = snap.now_playing.clone();
    let playing = snap.has_source && !snap.is_paused;
    let transport_locked = snap.transport_locked;
    // Same idle fallback the bottombar has: with a restored queue but nothing
    // loaded yet, show the pending entry rather than "Nothing playing" — the
    // play button below starts exactly that track, so claiming nothing is
    // playing while the bar two pixels down shows the title was just wrong.
    let idle_track = (!snap.has_source && np.is_none())
        .then(|| {
            let entries = queue.entries.read();
            (*queue.current_index.read()).and_then(|i| entries.get(i).cloned())
        })
        .flatten();
    let cover_url = np
        .as_ref()
        .and_then(|n| n.cover_url.clone())
        .or_else(|| idle_track.as_ref().and_then(|t| t.cover_url.clone()))
        .unwrap_or_default();
    let title = np
        .as_ref()
        .map(|n| n.title.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| idle_track.as_ref().map(|t| t.title.clone()))
        .unwrap_or_else(|| "Nothing playing".to_string());
    let artist = np
        .as_ref()
        .map(|n| n.artist.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            idle_track.as_ref().map(|t| {
                t.artists
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "—".to_string());

    // Progress, with the same stale-progress blanking as the bar.
    let track_loading = *queue.is_loading_track.read();
    let duration = if track_loading { None } else { snap.duration };
    let position = if track_loading { Duration::ZERO } else { snap.position };
    let track_key = np
        .as_ref()
        .and_then(|n| n.track_uri.clone())
        .unwrap_or_default();
    let live_pct = match duration {
        Some(d) if d.as_secs() > 0 => {
            ((position.as_secs_f64() / d.as_secs_f64()) * 100.0).clamp(0.0, 100.0)
        }
        _ => 0.0,
    };
    let scrub_val: Option<f64> = scrub
        .read()
        .as_ref()
        .filter(|(key, _)| *key == track_key)
        .map(|(_, v)| *v);
    let progress_pct = scrub_val.unwrap_or(live_pct);
    let position_str = fmt_time(position.as_secs());
    let duration_str = duration
        .map(|d| fmt_time(d.as_secs()))
        .unwrap_or_else(|| "--:--".to_string());

    let has_prev = queue.has_previous();
    let has_next = queue.has_next();
    let queue_len = queue.entries.read().len();
    let shuffle_on = *queue.shuffle_enabled.read();
    let repeat_mode = *queue.repeat_mode.read();
    let repeat_title = match repeat_mode {
        RepeatMode::Off => "Repeat off",
        RepeatMode::All => "Repeat all",
        RepeatMode::One => "Repeat one",
    };

    rsx! {
        div {
            class: if *closing.read() { "cover-overlay closing" } else { "cover-overlay" },
            "data-playing": if playing { "true" } else { "false" },

            // Visual backdrop: two crossfade panes (background-image set by
            // the script), scrim, vignettes. Never intercepts the pointer.
            div { class: "cover-bg",
                div { id: "nira-cover-bg-a", class: "cover-bg-pane",
                    div { class: "cover-bg-blur" }
                }
                div { id: "nira-cover-bg-b", class: "cover-bg-pane",
                    div { class: "cover-bg-blur" }
                }
                div { class: "cover-scrim" }
                div { class: "cover-vignette" }
            }
            // Click-outside catcher — content sits above it, so only clicks
            // on the background land here. Same shape as the queue overlay.
            button {
                class: "cover-backdrop",
                r#type: "button",
                tabindex: "-1",
                "aria-hidden": "true",
                onclick: request_close,
            }
            // Bridge for the shell's Escape handler.
            button {
                id: "nira-key-cover-close",
                class: "hotkey-bridge",
                r#type: "button",
                tabindex: "-1",
                onclick: request_close,
            }

            div { class: "cover-content",
                div { class: "cover-disc-wrap",
                    div { id: "nira-vinyl-glow", class: "cover-glow" }
                    div { id: "nira-vinyl-disc", class: "cover-disc",
                        if !cover_url.is_empty() {
                            img {
                                id: "nira-vinyl-art",
                                src: "{cover_url}",
                                alt: "",
                                draggable: "false",
                            }
                        } else {
                            i { class: "fa-solid fa-music" }
                        }
                        div { class: "cover-hole" }
                    }
                }

                div { class: "cover-copy",
                    div { class: "cover-title", title: "{title}", "{title}" }
                    div { class: "cover-artist", "{artist}" }
                }

                div { class: "cover-seek-row",
                    span { class: "cover-time", "{position_str}" }
                    div {
                        class: "cover-seek",
                        style: "--seek-pct: {progress_pct}%;",
                        input {
                            r#type: "range",
                            class: "cover-seek-input",
                            min: "0",
                            max: "1000",
                            step: "1",
                            value: "{(progress_pct * 10.0) as i64}",
                            disabled: transport_locked || duration.map(|d| d.as_secs() == 0).unwrap_or(true),
                            title: if transport_locked { "Following host" } else { "Seek" },
                            "aria-label": if transport_locked { "Following host" } else { "Seek" },
                            onpointerdown: move |_| {
                                scrub.set(None);
                                scrub_dragging.set(true);
                            },
                            onpointerup: move |_| scrub_dragging.set(false),
                            onpointercancel: move |_| {
                                scrub_dragging.set(false);
                                scrub.set(None);
                            },
                            oninput: {
                                let player = player.clone();
                                let dur = duration;
                                let track_key = track_key.clone();
                                move |evt: FormEvent| {
                                    let Ok(v) = evt.value().parse::<f64>() else { return; };
                                    let Some(d) = dur else { return; };
                                    let pct = (v / 10.0).clamp(0.0, 100.0);
                                    scrub.set(Some((track_key.clone(), pct)));
                                    let target =
                                        Duration::from_secs_f64(d.as_secs_f64() * pct / 100.0);
                                    player.seek(target);
                                    scrub_committed.set(Some(Instant::now()));
                                }
                            },
                        }
                    }
                    span { class: "cover-time", "{duration_str}" }
                }

                div { class: "cover-controls",
                    button {
                        class: if shuffle_on { "cover-btn active" } else { "cover-btn" },
                        title: if shuffle_on { "Shuffle on" } else { "Shuffle off" },
                        "aria-label": if shuffle_on { "Shuffle on" } else { "Shuffle off" },
                        "aria-pressed": if shuffle_on { "true" } else { "false" },
                        disabled: transport_locked || queue_len < 2,
                        onclick: {
                            let queue = queue.clone();
                            move |_| queue.toggle_shuffle()
                        },
                        i { class: "fa-solid fa-shuffle" }
                    }
                    button {
                        class: "cover-btn",
                        title: if transport_locked { "Following host" } else { "Previous" },
                        "aria-label": if transport_locked { "Following host" } else { "Previous track" },
                        disabled: transport_locked || !has_prev,
                        onclick: {
                            let queue = queue.clone();
                            move |_| queue.previous()
                        },
                        i { class: "fa-solid fa-backward-step" }
                    }
                    button {
                        class: "cover-btn cover-play",
                        title: if transport_locked { "Following host" } else if playing { "Pause" } else { "Play" },
                        "aria-label": if transport_locked { "Following host" } else if playing { "Pause" } else { "Play" },
                        disabled: transport_locked,
                        onclick: {
                            let player = player.clone();
                            let queue = queue.clone();
                            move |_| {
                                if player.toggle() {
                                    return;
                                }
                                let idx = (*queue.current_index.peek()).unwrap_or(0);
                                queue.play_index(idx);
                            }
                        },
                        if playing {
                            i { class: "fa-solid fa-pause" }
                        } else {
                            i { class: "fa-solid fa-play" }
                        }
                    }
                    button {
                        class: "cover-btn",
                        title: if transport_locked { "Following host" } else { "Next" },
                        "aria-label": if transport_locked { "Following host" } else { "Next track" },
                        disabled: transport_locked || !has_next,
                        onclick: {
                            let queue = queue.clone();
                            move |_| queue.next()
                        },
                        i { class: "fa-solid fa-forward-step" }
                    }
                    button {
                        class: match repeat_mode {
                            RepeatMode::Off => "cover-btn",
                            RepeatMode::All => "cover-btn active",
                            RepeatMode::One => "cover-btn active repeat-one",
                        },
                        title: "{repeat_title}",
                        "aria-label": "{repeat_title}",
                        disabled: transport_locked,
                        onclick: {
                            let queue = queue.clone();
                            move |_| queue.cycle_repeat()
                        },
                        i { class: "fa-solid fa-repeat" }
                    }
                }
            }
        }
    }
}


/// The per-frame engine, translated from the QML spec's 4 ms Timer into a
/// dt-normalized rAF loop (n = dt/4 spec ticks per frame; the exponential
/// approach uses 1-(1-t)^n so the feel is framerate-independent). Velocity
/// stays in the spec's unit — degrees per 4 ms tick — so the spec constants
/// (base 0.048 = 12 °/s, flick clamp ±8 = 2000 °/s) carry over verbatim;
/// only the smoothing tiers are halved by request so a flick coasts longer
/// before returning to base speed. The hidden-timer branch (32 ms / scale 8)
/// is dropped: the overlay unmounts when closed, there is nothing to keep
/// simulating. Style writes are suppressed while the disc is visibly still.
const VINYL_JS: &str = r#"
(function () {
  // Generation token — a reopen boots a fresh loop, older ones retire.
  const gen = (window.__niraVinylGen = (window.__niraVinylGen || 0) + 1);
  let tries = 0;
  function boot() {
    if (gen !== window.__niraVinylGen) return;
    const overlay = document.querySelector('.cover-overlay');
    const disc = document.getElementById('nira-vinyl-disc');
    const glow = document.getElementById('nira-vinyl-glow');
    if (!overlay || !disc || !glow) {
      if (++tries < 120) requestAnimationFrame(boot);
      return;
    }
    const retired = () => !disc.isConnected || gen !== window.__niraVinylGen;
    const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

    // -- background crossfade: two panes, back pane gets the new art and
    //    fades over the front (600ms InOutQuart transition in CSS).
    const panes = [
      document.getElementById('nira-cover-bg-a'),
      document.getElementById('nira-cover-bg-b'),
    ];
    if (reduceMotion) {
      for (const pane of panes) if (pane) pane.style.transition = 'none';
    }
    let front = 0, shownSrc = null;
    const setPane = (pane, src) => {
      for (const el of pane.children)
        el.style.backgroundImage = src ? 'url("' + src + '")' : 'none';
    };

    // -- physics state. Spec units: velocity in deg per 4ms tick.
    const BASE = 0.048, MAXV = 8;
    let rot = 0, vel = 0, dragging = false, prevAngle = 0, samples = [];
    const angleAt = (e) => {
      const r = disc.getBoundingClientRect();
      return Math.atan2(
        e.clientY - (r.top + r.height / 2),
        e.clientX - (r.left + r.width / 2)
      ) * 180 / Math.PI;
    };
    disc.addEventListener('pointerdown', (e) => {
      if (retired() || reduceMotion) return;
      dragging = true;
      vel = 0;
      prevAngle = angleAt(e);
      samples = [{ rot: rot, t: performance.now() }];
      try { disc.setPointerCapture(e.pointerId); } catch (err) {}
      e.preventDefault();
    });
    disc.addEventListener('pointermove', (e) => {
      if (!dragging) return;
      const a = angleAt(e);
      let d = a - prevAngle;
      if (d > 180) d -= 360; else if (d < -180) d += 360;
      prevAngle = a;
      rot += d;
      const t = performance.now();
      samples.push({ rot: rot, t: t });
      while (samples.length > 1 && t - samples[0].t > 80) samples.shift();
    });
    const release = () => {
      if (!dragging) return;
      dragging = false;
      const a = samples[0], b = samples[samples.length - 1];
      // deg/ms -> deg per 4ms tick, clamped to +-8 (2000 deg/s).
      vel = (a && b && b.t > a.t)
        ? Math.max(-MAXV, Math.min(MAXV, ((b.rot - a.rot) / (b.t - a.t)) * 4))
        : 0;
    };
    disc.addEventListener('pointerup', release);
    disc.addEventListener('pointercancel', release);

    let last = performance.now(), lastGlow = -1, lastRot = null;
    function frame(now) {
      if (retired()) return;
      requestAnimationFrame(frame);
      const dt = Math.min(now - last, 100);
      last = now;
      if (!reduceMotion && !dragging) {
        const n = dt / 4; // spec ticks elapsed this frame
        const target = overlay.dataset.playing === 'true' ? BASE : 0;
        const speed = Math.abs(vel);
        // Halved from the spec's 0.003/0.006/0.01 tiers — a flick glides
        // back to base speed over roughly twice the time.
        const t = speed > 2 ? 0.0015 : speed > 0.5 ? 0.003 : 0.006;
        vel += (target - vel) * (1 - Math.pow(1 - t, n));
        // Settle instead of asymptoting, so the idle guard below engages.
        if (target === 0 && Math.abs(vel) < 0.0004) vel = 0;
        rot = (rot + vel * n) % 360;
      }
      // Idle guard: no style writes when nothing visibly moved — a paused,
      // settled disc costs zero paint per frame.
      if (lastRot === null || Math.abs(rot - lastRot) > 0.02) {
        disc.style.transform = 'rotate(' + rot + 'deg)';
        lastRot = rot;
      }
      const g = 0.5 + Math.min(0.5, Math.abs(vel) * 0.05);
      if (Math.abs(g - lastGlow) > 0.04) {
        glow.style.opacity = g;
        lastGlow = g;
      }
      // Track-change watch: crossfade the blurred background to the new art.
      const art = document.getElementById('nira-vinyl-art');
      const src = (art && (art.currentSrc || art.src)) || '';
      if (src !== shownSrc && panes[0] && panes[1]) {
        const instant = shownSrc === null; // first paint rides the open fade
        shownSrc = src;
        const back = 1 - front;
        setPane(panes[back], src);
        if (instant) {
          panes[back].style.transition = 'none';
          panes[back].style.opacity = '1';
          void panes[back].offsetWidth;
          panes[back].style.transition = '';
        } else {
          panes[back].style.opacity = '1';
        }
        panes[front].style.opacity = '0';
        front = back;
      }
    }
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(boot);
})();
"#;

#[cfg(test)]
mod tests {
    use super::VINYL_JS;

    #[test]
    fn vinyl_physics_keeps_spec_constants_and_teardown() {
        // Spec constants survive translation: 12 °/s base spin, ±8 flick
        // clamp, 80 ms flick sample window, 4 ms tick normalization.
        assert!(VINYL_JS.contains("BASE = 0.048, MAXV = 8"));
        assert!(VINYL_JS.contains("samples[0].t > 80"));
        assert!(VINYL_JS.contains("dt / 4"));
        // Framerate-independent smoothing (halved tiers) + the retire path.
        assert!(VINYL_JS.contains("Math.pow(1 - t, n)"));
        assert!(VINYL_JS.contains("speed > 2 ? 0.0015"));
        assert!(VINYL_JS.contains("!disc.isConnected"));
        // Idle guard: a settled disc must not write styles every frame.
        assert!(VINYL_JS.contains("Math.abs(rot - lastRot)"));
        // No top-level await — the eval handle is dropped immediately, so
        // the script must finish synchronously and live on via rAF.
        assert!(!VINYL_JS.contains("await"));
    }

    #[test]
    fn vinyl_respects_reduced_motion_without_hiding_track_changes() {
        assert!(VINYL_JS.contains("prefers-reduced-motion: reduce"));
        assert!(VINYL_JS.contains("if (!reduceMotion && !dragging)"));
        assert!(VINYL_JS.contains("src !== shownSrc"));
    }
}
