//! The chat screen: transcript bubbles, live streaming, permission prompts,
//! and the composer.
//!
//! Layout is keyboard-aware: the composer sits above the software keyboard
//! (`keyboard_height`), the transcript scrolls behind it, and everything
//! respects the top safe area (rendered in the root chrome).

use gpui::{div, prelude::*, px, rgb, MouseButton, MouseDownEvent};
use gpui_mobile::{hide_keyboard, show_keyboard_with_type, KeyboardType};
use waku_client::model::{Message, MessageRole, SessionStatus};

use crate::screens::connect::FieldTarget;
use crate::screens::{ACCENT, DANGER};
use crate::state::WakuMobile;

const USER_BUBBLE: u32 = 0x1D3F6E;
const USER_BUBBLE_TEXT: u32 = 0xE8F1FF;
const ASSISTANT_BUBBLE: u32 = 0x26272D;
const ASSISTANT_BUBBLE_TEXT: u32 = 0xE4E4E9;
const SYSTEM_TEXT: u32 = 0x8A93A5;

pub fn render_chat_screen(
    this: &mut WakuMobile,
    cx: &mut gpui::Context<WakuMobile>,
) -> gpui::AnyElement {
    let kb_height = gpui_mobile::keyboard_height();
    let kb_padding = if kb_height > 0.0 {
        kb_height + 4.0
    } else {
        this.safe_area.bottom
    };

    let Some(chat) = this.chat.as_ref() else {
        // Chat screen with nothing loaded (opening in flight).
        return div()
            .id("chat-loading")
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .text_color(rgb(SYSTEM_TEXT))
            .child(if this.chat_loading { "加载中…" } else { "没有打开会话" })
            .into_any_element();
    };

    let messages = chat.session.messages.clone();
    let status = chat.session.status;
    let activity = chat.activity.clone();
    let error = chat.error.clone();
    let busy = chat.is_busy();
    let supports_steer = chat.supports_steer;
    let permission = chat.pending_permission.as_ref().map(|permission| {
        (
            permission.request_id.clone(),
            permission.title.clone(),
            permission.detail.clone(),
            permission.options.clone(),
        )
    });
    let compose = this.compose_input.clone();
    let compose_focused = this.active_field == FieldTarget::Composer;

    div()
        .flex()
        .flex_col()
        .flex_1()
        .id("chat-root")
        // ── Transcript ──────────────────────────────────────────────────
        .child(
            div()
                .id("chat-transcript")
                .flex_1()
                .overflow_y_scroll()
                .track_scroll(&this.chat_scroll)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                        if this.active_field == FieldTarget::Composer {
                            this.active_field = FieldTarget::Ticket;
                            hide_keyboard();
                            cx.notify();
                        }
                    }),
                )
                .children(messages.iter().map(|message| render_message(message)))
                .children(busy.then(|| {
                    div()
                        .px_4()
                        .py_1p5()
                        .text_sm()
                        .text_color(rgb(SYSTEM_TEXT))
                        .child(
                            activity
                                .clone()
                                .map(|title| format!("正在处理 · {title}"))
                                .unwrap_or_else(|| "正在处理…".into()),
                        )
                })),
        )
        // ── Permission card ─────────────────────────────────────────────
        .children(permission.map(|(request_id, title, detail, options)| {
            render_permission_card(this, cx, request_id, title, detail, options)
        }))
        // ── Error strip ─────────────────────────────────────────────────
        .children(error.map(|message| {
            crate::screens::render_error_strip(&message, cx, |this, _cx| {
                if let Some(chat) = this.chat.as_mut() {
                    chat.error = None;
                }
            })
        }))
        // ── Composer ────────────────────────────────────────────────────
        .child(render_composer(
            this,
            cx,
            &compose,
            compose_focused,
            busy,
            supports_steer,
            status,
        ))
        .child(div().w_full().h(px(kb_padding)))
        .into_any_element()
}

fn render_message(message: &Message) -> impl IntoElement {
    match message.role {
        MessageRole::User => {
            let text = message.display_content.as_deref().unwrap_or(&message.content);
            div()
                .w_full()
                .flex()
                .justify_end()
                .px_3()
                .py_1()
                .child(
                    div()
                        .max_w(px(320.0))
                        .px_3p5()
                        .py_2p5()
                        .rounded_2xl()
                        .bg(rgb(USER_BUBBLE))
                        .text_color(rgb(USER_BUBBLE_TEXT))
                        .text_base()
                        .line_height(px(20.0))
                        .child(render_text(&text)),
                )
        }
        MessageRole::Assistant => {
            let streaming = message.streaming;
            let text = if message.content.is_empty() && streaming {
                "…"
            } else {
                &message.content
            };
            div()
                .w_full()
                .flex()
                .justify_start()
                .px_3()
                .py_1()
                .child(
                    div()
                        .max_w(px(340.0))
                        .px_3p5()
                        .py_2p5()
                        .rounded_2xl()
                        .bg(rgb(ASSISTANT_BUBBLE))
                        .text_color(rgb(ASSISTANT_BUBBLE_TEXT))
                        .text_base()
                        .line_height(px(20.0))
                        .child(render_text(text))
                        .when(streaming, |d| d.child(render_caret())),
                )
        }
        MessageRole::System => div()
            .w_full()
            .px_6()
            .py_1p5()
            .text_xs()
            .text_color(rgb(SYSTEM_TEXT))
            .text_center()
            .child(message.content.clone()),
    }
}

