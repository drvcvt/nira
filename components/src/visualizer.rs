//! Fullscreen audio visualizer overlay.
//!
//! Split of labour: Rust owns ALL the analysis (sample tap → FFT →
//! log-spaced bands → beat detection, see `player::viz`); this component
//! pumps ~30 analysis frames/s into a single long-lived `document::eval`
//! whose JS renders a grayscale radial spectrum + beat-driven particles on
//! a canvas. Techniques cherry-picked from the classics: cava's
//! gravity fall-off and Monstercat neighbour smoothing for the bars,
//! MilkDrop-style feedback trails (fade instead of clear), Parallelcube's
//! energy-ratio beat detection (done in Rust).
//!
//! librespot is its own audio engine with no tap — while Spotify plays the
//! overlay shows a hint instead of fake motion.

use dioxus::document;
use dioxus::prelude::*;
use hooks::use_player;

/// Open/close state, provided by the shell so the bottombar button, the V
/// keybind bridge and the overlay itself share it.
#[derive(Clone, Copy)]
pub struct VizOpen(pub Signal<bool>);

pub fn use_viz_open() -> VizOpen {
    use_context::<VizOpen>()
}

/// Frame pump cadence. 30 fps of *data*; the canvas interpolates at rAF
/// speed, so the picture stays smooth even between frames.
const PUMP_MS: u64 = 33;

#[component]
pub fn Visualizer() -> Element {
    let mut open = use_viz_open().0;
    let player = use_player();
    let is_open = *open.read();

    // Long-lived pump: creates the eval (and with it the JS render loop)
    // when the overlay opens, feeds it analysis frames, drops it on close.
    use_hook({
        let player = player.clone();
        move || {
            spawn(async move {
                let mut eval: Option<document::Eval> = None;
                let mut logged_flow = false;
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(PUMP_MS)).await;
                    if !*open.peek() {
                        // Dropping the eval rejects the JS-side recv(),
                        // which ends its render loop.
                        eval = None;
                        continue;
                    }
                    if eval.is_none() {
                        tracing::info!("viz: starting renderer eval");
                        eval = Some(document::eval(RENDERER_JS));
                    }
                    let snap = player.snapshot();
                    let frame = if snap.has_source && !snap.is_paused {
                        player.viz_frame()
                    } else {
                        None
                    };
                    if frame.is_some() && !logged_flow {
                        logged_flow = true;
                        tracing::info!("viz: analysis frames flowing");
                    }
                    if let Some(e) = eval.as_mut()
                        && let Err(err) = e.send(&frame)
                    {
                        tracing::warn!(error = %err, "viz: eval send failed, restarting renderer");
                        eval = None;
                    }
                }
            });
        }
    });

    if !is_open {
        return rsx! {};
    }

    let snap = player.snapshot();
    let np = snap.now_playing.clone();
    let title = np.as_ref().map(|n| n.title.clone()).unwrap_or_default();
    let artist = np.as_ref().map(|n| n.artist.clone()).unwrap_or_default();
    let spotify_active = snap.active == hooks::Active::Spotify;

    rsx! {
        div { class: "viz-overlay",
            canvas { id: "nira-viz-canvas", class: "viz-canvas" }
            button {
                class: "viz-close",
                title: "Close (Esc)",
                onclick: move |_| open.set(false),
                i { class: "fa-solid fa-xmark" }
            }
            if !title.is_empty() {
                div { class: "viz-np",
                    span { class: "viz-np-title", "{title}" }
                    span { class: "viz-np-artist", "{artist}" }
                }
            }
            if spotify_active {
                div { class: "viz-hint",
                    "Spotify plays in its own engine — the visualizer follows local/SoundCloud/the hi-res provider audio."
                }
            }
        }
    }
}

