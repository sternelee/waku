//! Navigation, chrome, and the four mobile screens.
//!
//! Mobile conventions this app follows: single-column layouts sized to the
//! window, safe-area padding around system chrome, 44px+ touch targets, a
//! bottom navigation bar for the three tab roots, a back button on pushed
//! screens, and the composer riding above the keyboard (`keyboard_height`).
//!
//! Styling uses GPUI's Tailwind-style `Styled` methods (`px_4`, `gap_2`,
//! `rounded_lg`, `size_full`, …). Only dynamic pixel values (safe-area
//! insets, keyboard height) go through `style(...)` refinements directly.

pub mod chat;
pub mod connect;
pub mod sessions;
pub mod settings;

use gpui::{
    div, prelude::*, px, rgb, AnyElement, Context, MouseButton, MouseDownEvent, Window,
};
use gpui_mobile::components::material::{MaterialTheme, NavigationBarBuilder, TopAppBar};

use crate::state::{ConnectionPhase, WakuMobile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Connect,
    Sessions,
    Chat,
    Settings,
}

impl Screen {
    pub fn title(&self) -> &'static str {
        match self {
            Screen::Connect => "Waku",
            Screen::Sessions => "会话",
            Screen::Chat => "对话",
            Screen::Settings => "设置",
        }
    }

    pub fn is_tab_root(&self) -> bool {
        matches!(self, Screen::Connect | Screen::Sessions | Screen::Settings)
    }
}

// ── Safe area ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct SafeArea {
    pub top: f32,
    pub bottom: f32,
    pub left: f32,
    pub right: f32,
}

/// Query the platform safe-area insets. iOS reports them once the window is
/// up; Android reads them from the global platform handle.
pub fn query_safe_area() -> SafeArea {
    #[cfg(target_os = "android")]
    {
        use gpui_mobile::android::jni;
        if let Some(platform) = jni::platform()
            && let Some(window) = platform.primary_window()
        {
            let insets = window.safe_area_insets_logical();
            return SafeArea {
                top: insets.top,
                bottom: insets.bottom,
                left: insets.left,
                right: insets.right,
            };
        }
    }
    #[cfg(target_os = "ios")]
    {
        let (top, bottom, left, right) = gpui_mobile::safe_area_insets();
        if top > 0.0 || bottom > 0.0 {
            return SafeArea {
                top,
                bottom,
                left,
                right,
            };
        }
        // Fallback while the window is still coming up.
        return SafeArea {
            top: 59.0,
            bottom: 34.0,
            left: 0.0,
            right: 0.0,
        };
    }
    #[allow(unreachable_code)]
    SafeArea::default()
}

// ── Shared chrome ───────────────────────────────────────────────────────────

pub const ACCENT: u32 = 0x4C8DFF;
pub const DANGER: u32 = 0xE5484D;
const OK: u32 = 0x46A758;

impl WakuMobile {
    /// The appearance is pinned dark: Waku's desktop theme is dark-first and
    /// a single palette keeps the mobile transcript legible in v1.
    pub fn theme(&self) -> MaterialTheme {
        MaterialTheme::from_appearance(true)
    }

    pub fn chrome_style(&self) -> gpui_mobile::SystemChromeStyle {
        let theme = self.theme();
        gpui_mobile::SystemChromeStyle {
            status_bar_color: Some(theme.surface),
            status_bar_style: gpui_mobile::StatusBarContentStyle::Light,
            navigation_bar_color: Some(theme.surface),
        }
    }

    fn render_nav_bar(can_go_back: bool, screen: Screen, cx: &mut Context<Self>) -> AnyElement {
        let title = screen.title();
        let theme = MaterialTheme::from_appearance(true);

        let mut bar = if can_go_back {
            TopAppBar::small(title, theme)
        } else {
            TopAppBar::center_aligned(title, theme)
        };

        if can_go_back {
            bar = bar.leading_icon(
                "←",
                cx.listener(|this, _event, _window, cx| {
                    this.go_back();
                    cx.notify();
                }),
            );
        }

        bar.build().into_any_element()
    }

    fn render_tab_bar(current: Screen, cx: &mut Context<Self>) -> AnyElement {
        NavigationBarBuilder::new(true)
            .item(
                "💬",
                "会话",
                current == Screen::Sessions,
                cx.listener(move |this, _, _, cx| {
                    this.navigate_to(Screen::Sessions);
                    cx.notify();
                }),
            )
            .item(
                "🔌",
                "连接",
                current == Screen::Connect,
                cx.listener(move |this, _, _, cx| {
                    this.navigate_to(Screen::Connect);
                    cx.notify();
                }),
            )
            .item(
                "⚙️",
                "设置",
                current == Screen::Settings,
                cx.listener(move |this, _, _, cx| {
                    this.navigate_to(Screen::Settings);
                    cx.notify();
                }),
            )
            .build()
            .into_any_element()
    }

