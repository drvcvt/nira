//! Top-level views.
//!
//! Each page owns its own subscriptions (via `hooks::*`) and renders into the
//! shell's `<main class="content">` slot. Pages don't pass each other state —
//! shared data flows through the relevant `hooks` signal.

pub mod album;
pub mod artist;
pub mod discover;
pub mod home;
pub mod library;
pub mod parts;
pub mod search;
pub mod settings;
