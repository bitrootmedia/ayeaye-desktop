// GPUI's derive macros expand to `gpui::…` paths; through the Kit alone, the
// Kit itself has to answer to that name.
extern crate gpui_kit as gpui;

mod actions;
mod api;
mod app_state;
mod assets;
#[cfg(feature = "devtools")]
mod devtools;
mod home;
mod kb;
mod notes;
mod people;
mod personal;
mod projects;
mod realtime;
mod session;
mod store;
mod tasks;
mod theme;
mod timer;
mod ui;

use gpui_kit::component::TitleBar;
use gpui_kit::*;

use app_state::AppState;
use ui::root::AppView;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("ayeaye=info,warn"))
        .init();

    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(|cx| {
            gpui_kit::init(cx);
            theme::init(cx);
            AppState::init(cx);
            actions::bind_keys(cx);
            actions::set_menus(cx);

            cx.on_action(|_: &actions::Quit, cx| cx.quit());
            cx.on_action(|_: &actions::About, cx| {
                // A menu action arrives while the window is mid-update, so
                // the dialog opens on the next turn rather than inside it.
                let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied())
                else {
                    return;
                };
                cx.defer(move |cx| {
                    window
                        .update(cx, |_, window, cx| ui::about::open(window, cx))
                        .ok();
                });
            });
            // One window, so closing it is quitting — unsaved notes are
            // flushed by the workspace's quit hook either way.
            cx.on_window_closed(|cx, _| cx.quit()).detach();

            // A scripted devtools run must not steal the keyboard from
            // whoever is at the machine.
            let scripted =
                cfg!(feature = "devtools") && std::env::var_os("AYEAYE_SCRIPT").is_some();
            let options = WindowOptions {
                focus: !scripted,
                show: !scripted,
                window_bounds: Some(WindowBounds::centered(size(px(1180.), px(780.)), cx)),
                window_min_size: Some(size(px(640.), px(420.))),
                app_id: Some("com.bitroot.ayeaye-desktop".into()),
                ..TitleBar::window_options()
            };
            #[cfg(feature = "devtools")]
            if let Some(path) = std::env::var_os("AYEAYE_RICH_TEXT") {
                let (window, _) = gpui_kit::open_window(options, cx, |_, cx| {
                    cx.new(|_| devtools::RichTextPreview::new(path.into()))
                })
                .expect("failed to open the window");
                devtools::run_script(window, cx);
                return;
            }
            let (_window, _) = gpui_kit::open_window(options, cx, |window, cx| {
                window.set_window_title("Ayeaye");
                cx.new(|cx| AppView::new(window, cx))
            })
            .expect("failed to open the window");
            #[cfg(feature = "devtools")]
            devtools::run_script(_window, cx);
            if !scripted {
                cx.activate(true);
            }
        });
}
