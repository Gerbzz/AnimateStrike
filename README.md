<p align="center">
  <img src="assets/branding/AnimateStrike.png" alt="Approved AnimateStrike logo" width="160" height="160">
</p>

<h1 align="center">AnimateStrike</h1>

<p align="center">
  <a href="https://prowebdevsolutions.com"><picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/branding/PWDS-logo-orange.svg">
    <img src="assets/branding/PWDS-logo.png" alt="PWDS company logo" width="120">
  </picture></a>
</p>

<p align="center"><strong>Bring a small character to life.</strong></p>

AnimateStrike joins painted cels to a parented 2D rig. Set poses across a timeline, revise a short scene, keep editable project revisions, and export PNG frames.

## Rust direction and migration status

- **Target:** A Rust-native 2D character-animation editor, extending the existing focused editor and selected EffectCraft components.
- **Current implementation:** The [Cargo workspace](Cargo.toml) and [AnimateStrike entry point](apps/animatestrike/src/main.rs) are already Rust, combining the first-party character/rig workflow with selected Rust donor crates rather than the full EffectCraft application.
- **Remaining migration:** Continue export, recovery and native-interaction work. Windows CI has now built and tested the selected standalone editor, but that does not establish interactive runtime, broad format or release acceptance.

![AnimateStrike baseline native editor showing a procedural character, parts, brush, pose controls, and timeline](docs/images/animatestrike-baseline-native.png)

*Native baseline preview from 12 October 2026, showing the original editor and its procedural three-part starter character. This remains a historical source-checkpoint image, not a new capture from the standalone build verified below. See [screenshot provenance](docs/SCREENSHOT_PROVENANCE.md).*

<p align="center"><a href="#build-from-source">Build from source</a> · <a href="SOURCE_SCOPE.md">Source and test boundary</a> · <a href="apps/animatestrike/CONTROL.md">Control protocol</a> · <a href="assets/branding">Original artwork</a></p>

## Paint, pose, and export

### Work across cels and a rig

The current editor starts with a procedural **three-part, 128 × 128, 24-frame** character. Paint and erase on held cels, choose a rig part, then key its position, rotation, or scale at a frame. Scrub the timeline to inspect the resulting motion.

### Keep an editable revision

The app offers eight in-memory undo/redo steps. Saving creates a new immutable `.animatestrike` revision; it does not preserve an undo journal or autosave unsaved edits. A fresh revision path and same-directory hard-link support are required.

### Export the frames

Export writes a sequence of PNG frames, not an encoded movie. GIF export, timing/palette handling, a six-part release template, and persistent unsaved-work recovery remain open.

## Current status

This is a **source-only standalone candidate**, with no installer. [Windows CI run 40127553920](https://github.com/Gerbzz/AnimateStrike/actions/runs/40127553920) passed `cargo test --locked -p animatestrike --bin animatestrike` and `cargo build --locked -p animatestrike` for source `7850b3afb7c38e3db912d691c6facaf9df816947`, subsequently merged without source changes. This verifies the selected editor's tests and build, not an interactive runtime session or every workspace package. Native pointer, keyboard-only, screen-reader, high-DPI, broad import, and other-platform acceptance remain open.

## Build from source

From this repository root with Rust 1.95 or newer, the intended checks are:

```powershell
cargo test --locked -p strike-character --lib
cargo test --locked -p animatestrike --bin animatestrike
cargo build --locked -p animatestrike
```

The linked Windows run executed the second and third commands against the reduced workspace. It does not establish a separate run of the `strike-character --lib` command above. The selected EffectCraft crates, donor licenses and notices, synthetic fixtures, and seven embedded OFL fonts are included. No FilmCraft dependency is in this candidate's selected graph. [SOURCE_SCOPE.md](SOURCE_SCOPE.md) records exact provenance and the original test evidence.

The new AnimateStrike app and `strike-character` crate have no selected first-party license grant. The selected EffectCraft donor code keeps its original MIT/Apache texts and asset notices; this is not a redistributable binary package. Explore the other [PWDS Strike repositories](https://github.com/Gerbzz?tab=repositories&q=Strike).

## PWDS brand artwork

The company mark in this masthead is the existing [official PWDS logo](https://github.com/Gerbzz/PWDS-Website/blob/main/public/prowebdevsolutions-logo.png), with the [official orange display asset](https://prowebdevsolutions.com/images/prowebdevsolutions-logo-orange.svg) for dark backgrounds. Any app logo above it remains the original Strike suite artwork.
