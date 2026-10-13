# AnimateStrike

AnimateStrike is a Windows-first Rust character animation editor prototype. It edits a bounded 2D parented rig, held RGBA cels, and position/rotation/scale keys over the selected EffectCraft project and CPU renderer. The current native editor starts with a procedural three-part, 128 × 128, 24-frame character. It supports paint/erase, frame scrubbing, pose keys, eight in-memory undo/redo steps, immutable `.animatestrike` revisions, and PNG sequence export.

This is a **source-only standalone candidate** prepared from a tested EffectCraft checkout. The original source passed 9 native app/control tests and 8 character-engine tests; its built Windows executable and production loopback workflow were independently rechecked. The smaller standalone workspace has passed metadata resolution but has **not** yet been compiled or tested. No release or installer is supplied.

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
