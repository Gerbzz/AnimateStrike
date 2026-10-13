# AnimateStrike

**by PWDS**

**Bring a small character to life.**

AnimateStrike combines a small 2D character rig with painted cels and pose keys. Scrub through frames, adjust a pose, paint or erase, keep immutable project revisions, and export a PNG sequence.

## Highlights

- Paint cels and set position, rotation, or scale keys.
- Scrub a short animation with in-memory undo and redo.
- Save editable revisions and export PNG frames.

## Current status

Source-only standalone candidate. The original checkout passed focused tests and a Windows loopback workflow, but this smaller published workspace has only resolved metadata and has not been compiled or tested. Its current three-part sample is not the planned release template; no installer is supplied.

## The current character workflow

The editor starts with a procedural three-part, 128 × 128, 24-frame character. It supports paint and erase, frame scrubbing, pose keys, and eight in-memory undo/redo steps. A saved `.animatestrike` revision keeps the editable work; export produces PNG frames. Undo and redo exist only in memory; saving creates a new immutable project revision, not a persistent undo history. Export writes PNG frames, not an encoded movie; persistent recovery and GIF export remain open.

## Build from source

From this repository root with Rust 1.95 or newer, the intended source checks are below. They have not yet run against this standalone package:

```powershell
cargo test --locked -p strike-character --lib
cargo test --locked -p animatestrike --bin animatestrike
cargo build --locked -p animatestrike
```

The reduced workspace and lockfile retain the tested versions in AnimateStrike's resolved dependency graph. Its selected EffectCraft crates, donor licenses, notices, attributed synthetic fixtures, and the seven embedded OFL fonts are included. The current graph has no FilmCraft dependency. See [SOURCE_SCOPE.md](SOURCE_SCOPE.md) for provenance and package changes and [CONTROL.md](apps/animatestrike/CONTROL.md) for opt-in loopback control.

## Limits

The starter character is three parts, not the planned six-part release template. Exports are PNG sequences; GIF timing and palette export are not implemented. Unsaved edits have in-memory undo/redo but no persistent autosave/recovery. Saving a new immutable revision requires same-directory hard-link support. Native pointer, keyboard-only, screen reader, high-DPI, broad import, and other-platform acceptance remain open.

The new AnimateStrike app and `strike-character` crate have no selected first-party license grant. The selected EffectCraft donor code keeps its original MIT/Apache license texts and asset notices. This candidate is not a redistributable binary package.
