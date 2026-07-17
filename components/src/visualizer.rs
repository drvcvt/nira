//! Fullscreen audio visualizer overlay.
//!
//! Rust owns the audio: the sample tap (see `player::viz`) feeds each frame
//! the newest 1024 raw PCM bytes plus our own FFT bands/beat. The webview
//! renders. Two renderers, chosen at runtime:
//!
//! * **Butterchurn** (primary) — the MIT WebGL2 port of Winamp's MilkDrop,
//!   running real `.milk` presets. We inject the lib + a curated preset pack
//!   once, create a visualizer on the canvas, and drive it by handing our
//!   PCM straight into `render({audioLevels})` so it runs its own FFT — no
//!   Web Audio graph needed (our sound never touches the webview).
//! * **Canvas fallback** — a hand-drawn grayscale scene (anchor ring +
//!   mirrored spectrum bars + oscilloscope + dust), used when WebGL2 or the
//!   lib isn't available.
//!
//! Preset nav: ←/→ switch, auto-cycles every ~24 s, `g` toggles grayscale.
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
                // Butterchurn is injected once, lazily, on the first open and
                // kept alive for the task's lifetime (window globals persist
                // across evals; dropping the handle could cancel the script).
                let mut bc_eval: Option<document::Eval> = None;
                let mut logged_flow = false;
                loop {
                    if !*open.peek() {
                        // Dropping the eval rejects the JS-side recv(),
                        // which ends its render loop. Closed = idle tick,
                        // no reason to spin at 30 Hz for a peek.
                        eval = None;
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        continue;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(PUMP_MS)).await;
                    if bc_eval.is_none() {
                        tracing::info!("viz: injecting butterchurn");
                        bc_eval = Some(document::eval(BUTTERCHURN_JS));
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
            // Filled by the renderer JS on preset switches; Dioxus only
            // owns the (static, empty) element.
            div { id: "nira-viz-preset", class: "viz-preset" }
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

/// Butterchurn (MilkDrop-in-WebGL2) + every MilkDrop preset pack the npm
/// package ships + the texture images some presets sample; the renderer
/// then narrows to the curated ~140 in assets/curated-presets.json.
/// Injected once into the webview on the first overlay open; UMD bundles
/// that attach `window.butterchurn*` globals.
// ponytail: inlined via include_str! — simplest robust load path; cost is a
// one-time ~2.8 MB parse on first open. Serve as assets only if that parse
// ever shows up as jank.
const BUTTERCHURN_JS: &str = concat!(
    include_str!("../assets/butterchurn.min.js"),
    "\n;\n",
    include_str!("../assets/butterchurn-presets-base.min.js"),
    "\n;\n",
    include_str!("../assets/butterchurn-presets-extra.min.js"),
    "\n;\n",
    include_str!("../assets/butterchurn-presets-extra2.min.js"),
    "\n;\n",
    include_str!("../assets/butterchurn-presets-md1.min.js"),
    "\n;\n",
    include_str!("../assets/butterchurn-extra-images.min.js"),
);

/// The webview renderer. Prefers Butterchurn (real MilkDrop presets); falls
/// back to the hand-drawn canvas scene when WebGL2 or the lib is missing.
/// Receives analysis frames via `dioxus.recv()` (null = paused/no data →
/// silence for Butterchurn / idle for the canvas).
///
/// IMPORTANT: Dioxus wraps this in an AsyncFunction and closes the eval
/// channel the moment its promise resolves — so the recv loop lives at the
/// TOP LEVEL (keeping the promise pending) instead of a detached IIFE.
const RENDERER_JS: &str = concat!(
    r#"
  const CURATED = "#,
    include_str!("../assets/curated-presets.json"),
    r#";
  let cv = null;
  for (let tries = 0; !cv && tries < 120; tries++) {
    cv = document.getElementById('nira-viz-canvas');
    if (!cv) await new Promise(r => setTimeout(r, 16));
  }
  if (cv) {
    const dpr = window.devicePixelRatio || 1;
    const fit = () => { cv.width = cv.clientWidth * dpr; cv.height = cv.clientHeight * dpr; };
    fit();
    window.addEventListener('resize', fit);
    // Generation token: a newer renderer boot bumps it so any older loop
    // retires itself instead of double-drawing.
    const gen = (window.__niraVizGen = (window.__niraVizGen || 0) + 1);
    const retired = () => !document.getElementById('nira-viz-canvas') || gen !== window.__niraVizGen;

    let latest = null;

    // Prefer Butterchurn (real MilkDrop presets) when WebGL2 + the lib are
    // both available; otherwise the hand-drawn canvas scene.
    let webgl2 = false;
    try { webgl2 = !!document.createElement('canvas').getContext('webgl2'); } catch (e) {}
    let bc = null, presets = null, extraImages = null;
    if (webgl2) {
      for (let t = 0; t < 400 && !window.butterchurnExtraImages; t++) await new Promise(r => setTimeout(r, 16));
      // The UMD bundles wrap the module namespace, so the real API can sit
      // under `.default` or directly on the global — unwrap either.
      const un = (g) => g && (g.default || g);
      const lib = un(window.butterchurn);
      if (lib && lib.createVisualizer) {
        bc = lib;
        // Merge every bundled pack; first pack wins on duplicate names.
        // Sorted so left/right walks alphabetically through all ~440.
        const merged = {};
        for (const g of ['butterchurnPresets', 'butterchurnPresetsMD1',
                         'butterchurnPresetsExtra', 'butterchurnPresetsExtra2']) {
          const pmod = un(window[g]);
          if (!pmod || !pmod.getPresets) continue;
          const map = pmod.getPresets();
          for (const name of Object.keys(map)) {
            if (!(name in merged)) merged[name] = map[name];
          }
        }
        // Curated cut: the ~140 best-known presets (full showcase pack +
        // five-star MD1 classics + five-star extras) — see
        // assets/curated-presets.json. Falls back to everything if the
        // list somehow matches nothing.
        const names = CURATED.filter(n => n in merged);
        presets = (names.length ? names : Object.keys(merged).sort((a, b) => a.localeCompare(b)))
          .map(name => ({ name, preset: merged[name] }));
        const imod = un(window.butterchurnExtraImages);
        if (imod && imod.getImages) extraImages = imod.getImages();
      }
    }

    if (bc && presets && presets.length) startButterchurn();
    else startCanvas();

    // Top-level await keeps the eval promise pending -> the channel stays open.
    while (true) { latest = await dioxus.recv(); }

    // -- Butterchurn: MilkDrop presets, fed by our Rust PCM ---------------
    function startButterchurn() {
      const AC = window.AudioContext || window.webkitAudioContext;
      const audioCtx = new AC();
      const viz = bc.createVisualizer(audioCtx, cv, {
        width: cv.width, height: cv.height, pixelRatio: 1, textureRatio: 1,
      });
      if (extraImages && viz.loadExtraImages) {
        try { viz.loadExtraImages(extraImages); } catch (e) {}
      }
      // Preset-name toast, faded in/out on every switch.
      let nameTimer = null;
      const showName = () => {
        const el = document.getElementById('nira-viz-preset');
        if (!el) return;
        el.textContent = presets[idx].name;
        el.classList.add('show');
        clearTimeout(nameTimer);
        nameTimer = setTimeout(() => el.classList.remove('show'), 2400);
      };
      let idx = Math.floor(Math.random() * presets.length);
      const load = (i, blend) => {
        idx = ((i % presets.length) + presets.length) % presets.length;
        try { viz.loadPreset(presets[idx].preset, blend); } catch (e) {}
        showName();
      };
      load(idx, 0);
      let lastCycle = performance.now();
      const onKey = (e) => {
        if (retired()) return;
        // Plain keys only — Ctrl+arrows stay next/prev-track transport.
        if (e.ctrlKey || e.metaKey || e.altKey) return;
        const k = (e.key || '').toLowerCase();
        if (k === 'arrowright') { load(idx + 1, 2.7); lastCycle = performance.now(); e.preventDefault(); e.stopPropagation(); }
        else if (k === 'arrowleft') { load(idx - 1, 2.7); lastCycle = performance.now(); e.preventDefault(); e.stopPropagation(); }
        else if (k === 'g') { cv.classList.toggle('viz-mono'); e.stopPropagation(); }
      };
      window.addEventListener('keydown', onKey, true);
      const ta = new Uint8Array(1024);
      const draw = () => {
        if (retired()) {
          window.removeEventListener('resize', fit);
          window.removeEventListener('keydown', onKey, true);
          return;
        }
        requestAnimationFrame(draw);
        if (cv.width !== cv.clientWidth * dpr || cv.height !== cv.clientHeight * dpr) {
          fit(); viz.setRendererSize(cv.width, cv.height);
        }
        const d = latest;
        if (d && d.pcm) ta.set(d.pcm); else ta.fill(128);
        viz.render({ audioLevels: { timeByteArray: ta, timeByteArrayL: ta, timeByteArrayR: ta } });
        // Auto-cycle presets while audio actually flows.
        if (d && d.pcm && performance.now() - lastCycle > 24000) { load(idx + 1, 2.7); lastCycle = performance.now(); }
      };
      requestAnimationFrame(draw);
    }

    // -- Canvas fallback: hand-drawn grayscale scene ----------------------
    function startCanvas() {
      const ctx = cv.getContext('2d');
      const css = getComputedStyle(document.documentElement);
      const fg = (css.getPropertyValue('--fg') || '#ECECEC').trim();
      const bg = (css.getPropertyValue('--bg') || '#121212').trim();
      const hexA = (hex, a) => {
        const v = parseInt(hex.slice(1), 16);
        return `rgba(${(v >> 16) & 255},${(v >> 8) & 255},${v & 255},${a})`;
      };
      const N = 48;                          // must match player::viz::BANDS
      const P = 2 * N;
      const sm = new Array(N).fill(0);
      let energy = 0, idleA = 0, beatSeen = null, beatEnv = 0;
      const dust = [];
      for (let i = 0; i < 110; i++) {
        dust.push({
          ang: Math.random() * Math.PI * 2,
          home: 0.24 + Math.random() * 0.22,
          r: 0, vr: 0,
          drift: (Math.random() - 0.5) * 0.0016,
          size: 0.6 + Math.random() * 1.5,
          tw: Math.random() * Math.PI * 2,
          tws: 0.008 + Math.random() * 0.02,
        });
      }
      dust.forEach(p => { p.r = p.home; });
      let lastT = performance.now();
      const frame = () => {
        if (retired()) { window.removeEventListener('resize', fit); return; }
        requestAnimationFrame(frame);
        if (cv.width !== cv.clientWidth * dpr || cv.height !== cv.clientHeight * dpr) fit();
        const W = cv.width, H = cv.height;
        const now = performance.now();
        const dt = Math.min(3, Math.max(0.25, (now - lastT) / 16.667));
        lastT = now;
        ctx.setTransform(1, 0, 0, 1, 0, 0);
        ctx.globalCompositeOperation = 'source-over';
        ctx.globalAlpha = 1;
        ctx.fillStyle = bg;
        ctx.fillRect(0, 0, W, H);
        const d = latest;
        const bands = (d && d.bands) || null;
        const bass = (d && d.bass) || 0;
        const md = Math.min(W, H);
        const cx = W / 2, cy = H / 2;
        const atk = 1 - Math.pow(0.25, dt), rel = 1 - Math.pow(0.90, dt);
        for (let i = 0; i < N; i++) {
          const tv = bands ? (bands[i] || 0) : 0;
          sm[i] += (tv - sm[i]) * (tv > sm[i] ? atk : rel);
        }
        energy += (bass - energy) * (1 - Math.pow(0.93, dt));
        idleA += ((d ? 0 : 1) - idleA) * (1 - Math.pow(0.95, dt));
        const R0 = md * (0.19 + energy * 0.05);
        const glowA = 0.015 + energy * 0.06;
        if (glowA > 0.005) {
          const g = ctx.createRadialGradient(cx, cy, 0, cx, cy, R0 * 1.9);
          g.addColorStop(0, hexA(fg, glowA));
          g.addColorStop(1, hexA(fg, 0));
          ctx.fillStyle = g;
          ctx.fillRect(cx - R0 * 2, cy - R0 * 2, R0 * 4, R0 * 4);
        }
        const pts = new Array(P);
        for (let i = 0; i < N; i++) {
          pts[i] = { ang: -Math.PI / 2 + ((i + 0.5) / N) * Math.PI, v: sm[i] };
          pts[P - 1 - i] = { ang: -Math.PI / 2 - ((i + 0.5) / N) * Math.PI, v: sm[i] };
        }
        {
          const prev = pts.map(p => p.v);
          for (let j = 0; j < P; j++) {
            pts[j].v = 0.22 * prev[(j + P - 1) % P] + 0.56 * prev[j] + 0.22 * prev[(j + 1) % P];
          }
        }
        const lwBase = Math.max(1.6, md / 520);
        const R = R0 * (1 + beatEnv * 0.02);
        ctx.strokeStyle = fg;
        ctx.lineWidth = lwBase * 0.8;
        ctx.globalAlpha = 0.22 * (1 - idleA * 0.6);
        ctx.beginPath();
        ctx.arc(cx, cy, R, 0, 7);
        ctx.stroke();
        const gap = Math.max(4, md * 0.008);
        ctx.lineCap = 'round';
        ctx.lineWidth = lwBase * 1.5;
        for (const p of pts) {
          const v = Math.min(1, p.v);
          const len = Math.pow(v, 1.25) * md * 0.21;
          if (len < 0.5) continue;
          const ca = Math.cos(p.ang), sa = Math.sin(p.ang);
          ctx.globalAlpha = 0.32 + v * 0.6;
          ctx.beginPath();
          ctx.moveTo(cx + ca * (R + gap), cy + sa * (R + gap));
          ctx.lineTo(cx + ca * (R + gap + len), cy + sa * (R + gap + len));
          ctx.stroke();
        }
        ctx.globalAlpha = 1;
        const wave = d && d.wave;
        if (wave && wave.length) {
          const n = wave.length, half = (R - gap * 2) * 0.86;
          const trace = () => {
            ctx.beginPath();
            for (let i = 0; i < n; i++) {
              const u = i / (n - 1);
              const env = Math.sin(Math.PI * u);
              const x = cx - half + u * half * 2;
              const y = cy + wave[i] * env * R * 0.42;
              i ? ctx.lineTo(x, y) : ctx.moveTo(x, y);
            }
          };
          ctx.lineJoin = 'round';
          ctx.lineWidth = lwBase * 2.6;
          ctx.globalAlpha = 0.07;
          trace();
          ctx.stroke();
          ctx.lineWidth = Math.max(1.1, lwBase * 0.7);
          ctx.globalAlpha = 0.44;
          trace();
          ctx.stroke();
          ctx.globalAlpha = 1;
        }
        beatEnv *= Math.pow(0.90, dt);
        if (d && d.beat && d !== beatSeen) {
          beatSeen = d;
          beatEnv = 1;
          for (const p of dust) p.vr += (0.5 + bass) * 0.006;
        }
        ctx.fillStyle = fg;
        for (const p of dust) {
          p.ang += p.drift * dt;
          p.tw += p.tws * dt;
          p.vr *= Math.pow(0.90, dt);
          p.r += p.vr * dt + (p.home - p.r) * (1 - Math.pow(0.985, dt));
          const a = (0.06 + 0.30 * (0.5 + 0.5 * Math.sin(p.tw))) * (0.35 + energy * 1.3);
          if (a < 0.015) continue;
          ctx.globalAlpha = Math.min(0.5, a * 0.8);
          ctx.beginPath();
          ctx.arc(cx + Math.cos(p.ang) * p.r * md, cy + Math.sin(p.ang) * p.r * md,
                  p.size * dpr * (0.7 + energy * 0.5), 0, 7);
          ctx.fill();
        }
        ctx.globalAlpha = 1;
        if (idleA > 0.02) {
          const ir = R0 * (1 + 0.025 * Math.sin(now / 900));
          ctx.strokeStyle = fg;
          ctx.globalAlpha = idleA * 0.22;
          ctx.lineWidth = lwBase;
          ctx.beginPath();
          ctx.arc(cx, cy, ir, 0, 7);
          ctx.stroke();
          ctx.globalAlpha = idleA * 0.35;
          const a0 = now / 1600;
          ctx.beginPath();
          ctx.arc(cx, cy, ir, a0, a0 + 1.1);
          ctx.stroke();
          ctx.globalAlpha = 1;
        }
      };
      requestAnimationFrame(frame);
    }
  }
"#,
);