/// The canvas renderer. Runs inside the webview; receives analysis frames
/// via `dioxus.recv()` (null = paused/no data → decay to idle).
///
/// IMPORTANT: Dioxus wraps this in an AsyncFunction and closes the eval
/// channel the moment its promise resolves — so the recv loop lives at the
/// TOP LEVEL (keeping the promise pending) instead of a detached IIFE.
const RENDERER_JS: &str = r#"
  let cv = null;
  for (let tries = 0; !cv && tries < 120; tries++) {
    cv = document.getElementById('nira-viz-canvas');
    if (!cv) await new Promise(r => setTimeout(r, 16));
  }
  if (cv) {
    const ctx = cv.getContext('2d');
    const dpr = window.devicePixelRatio || 1;
    let W, H;
    const fit = () => { W = cv.width = cv.clientWidth * dpr; H = cv.height = cv.clientHeight * dpr; };
    fit();
    window.addEventListener('resize', fit);
    const css = getComputedStyle(document.documentElement);
    const fg = (css.getPropertyValue('--fg') || '#ECECEC').trim();
    const bg = (css.getPropertyValue('--bg') || '#121212').trim();

    // Immediate static mark so a stuck data path is distinguishable from
    // a renderer that never booted: faint idle ring at the centre.
    ctx.strokeStyle = fg;
    ctx.globalAlpha = 0.25;
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.arc(W / 2, H / 2, Math.min(W, H) * 0.15, 0, 7);
    ctx.stroke();
    ctx.globalAlpha = 1;

    let latest = null;

    const N = 48;
    let disp = new Array(N).fill(0);      // displayed band heights
    let ring = 0;                          // beat ring life
    let energy = 0;                        // smoothed overall level
    const parts = [];
    const spawn = (n, speed) => {
      for (let i = 0; i < n; i++) {
        const a = Math.random() * Math.PI * 2;
        const v = (0.5 + Math.random()) * speed;
        parts.push({
          x: W / 2, y: H / 2,
          vx: Math.cos(a) * v, vy: Math.sin(a) * v,
          life: 0.9 + Math.random() * 0.3,
          r: (0.8 + Math.random() * 2.2) * dpr,
        });
      }
    };

    const frame = () => {
      // Overlay closed → canvas unmounts → the loop ends itself.
      if (!document.getElementById('nira-viz-canvas')) return;
      requestAnimationFrame(frame);

      // MilkDrop-ish feedback: fade the last frame instead of clearing.
      ctx.globalCompositeOperation = 'source-over';
      ctx.globalAlpha = 0.20;
      ctx.fillStyle = bg;
      ctx.fillRect(0, 0, W, H);
      ctx.globalAlpha = 1;

      const d = latest;
      const bands = (d && d.bands) || null;
      const bass = (d && d.bass) || 0;

      // cava: bars rise instantly, fall with gravity.
      for (let i = 0; i < N; i++) {
        const t = bands ? (bands[i] || 0) : 0;
        if (t > disp[i]) disp[i] = t;
        else disp[i] = Math.max(t, disp[i] - 0.012 - disp[i] * 0.055);
      }
      // Monstercat neighbour smoothing.
      const sm = disp.slice();
      for (let i = 0; i < N; i++) {
        for (let k = 1; k <= 3; k++) {
          const v = disp[i] / Math.pow(1.7, k);
          if (i - k >= 0 && v > sm[i - k]) sm[i - k] = v;
          if (i + k < N && v > sm[i + k]) sm[i + k] = v;
        }
      }
      energy = energy * 0.92 + bass * 0.08;

      const cx = W / 2, cy = H / 2;
      const R = Math.min(W, H) * (0.15 + energy * 0.03);

      // Radial spectrum, mirrored left/right like a butterfly.
      ctx.strokeStyle = fg;
      ctx.lineCap = 'round';
      ctx.lineWidth = Math.max(2, Math.min(W, H) / 340);
      for (let i = 0; i < N; i++) {
        const len = sm[i] * Math.min(W, H) * 0.24;
        if (len < 1) continue;
        for (const side of [1, -1]) {
          const ang = -Math.PI / 2 + side * ((i + 0.5) / N) * Math.PI;
          ctx.globalAlpha = 0.22 + sm[i] * 0.7;
          ctx.beginPath();
          ctx.moveTo(cx + Math.cos(ang) * R, cy + Math.sin(ang) * R);
          ctx.lineTo(cx + Math.cos(ang) * (R + len), cy + Math.sin(ang) * (R + len));
          ctx.stroke();
        }
      }
      ctx.globalAlpha = 1;

      // Beat: ring pulse + particle burst from the centre.
      if (d && d.beat) {
        spawn(24 + Math.floor(bass * 46), (2.6 + bass * 3.2) * dpr);
        ring = 1;
      }
      if (ring > 0) {
        ctx.globalAlpha = ring * 0.45;
        ctx.lineWidth = Math.max(1.5, dpr);
        ctx.beginPath();
        ctx.arc(cx, cy, R * (2.0 - ring * 0.85), 0, 7);
        ctx.stroke();
        ctx.globalAlpha = 1;
        ring -= 0.05;
      }

      // Ambient drizzle while music plays; hard cap keeps it lightweight.
      if (bass > 0.1 && parts.length < 420) spawn(1 + Math.floor(bass * 3), 1.1 * dpr);
      ctx.fillStyle = fg;
      for (let i = parts.length - 1; i >= 0; i--) {
        const p = parts[i];
        p.x += p.vx; p.y += p.vy;
        p.vx *= 0.986; p.vy *= 0.986;
        p.life -= 0.009;
        if (p.life <= 0) { parts.splice(i, 1); continue; }
        ctx.globalAlpha = Math.max(0, Math.min(1, p.life)) * 0.75;
        ctx.beginPath();
        ctx.arc(p.x, p.y, p.r * (0.5 + p.life * 0.6), 0, 7);
        ctx.fill();
      }
      ctx.globalAlpha = 1;
    };
    requestAnimationFrame(frame);

    // Top-level await keeps the eval promise pending → channel stays open.
    while (true) {
      latest = await dioxus.recv();
    }
  }
"#;