    fn render_connection_banner(&mut self, theme: MaterialTheme) -> Option<AnyElement> {
        if self.phase != ConnectionPhase::Connected {
            return None;
        }
        let daemon = self.daemon_label.clone().unwrap_or_else(|| "远程主机".into());
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1p5()
                .px_4()
                .py_1p5()
                .bg(rgb(theme.surface_container))
                .text_sm()
                .text_color(rgb(theme.on_surface_variant))
                .child(div().child("●").text_color(rgb(OK)))
                .child(div().flex_1().child(format!("已连接 {daemon}")))
                .into_any_element(),
        )
    }

    // ── Screen renders ──

    fn render_connect_screen(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        connect::render_connect_screen(self, cx)
    }

    fn render_sessions_screen(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        sessions::render_sessions_screen(self, cx)
    }

    fn render_chat_screen(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        chat::render_chat_screen(self, cx)
    }

    fn render_settings_screen(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        settings::render_settings_screen(self, cx)
    }
}

impl gpui::Render for WakuMobile {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // IME text since the last frame lands before paint; Enter submits.
        self.drain_input(cx);
        let theme = self.theme();
        let safe = self.safe_area;
        let show_tab_bar = self.screen.is_tab_root();

        gpui_mobile::set_system_chrome(&self.chrome_style());

        let banner = self.render_connection_banner(theme);
        let screen_content: AnyElement = match self.screen {
            Screen::Connect => self.render_connect_screen(cx).into_any_element(),
            Screen::Sessions => self.render_sessions_screen(cx).into_any_element(),
            Screen::Chat => self.render_chat_screen(cx).into_any_element(),
            Screen::Settings => self.render_settings_screen(cx).into_any_element(),
        };
        let tab_bar = Self::render_tab_bar(self.screen, cx);
        let nav_bar = Self::render_nav_bar(self.can_go_back(), self.screen, cx);
        let toast = self.toast.clone();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.surface))
            .text_color(rgb(theme.on_surface))
            .when(safe.top > 0.0, |d| {
                d.child(div().w_full().h(px(safe.top)).bg(rgb(theme.surface)))
            })
            .child(nav_bar)
            .when_some(banner, |d, banner| d.child(banner))
            .child(screen_content)
            .when(show_tab_bar, |d| d.child(tab_bar))
            .when(show_tab_bar && safe.bottom > 0.0, |d| {
                d.child(div().w_full().h(px(safe.bottom)).bg(rgb(theme.surface)))
            })
            .children(toast.map(|message| render_toast(&theme, message)))
    }
}

fn render_toast(theme: &MaterialTheme, message: String) -> AnyElement {
    div()
        .absolute()
        .bottom_24()
        .left_6()
        .right_6()
        .p_3()
        .rounded_lg()
        .bg(rgb(theme.inverse_surface))
        .text_color(rgb(theme.inverse_on_surface))
        .text_sm()
        .child(message)
        .into_any_element()
}

/// A dismissible error strip shared by the connect and chat screens.
pub fn render_error_strip(
    message: &str,
    cx: &mut Context<WakuMobile>,
    on_dismiss: impl Fn(&mut WakuMobile, &mut Context<WakuMobile>) + 'static,
) -> AnyElement {
    div()
        .id("error-strip")
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .mx_4()
        .mt_2()
        .px_3()
        .py_2()
        .rounded_lg()
        .bg(rgb(0x3A1D1F))
        .text_color(rgb(0xFFB4AB))
        .text_sm()
        .child(div().flex_1().child(message.to_owned()))
        .child(
            div()
                .id("error-dismiss")
                .px_1p5()
                .py_0p5()
                .text_color(rgb(0xFFB4AB))
                .hover(|style| style.text_color(rgb(0xFFFFFF)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                        on_dismiss(this, cx);
                    }),
                )
                .child("✕"),
        )
        .into_any_element()
}

/// Convenience for the accent-tinted primary action button used across
/// screens. Height stays above the 44px touch guideline.
pub fn primary_button(label: &str, disabled: bool) -> gpui::Stateful<gpui::Div> {
    let bg = if disabled { 0x2A3B55 } else { ACCENT };
    let text = if disabled { 0x8A97A8 } else { 0xFFFFFF };
    div()
        .id("primary-action")
        .h_12()
        .rounded_lg()
        .bg(rgb(bg))
        .text_color(rgb(text))
        .text_base()
        .font_weight(gpui::FontWeight::MEDIUM)
        .flex()
        .items_center()
        .justify_center()
        .child(label.to_owned())
}
