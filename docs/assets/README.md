# README assets

This directory contains project-local visuals used by the root README.

| Asset | Source | Notes |
| --- | --- | --- |
| `vigil-hero.png` | Generated with Codex built-in image generation, then copied into this repo. | Raster README hero. No embedded text, logos, or remote dependency. |
| `vigil-mark.svg` | Hand-authored SVG. | Scalable project mark for README and future docs. |
| `vigil-status.svg` | Hand-authored SVG. | Deterministic status preview; labels stay crisp on GitHub. |
| `vigil-architecture.svg` | Hand-authored SVG. | High-level architecture overview for README/docs. |

Hero generation prompt:

```text
Use case: stylized-concept
Asset type: README hero background for a developer tool named Vigil
Primary request: Create a premium high-resolution visual for a macOS/Rust CLI utility that keeps AI coding agents running while the laptop can lock and the display can sleep.
Scene/backdrop: a dark, precise desktop workstation at night with a MacBook-like silhouette, a softly sleeping display, a terminal window represented only by abstract command-line rows, and a subtle shield/power-hold motif.
Subject: calm autonomous coding session continuing safely while the machine appears locked and asleep.
Style/medium: polished 3D editorial render, clean modern developer-tool aesthetic, high definition, crisp geometry, subtle depth, no cartoon style.
Composition/framing: wide 16:9 hero composition with generous clean negative space near the upper-left and center-left for README overlay text added separately; important visual detail concentrated on the right and lower-right.
Lighting/mood: controlled low-key studio lighting, cool graphite shadows, small accents of cyan and warm amber, calm and reliable.
Color palette: graphite, near-black, silver, muted cyan, small amber highlights; avoid a one-note blue/purple palette.
Materials/textures: anodized aluminum, matte glass, soft terminal glow, crisp edges.
Text (verbatim): no text.
Constraints: no logos, no brand marks, no readable UI text, no watermark, no distorted typography, no people, no animals. Must feel like a serious open-source systems utility, not a marketing splash screen.
```
