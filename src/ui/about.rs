//! About Ayeaye: which build this is, so "is my change in?" has an answer.

use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::*;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_ID: &str = env!("AYEAYE_BUILD_ID");
pub const BUILT_AT: &str = env!("AYEAYE_BUILT_AT");

pub fn open(window: &mut Window, cx: &mut App) {
    window.open_alert_dialog(cx, |alert, _, cx| {
        let muted = cx.theme().muted_foreground;
        let mono = cx.theme().mono_font_family.clone();
        alert
            .title("Ayeaye")
            .description(
                v_flex()
                    .gap_1()
                    .child(format!("Version {VERSION}"))
                    .child(
                        div()
                            .text_color(muted)
                            .child(format!("Build {BUILD_ID} · {BUILT_AT}")),
                    )
                    .child(
                        div().text_xs().font_family(mono).text_color(muted).child(
                            std::env::current_exe()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default(),
                        ),
                    ),
            )
            .ok_text("OK")
    });
}
