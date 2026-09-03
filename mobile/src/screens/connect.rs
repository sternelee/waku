//! The connect screen: paste the owner's iroh ticket, dial the daemon host.
//!
//! The ticket is the full capability (endpoint address + token) shown by the
//! desktop Settings screen's P2P remote section. It persists in
//! shared_preferences so the next launch reconnects silently.

use gpui::{div, prelude::*, px, rgb, MouseButton, MouseDownEvent};
use gpui_mobile::{hide_keyboard, show_keyboard_with_type, KeyboardType};

use crate::state::{ConnectionPhase, WakuMobile};
use crate::screens::{primary_button, ACCENT};

/// The field the global IME callback currently targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldTarget {
    Ticket,
    Composer,
    /// No field is focused — used to dismiss the keyboard when tapping
    /// outside any text field.
    None,
}

/// Feed IME text into the focused field's buffer. Called from `render` via
/// the drained pending buffer so it can mutate the entity.
pub fn dispatch_field_input(this: &mut WakuMobile, text: &str) {
    match this.active_field {
        FieldTarget::Ticket => {
            for ch in text.chars() {
                match ch {
                    '\x08' => {
                        this.ticket_input.pop();
                    }
                    '\n' | '\r' => {}
                    _ => this.ticket_input.push(ch),
                }
            }
        }
        FieldTarget::Composer => {
            for ch in text.chars() {
                match ch {
                    '\x08' => {
                        this.compose_input.pop();
                    }
                    '\n' | '\r' => {
                        // Enter submits; render drains the flag below.
                        this.compose_input.push('\n');
                    }
                    _ => this.compose_input.push(ch),
                }
            }
        }
        // No focused field — drop the text (keyboard should be hidden).
        FieldTarget::None => {}
    }
}

pub fn render_connect_screen(
    this: &mut WakuMobile,
    cx: &mut gpui::Context<WakuMobile>,
) -> gpui::AnyElement {
    let theme = this.theme();
    let connecting = this.phase == ConnectionPhase::Connecting;
    let connected = this.phase == ConnectionPhase::Connected;
    let ticket = this.ticket_input.clone();
    let error = this.error.clone();
    let focused = this.active_field == FieldTarget::Ticket;

    let button_label = if connecting {
        "连接中…"
    } else if connected {
        "已连接"
    } else {
        "连接远程主机"
    };
    let disabled = connecting || connected || ticket.trim().is_empty();

    div()
        .id("connect-root")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .child(
            div()
                .id("connect-content")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_6()
                .pt_7()
                .gap_5()
                .child(
                    div()
                        .text_3xl()
                        .font_weight(gpui::FontWeight::BOLD)
                        .child("Waku 远程"),
                )
                .child(
                    div()
                        .text_base()
                        .line_height(px(21.0))
                        .text_color(rgb(theme.on_surface_variant))
                        .child(
                            "粘贴桌面端「设置 → P2P 远程」生成的票据，通过 P2P 直连你的 Waku 主机，查看并继续已共享的 agent 会话。",
                        ),
                )
                // Ticket field
                .child(
                    div()
                        .id("ticket-field")
                        .h(px(104.0))
                        .w_full()
                        .p_3()
                        .rounded_lg()
                        .bg(rgb(0x1E1F25))
                        .border_1()
                        .border_color(rgb(if focused { ACCENT } else { 0x2E3038 }))
                        .text_sm()
                        .text_color(rgb(theme.on_surface))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                                this.active_field = FieldTarget::Ticket;
                                show_keyboard_with_type(KeyboardType::Default);
                                cx.notify();
                            }),
                        )
                        // Tapping outside the field dismisses the keyboard
                        // so it doesn't cover the connect button below.
                        .on_mouse_down_out(
                            cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                                if this.active_field != FieldTarget::None {
                                    this.active_field = FieldTarget::None;
                                    hide_keyboard();
                                    cx.notify();
                                }
                            }),
                        )
                        .child(if ticket.is_empty() {
                            div()
                                .text_color(rgb(0x6B7280))
                                .child("waku://… 粘贴票据")
                        } else {
                            div().child(truncate_ticket(&ticket))
                        }),
                )
                // Paste / clear buttons — read or reset the clipboard-driven
                // ticket directly. Paste is reliable on NativeActivity where
                // IME commitText (soft-keyboard paste) delivery is
                // vendor-dependent and often dropped.
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_3()
                        .w_full()
                        .child(
                            div()
                                .id("paste-ticket")
                                .flex()
                                .flex_1()
                                .items_center()
                                .justify_center()
                                .h(px(40.0))
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(0x2E3038))
                                .text_sm()
                                .text_color(rgb(theme.on_surface_variant))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                                        let text = gpui_mobile::get_clipboard_text();
                                        let trimmed = text.trim();
                                        if !trimmed.is_empty() {
                                            this.ticket_input = trimmed.to_owned();
                                            this.active_field = FieldTarget::Ticket;
                                            this.error = None;
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child("📋 从剪贴板粘贴"),
                        )
                        .child(
                            div()
                                .id("clear-ticket")
                                .flex()
                                .items_center()
                                .justify_center()
                                .h(px(40.0))
                                .px_4()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(0x2E3038))
                                .text_sm()
                                .text_color(rgb(theme.on_surface_variant))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                                        this.ticket_input.clear();
                                        this.error = None;
                                        cx.notify();
                                    }),
                                )
                                .child("清空"),
                        )
                        .child(
                            div()
                                .id("scan-ticket")
                                .flex()
                                .items_center()
                                .justify_center()
                                .h(px(40.0))
                                .px_4()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(0x2E3038))
                                .text_sm()
                                .text_color(rgb(theme.on_surface_variant))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                                        // Scanned text is delivered through the
                                        // IME text path, which fills the
                                        // active field — aim it at the ticket.
                                        this.active_field = FieldTarget::Ticket;
                                        this.error = None;
                                        gpui_mobile::scan_qr_code();
                                        cx.notify();
                                    }),
                                )
                                .child("📷 扫码"),
                        ),
                )
                .child({
                    primary_button(button_label, disabled).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                            if this.phase == ConnectionPhase::Connecting
                                || this.phase == ConnectionPhase::Connected
                            {
                                return;
                            }
                            let ticket = this.ticket_input.trim().to_owned();
                            if ticket.is_empty() {
                                return;
                            }
                            this.active_field = FieldTarget::Ticket;
                            hide_keyboard();
                            this.connect(ticket, true, cx);
                        }),
                    )
                })
                .children(error.map(|message| {
                    crate::screens::render_error_strip(&message, cx, |this, _cx| {
                        this.error = None;
                    })
                }))
                .children(if connected {
                    Some(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .text_sm()
                            .text_color(rgb(0x46A758))
                            .child("●")
                            .child(format!(
                                "已连接 {}",
                                this.daemon_label
                                    .clone()
                                    .unwrap_or_else(|| "远程主机".into())
                            )),
                    )
                } else if connecting {
                    Some(
                        div()
                            .text_sm()
                            .text_color(rgb(theme.on_surface_variant))
                            .child("正在建立 P2P 连接…"),
                    )
                } else {
                    None
                }),
        )
        .into_any_element()
}

fn truncate_ticket(ticket: &str) -> String {
    // Tickets are long base64 blobs; show head and tail so the owner can
    // recognize the label portion without wrapping hundreds of characters.
    let trimmed = ticket.trim();
    if trimmed.len() <= 88 {
        return trimmed.to_owned();
    }
    let head: String = trimmed.chars().take(48).collect();
    let tail: String = trimmed.chars().skip(trimmed.len() - 24).collect();
    format!("{head}…{tail}")
}
