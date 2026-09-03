//! The sessions screen: the shared catalog from the remote daemon host.

use gpui::{div, prelude::*, px, rgb, MouseButton, MouseDownEvent};
use waku_client::model::{unix_time};

use crate::state::WakuMobile;

fn relative_time(timestamp: u64) -> String {
    let now = unix_time();
    let delta = now.saturating_sub(timestamp);
    if delta < 60 {
        "刚刚".into()
    } else if delta < 3600 {
        format!("{} 分钟前", delta / 60)
    } else if delta < 86400 {
        format!("{} 小时前", delta / 3600)
    } else {
        format!("{} 天前", delta / 86400)
    }
}

pub fn render_sessions_screen(
    this: &mut WakuMobile,
    cx: &mut gpui::Context<WakuMobile>,
) -> impl IntoElement {
    let sessions = this.sessions.clone();
    let loading = this.loading_catalog;
    let connected = this.phase == crate::state::ConnectionPhase::Connected;

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .id("sessions-root")
        .child(
            div()
                .id("sessions-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .children(if !connected {
                    vec![empty_state("尚未连接", "前往「连接」页粘贴远程主机票据")]
                } else if loading && sessions.is_empty() {
                    vec![empty_state("正在加载会话…", "")]
                } else if sessions.is_empty() {
                    vec![empty_state(
                        "没有共享会话",
                        "在桌面端右键会话，勾选「允许远程同步与控制」",
                    )]
                } else {
                    let mut rows = Vec::with_capacity(sessions.len());
                    for session in sessions.iter() {
                        rows.push(session_row(this, cx, session));
                    }
                    rows
                }),
        )
}

fn empty_state(title: &str, hint: &str) -> gpui::AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .flex_1()
        .gap_2()
        .text_color(rgb(0x6B7280))
        .child(
            div()
                .text_base()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(title.to_owned()),
        )
        .child(
            div()
                .px_6()
                .text_sm()
                .text_center()
                .child(hint.to_owned()),
        )
        .into_any_element()
}

fn session_row(
    this: &mut WakuMobile,
    cx: &mut gpui::Context<WakuMobile>,
    session: &waku_client::model::AgentSession,
) -> gpui::AnyElement {
    let theme = this.theme();
    let title = session
        .auto_title
        .as_ref()
        .filter(|title| !title.is_empty())
        .unwrap_or(&session.title)
        .clone();
    let provider = session.provider.display_name().to_owned();
    let busy = session.status.is_busy();
    let last_reply = session.last_reply_at.unwrap_or(session.updated_at);
    let time = relative_time(last_reply);
    let model = session.model.clone().unwrap_or_default();
    let session_id = session.id;

    let dot_color = if busy { 0x4C8DFF } else { 0x3A3C44 };

    div()
        .id(gpui::ElementId::Name(session.id.to_string().into()))
        .h(px(72.0))
        .px_4()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .hover(|style| style.bg(rgb(0x1E1F25)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                this.open_chat(session_id, cx);
            }),
        )
        .child(div().w_2p5().h_2p5().rounded_full().bg(rgb(dot_color)))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .gap_0p5()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex_1()
                                .text_base()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme.on_surface_variant))
                                .child(time),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .text_sm()
                        .text_color(rgb(theme.on_surface_variant))
                        .child(provider)
                        .when(!model.is_empty(), |d| d.child("·").child(model))
                        .child("·")
                        .child(if busy { "工作中" } else { "空闲" }),
                ),
        )
        .into_any_element()
}