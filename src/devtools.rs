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
//! `scroll <x> <y> <dy>` (positive scrolls down),
//! `quit`.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::*;

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
