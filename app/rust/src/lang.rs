//! UI language for the few strings Rust shows (errors, message previews); Dart sets it.

use std::sync::atomic::{AtomicBool, Ordering};

static ENGLISH: AtomicBool = AtomicBool::new(false);

pub fn set_english(on: bool) {
    ENGLISH.store(on, Ordering::Relaxed);
}

/// The string for the current language.
pub fn tr(ru: &'static str, en: &'static str) -> &'static str {
    if ENGLISH.load(Ordering::Relaxed) {
        en
    } else {
        ru
    }
}
