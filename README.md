# AnimateStrike

**by PWDS**

**Bring a small character to life.**

AnimateStrike combines a bounded 2D rig with painted cels and pose keys. Scrub through frames, adjust a pose, paint or erase, keep immutable project revisions, and export a PNG sequence.

### Highlights

- Paint cels and set position, rotation, or scale keys.
- Scrub a short animation with in-memory undo and redo.
- Save editable revisions and export PNG frames.

## Current status

Source-only standalone candidate. The original checkout passed focused tests and a Windows loopback workflow, but this smaller published workspace has only resolved metadata and has not been compiled or tested. Its current three-part sample is not the planned release template; no installer is supplied.

## Animate a short scene

Work with the current three-part character, paint or erase held cels, and add pose keys as you scrub through frames. Save a new `.animatestrike` revision to retain the editable state, or export a PNG sequence for review.

## Build after the shared build slot is available

From this repository root with Rust 1.95 or newer:

```powershell
cargo test --locked -p strike-character --lib
cargo test --locked -p animatestrike --bin animatestrike
cargo build --locked -p animatestrike
```

The reduced workspace and lockfile retain the tested versions in AnimateStrike's resolved dependency graph. Its selected EffectCraft crates, donor licenses, notices, attributed synthetic fixtures, and the seven embedded OFL fonts are included. The current graph has no FilmCraft dependency. See [SOURCE_SCOPE.md](SOURCE_SCOPE.md) for provenance and package changes and [CONTROL.md](apps/animatestrike/CONTROL.md) for opt-in loopback control.

## Limits

The starter character is three parts, not the planned six-part release template. Exports are PNG sequences; GIF timing and palette export are not implemented. Unsaved edits have in-memory undo/redo but no persistent autosave/recovery. Saving a new immutable revision requires same-directory hard-link support. Native pointer, keyboard-only, screen reader, high-DPI, broad import, and other-platform acceptance remain open.

The new AnimateStrike app and `strike-character` crate have no selected first-party license grant. The selected EffectCraft donor code keeps its original MIT/Apache license texts and asset notices. This candidate is not a redistributable binary package.
