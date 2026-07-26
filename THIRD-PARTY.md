# Third-party assets

nira itself is Apache-2.0 (see [LICENSE](LICENSE)). The binary assets bundled
in this repository are not ours, and several of their licences require the
notices below to travel with the files. Nothing here is modified from what
upstream ships, apart from the preset selection noted at the end.

## Icons

**Font Awesome Free 6.5.1** — © 2023 Fonticons, Inc.
<https://fontawesome.com> · <https://fontawesome.com/license/free>

Icons are CC BY 4.0, the icon fonts are SIL OFL 1.1, the accompanying CSS is
MIT. Files: `nira/assets/fontawesome/fontawesome.css`,
`nira/assets/fonts/fa-{brands-400,regular-400,solid-900}.woff2`.

## Typefaces

**Geist / Geist Mono** — © Vercel, Inc. — SIL Open Font License 1.1
<https://vercel.com/font>
Files: `nira/assets/fonts/Geist{,Mono}-{,Italic-}Variable.ttf`.

**Charter** — © Bitstream, Inc., designed by Matthew Carter.
Bitstream contributed Charter under terms permitting redistribution provided
the copyright notice travels with it and the fonts are not sold on their own.
Files: `nira/assets/fonts/Charter-{Regular,Bold}.ttf`.

## Visualiser

**Butterchurn** — MIT — © Jordan Berg
<https://github.com/jberg/butterchurn>
A WebGL implementation of Ryan Geiss's MilkDrop. Files:
`components/assets/butterchurn.min.js`,
`components/assets/butterchurn-extra-images.min.js`.

**Butterchurn presets** — `components/assets/butterchurn-presets-base.min.js`,
the upstream `butterchurn-presets` bundle. The individual MilkDrop presets are
the work of their respective authors and carry their own terms; the bundle is
shipped unmodified. `components/assets/curated-presets.json` is ours and only
selects a subset by name at runtime — it contains no preset data.

---

The bundled files are minified and carry no embedded version string, so the
Butterchurn version above is the one recorded when it was vendored rather than
something derived from the file. If you are redistributing nira, confirm these
against upstream before relying on them.
