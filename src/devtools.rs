//! A scripted driver for looking at the app without a person at the keyboard
//! — and without the screen-recording permission a screenshot tool needs:
//! frames are rendered straight to PNG from the window's own scene.
//!
//! Built only with `--features devtools`. The script is `;`-separated steps:
//!
//! ```sh
//! AYEAYE_SCRIPT='wait 1500; shot start; keys cmd-n; type Hello; wait 900; shot typed' \
//! AYEAYE_SHOTS=/tmp/shots cargo run --features devtools
//! ```
//!
//! Steps: `wait <ms>`, `shot <name>`, `keys <keystroke> …`, `type <text>`,
//! `click <x> <y>` / `dclick <x> <y>` / `rclick <x> <y>` in window points,
//! `scroll <x> <y> <dy>` (positive scrolls down), `about` (the menu item,
//! dispatched the way the menu bar does it), `drop <x> <y> <path>…` (files
//! dragged in from Finder and let go there),
//! `quit`.
//!
//! `AYEAYE_RICH_TEXT=<file>` opens no workspace: the window shows that file
//! as an article body, laid out as the reader lays it out — for comparing
//! the rendering with the web app's without signing in.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::*;

pub struct RichTextPreview {
    body: String,
}

impl RichTextPreview {
    pub fn new(path: PathBuf) -> Self {
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| format!("could not read {}: {e}", path.display()));
        Self { body }
    }
}

impl Render for RichTextPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("preview")
            .size_full()
            .overflow_y_scroll()
            .bg(cx.theme().background)
            .px_6()
            .py_4()
            .child(
                v_flex()
                    .max_w(px(780.))
                    .gap_4()
                    .pb_8()
                    .child(crate::kb::article::read_title(
                        "Hindsight — setting up Claude Code",
                    ))
                    .child(crate::ui::widgets::rich_text("preview-body", &self.body)),
            )
    }
}

pub fn run_script(window: AnyWindowHandle, cx: &mut App) {
    let Ok(script) = std::env::var("AYEAYE_SCRIPT") else {
        return;
    };
    let dir = PathBuf::from(std::env::var("AYEAYE_SHOTS").unwrap_or_else(|_| ".".into()));
    let _ = std::fs::create_dir_all(&dir);
    let steps: Vec<String> = script
        .split(';')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();

    cx.spawn(async move |cx| {
        for step in steps {
            let (verb, rest) = step.split_once(' ').unwrap_or((step.as_str(), ""));
            match verb {
                "wait" => {
                    let ms = rest.trim().parse().unwrap_or(500);
                    cx.background_executor()
                        .timer(Duration::from_millis(ms))
                        .await;
                    continue;
                }
                "quit" => {
                    cx.update(|cx| cx.quit());
                    return;
                }
                _ => {}
            }
            let dir = dir.clone();
            let rest = rest.to_owned();
            let verb = verb.to_owned();
            let result = window.update(cx, |_, window, cx| {
                // A hidden window gets no display-link frames: draw one now,
                // so hit-testing and the shot both see the current state.
                window.draw(cx).clear(cx);
                match verb.as_str() {
                    "shot" => match window.render_to_image() {
                        Ok(image) => {
                            let path = dir.join(format!("{}.png", rest.trim()));
                            if let Err(e) = image.save(&path) {
                                eprintln!("shot {}: {e}", path.display());
                            } else {
                                eprintln!("shot {}", path.display());
                            }
                        }
                        Err(e) => eprintln!("shot failed: {e}"),
                    },
                    "keys" => {
                        for key in rest.split_whitespace() {
                            match Keystroke::parse(key) {
                                Ok(k) => {
                                    window.dispatch_keystroke(k, cx);
                                }
                                Err(e) => eprintln!("bad keystroke {key}: {e}"),
                            }
                        }
                    }
                    "type" => {
                        for ch in rest.chars() {
                            let key = match ch {
                                ' ' => "space".to_owned(),
                                '\n' => "enter".to_owned(),
                                c => c.to_string(),
                            };
                            if let Ok(mut k) = Keystroke::parse(&key) {
                                k.key_char = Some(ch.to_string());
                                window.dispatch_keystroke(k, cx);
                            }
                        }
                    }
                    "click" | "dclick" | "rclick" => {
                        let mut parts = rest
                            .split_whitespace()
                            .filter_map(|p| p.parse::<f32>().ok());
                        let (Some(x), Some(y)) = (parts.next(), parts.next()) else {
                            return;
                        };
                        let position = point(px(x), px(y));
                        let button = if verb == "rclick" {
                            MouseButton::Right
                        } else {
                            MouseButton::Left
                        };
                        let clicks = if verb == "dclick" { 2 } else { 1 };
                        window.dispatch_event(
                            PlatformInput::MouseMove(MouseMoveEvent {
                                position,
                                pressed_button: None,
                                modifiers: Modifiers::default(),
                            }),
                            cx,
                        );
                        for click_count in 1..=clicks {
                            window.dispatch_event(
                                PlatformInput::MouseDown(MouseDownEvent {
                                    button,
                                    position,
                                    modifiers: Modifiers::default(),
                                    click_count,
                                    first_mouse: false,
                                }),
                                cx,
                            );
                            window.dispatch_event(
                                PlatformInput::MouseUp(MouseUpEvent {
                                    button,
                                    position,
                                    modifiers: Modifiers::default(),
                                    click_count,
                                }),
                                cx,
                            );
                        }
                    }
                    "scroll" => {
                        let mut parts = rest
                            .split_whitespace()
                            .filter_map(|p| p.parse::<f32>().ok());
                        let (Some(x), Some(y), Some(dy)) =
                            (parts.next(), parts.next(), parts.next())
                        else {
                            return;
                        };
                        window.dispatch_event(
                            PlatformInput::ScrollWheel(ScrollWheelEvent {
                                position: point(px(x), px(y)),
                                delta: ScrollDelta::Pixels(point(px(0.), px(-dy))),
                                modifiers: Modifiers::default(),
                                touch_phase: TouchPhase::Moved,
                            }),
                            cx,
                        );
                    }
                    "about" => window.dispatch_action(Box::new(crate::actions::About), cx),
                    "drop" => {
                        let mut parts = rest.split_whitespace();
                        let (Some(x), Some(y)) = (
                            parts.next().and_then(|p| p.parse::<f32>().ok()),
                            parts.next().and_then(|p| p.parse::<f32>().ok()),
                        ) else {
                            return;
                        };
                        let position = point(px(x), px(y));
                        let paths = ExternalPaths(parts.map(PathBuf::from).collect());
                        for event in [
                            FileDropEvent::Entered { position, paths },
                            FileDropEvent::Pending { position },
                            FileDropEvent::Submit { position },
                        ] {
                            window.dispatch_event(PlatformInput::FileDrop(event), cx);
                        }
                    }
                    other => eprintln!("unknown step: {other}"),
                }
            });
            if result.is_err() {
                return;
            }
            // Let the frame settle between steps.
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;
        }
    })
    .detach();
}