/// Minimal markdown-ish rendering: bold and inline code only, so prose stays
/// readable without pulling a full markdown stack into the mobile binary.
fn render_text(text: &str) -> impl IntoElement {
    let mut children = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("**") {
        let before = &rest[..open];
        if !before.is_empty() {
            children.push(div().child(before.to_owned()).into_any_element());
        }
        let after = &rest[open + 2..];
        let Some(close) = after.find("**") else {
            children.push(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(rest[open..].to_owned())
                    .into_any_element(),
            );
            rest = "";
            break;
        };
        children.push(
            div()
                .font_weight(gpui::FontWeight::BOLD)
                .child(after[..close].to_owned())
                .into_any_element(),
        );
        rest = &after[close + 2..];
    }
    if !rest.is_empty() {
        children.push(div().child(rest.to_owned()).into_any_element());
    }
    div().flex().flex_wrap().children(children)
}

fn render_caret() -> impl IntoElement {
    div().text_color(rgb(ACCENT)).child("▍")
}

fn render_permission_card(
    this: &mut WakuMobile,
    cx: &mut gpui::Context<WakuMobile>,
    request_id: String,
    title: String,
    detail: String,
    options: Vec<waku_client::model::PermissionOption>,
) -> impl IntoElement {
    let theme = this.theme();
    let mut card = div()
        .flex()
        .flex_col()
        .gap_2()
        .mx_3()
        .mb_2()
        .p_3p5()
        .rounded_xl()
        .bg(rgb(theme.primary_container))
        .text_color(rgb(theme.on_primary_container))
        .child(
            div()
                .text_base()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(
            div()
                .text_sm()
                .text_color(rgb(theme.on_primary_container))
                .child(detail),
        );

    for option in options {
        let option_id = option.id.clone();
        let label = option.label.clone();
        let allow = option.allow;
        let request_id = request_id.clone();
        card = card.child(
            div()
                .id("permission-option")
                .h_11()
                .mt_1()
                .rounded_lg()
                .bg(rgb(if allow { 0x1D6A41 } else { 0x3A3A3C }))
                .text_color(rgb(0xFFFFFF))
                .text_base()
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                        this.respond_permission(request_id.clone(), option_id.clone(), cx);
                    }),
                )
                .child(label),
        );
    }
    card
}

fn render_composer(
    this: &mut WakuMobile,
    cx: &mut gpui::Context<WakuMobile>,
    compose: &str,
    focused: bool,
    busy: bool,
    supports_steer: bool,
    _status: SessionStatus,
) -> impl IntoElement {
    let theme = this.theme();
    let can_send = !compose.trim().is_empty();

    let send_label = if busy {
        if supports_steer {
            "发送" // steer
        } else {
            "停止" // cancel
        }
    } else {
        "发送"
    };

    div()
        .flex()
        .flex_row()
        .items_end()
        .gap_2()
        .px_3()
        .py_2p5()
        .bg(rgb(0x17181C))
        .border_t_1()
        .border_color(rgb(0x26272D))
        .child(
            div()
                .id("composer-field")
                .flex_1()
                .min_h(px(44.0))
                .px_3p5()
                .py_2p5()
                .rounded_full()
                .bg(rgb(0x26272D))
                .border_1()
                .border_color(rgb(if focused { ACCENT } else { 0x33343C }))
                .text_base()
                .text_color(rgb(theme.on_surface))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                        this.active_field = FieldTarget::Composer;
                        show_keyboard_with_type(KeyboardType::Default);
                        cx.notify();
                    }),
                )
                .child(if compose.is_empty() {
                    div()
                        .text_color(rgb(0x6B7280))
                        .child("发送消息给 agent…")
                } else {
                    div().child(compose.to_owned())
                }),
        )
        .child(
            div()
                .id("composer-send")
                .h_11()
                .min_w_16()
                .rounded_full()
                .bg(rgb(if busy && !supports_steer { DANGER } else { ACCENT }))
                .text_color(rgb(0xFFFFFF))
                .text_base()
                .font_weight(gpui::FontWeight::MEDIUM)
                .flex()
                .items_center()
                .justify_center()
                .when(!can_send && !busy, |d| d.opacity(0.4))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                        if busy {
                            if supports_steer && can_send {
                                // Steer the running turn.
                                this.steer_composer(cx);
                            } else {
                                this.cancel_turn(cx);
                            }
                        } else if can_send {
                            this.send_composer(cx);
                        }
                    }),
                )
                .child(send_label),
        )
}
