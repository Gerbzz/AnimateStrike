//! AnimateStrike character tools over the pinned EffectCraft project model.
//!
//! This crate stores no replacement animation timeline. Part IDs, parenting,
//! transforms and keyframes belong to `effectcraft-project`.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod character;
pub mod render;
pub mod rig;
