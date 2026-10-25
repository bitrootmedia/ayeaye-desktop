//! Zed's One Dark and One Light, re-expressed as a GPUI Kit theme set, and
//! following the system appearance like Zed does by default.

use std::rc::Rc;

use gpui_kit::component::{Theme, ThemeMode, ThemeSet};
use gpui_kit::{App, Window};

const ONE: &str = include_str!("../assets/themes/one.json");

pub fn init(cx: &mut App) {
    let set: ThemeSet = serde_json::from_str(ONE).expect("bundled theme parses");
    let mut dark = None;
    let mut light = None;
    for theme in set.themes {
        if theme.mode.is_dark() {
            dark = Some(Rc::new(theme));
        } else {
            light = Some(Rc::new(theme));
        }
    }
    Theme::update(cx, |theme| {
        if let Some(dark) = dark {
            theme.dark_theme = dark;
        }
        if let Some(light) = light {
            theme.light_theme = light;
        }
    });
    Theme::change(
        forced_mode().unwrap_or_else(|| cx.window_appearance().into()),
        None,
        cx,
    );
}

/// `AYEAYE_THEME=dark` or `light` pins the theme instead of following the OS.
fn forced_mode() -> Option<ThemeMode> {
    match std::env::var("AYEAYE_THEME").ok()?.as_str() {
        "dark" => Some(ThemeMode::Dark),
        "light" => Some(ThemeMode::Light),
        _ => None,
    }
}

/// Called when the OS flips between light and dark.
pub fn follow_system(window: &mut Window, cx: &mut App) {
    if forced_mode().is_none() {
        Theme::sync_system_appearance(Some(window), cx);
    }
}
