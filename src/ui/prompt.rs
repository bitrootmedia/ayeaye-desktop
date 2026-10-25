//! A one-line prompt floating over the workspace: "New task", "Spark",
//! "New book". Return submits, Escape dismisses — the menu bar app's box.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::*;

use crate::actions::Cancel;

pub enum PromptEvent {
    Submit(String),
    Dismissed,
}

impl EventEmitter<PromptEvent> for PromptView {}

pub struct PromptView {
    label: SharedString,
    hint: SharedString,
    icon: Icon,
    input: Entity<InputState>,
    _subscription: Subscription,
}

impl PromptView {
    pub fn new(
        label: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
        hint: impl Into<SharedString>,
        icon: impl Into<Icon>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = placeholder.into();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        input.update(cx, |i, cx| i.focus(window, cx));
        let subscription = cx.subscribe_in(&input, window, |_, input, e: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = e {
                let value = input.read(cx).value().trim().to_owned();
                if !value.is_empty() {
                    cx.emit(PromptEvent::Submit(value));
                }
            }
        });
        Self {
            label: label.into(),
            hint: hint.into(),
            icon: icon.into(),
            input,
            _subscription: subscription,
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(PromptEvent::Dismissed);
    }
}

impl Render for PromptView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .key_context("QuickOpen")
            .on_action(cx.listener(Self::cancel))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(PromptEvent::Dismissed)))
            .w(px(560.))
            .p_1()
            .gap_1()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.input)
            .bg(theme.popover)
            .shadow_lg()
            .child(
                h_flex()
                    .px_2()
                    .pt_1()
                    .gap_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(self.icon.clone())
                    .child(self.label.clone()),
            )
            .child(Input::new(&self.input).appearance(false))
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(self.hint.clone()),
            )
    }
}
