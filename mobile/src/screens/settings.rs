//! The settings screen: connection status, disconnect, about.

use gpui::{div, prelude::*, px, rgb, MouseButton, MouseDownEvent};

use crate::state::{ConnectionPhase, WakuMobile};

pub fn render_settings_screen(
    this: &mut WakuMobile,
    cx: &mut gpui::Context<WakuMobile>,
) -> impl IntoElement {
    let theme = this.theme();
    let phase = this.phase;
    let label = this.daemon_label.clone();
    let connected = phase == ConnectionPhase::Connected;

    div()
        .id("settings-scroll")
        .flex()
        .flex_col()
        .flex_1()
        .overflow_y_scroll()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_4()
                .px_4()
                .py_4()
                // ── Connection card ──────────────────────────────────────
                .child(
                    div()
                        .rounded_xl()
                        .bg(rgb(theme.surface_container))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(theme.on_surface_variant))
                                .child("远程连接"),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_base()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(if connected {
                                            "已连接".to_owned()
                                        } else {
                                            "未连接".to_owned()
                                        }),
                                )
                                .child(
                                    div()
                                        .w_2()
                                        .h_2()
                                        .rounded_full()
                                        .bg(rgb(if connected { 0x46A758 } else { 0x6B7280 })),
                                ),
                        )
                        .when_some(label, |d, label| {
                            d.child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(theme.on_surface_variant))
                                    .child(format!("主机：{label}")),
                            )
                        }),
                )
                // ── Disconnect ──────────────────────────────────────────
                .children(if connected {
                    Some(
                        div()
                            .id("disconnect-button")
                            .h_12()
                            .rounded_lg()
                            .bg(rgb(0x3A1D1F))
                            .text_color(rgb(0xFF8A8A))
                            .text_base()
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                                    this.disconnect(cx);
                                }),
                            )
                            .child("断开连接并清除票据"),
                    )
                } else {
                    None
                })
                // ── About ───────────────────────────────────────────────
                .child(
                    div()
                        .rounded_xl()
                        .bg(rgb(theme.surface_container))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(theme.on_surface_variant))
                                .child("关于"),
                        )
                        .child(div().text_base().child("Waku Mobile 0.1"))
                        .child(
                            div()
                                .text_sm()
                                .line_height(px(18.0))
                                .text_color(rgb(theme.on_surface_variant))
                                .child(
                                    "通过 iroh P2P 连接你的 Waku 桌面主机，查看并继续已共享的 agent 会话。所有计算都运行在主机上。",
                                ),
                        ),
                ),
        )
}
