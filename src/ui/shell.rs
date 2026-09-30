//! Application shell: the custom title bar, navigation sidebar and page header that frame every
//! screen. Presentation only — state and intents live in the view that composes the shell.

use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    div, px, App, ClickEvent, Div, FontWeight, IntoElement, Rgba, SharedString, Window,
    WindowControlArea,
};

use crate::ui::components::Divider;
use crate::ui::theme::{self, Theme};

type NavHandler = Rc<dyn Fn(Screen, &ClickEvent, &mut Window, &mut App)>;

/// The primary pages of the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    AutoAccept,
    Notifications,
    Settings,
}

impl Screen {
    pub const ALL: [Screen; 3] = [Screen::AutoAccept, Screen::Notifications, Screen::Settings];

    fn label(self) -> &'static str {
        match self {
            Screen::AutoAccept => "Auto Accept",
            Screen::Notifications => "Notifications",
            Screen::Settings => "Settings",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Screen::AutoAccept => "\u{25C9}",    // ◉
            Screen::Notifications => "\u{2709}", // ✉
            Screen::Settings => "\u{2699}",      // ⚙
        }
    }
}

/// The custom window chrome: draggable region, app mark and window controls.
#[derive(IntoElement)]
pub struct TitleBar {}

impl TitleBar {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for TitleBar {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn app_mark(t: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(18.0))
        .rounded(px(5.0))
        .bg(t.accent)
        .child(div().size(px(7.0)).rounded_full().bg(t.accent_fg))
}

impl RenderOnce for TitleBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx);
        div()
            .flex()
            .items_center()
            .h(px(38.0))
            .w_full()
            .flex_shrink_0()
            .bg(t.sidebar_bg)
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::space_2())
                    .flex_1()
                    .h_full()
                    .min_w(px(0.0))
                    .px(theme::space_3())
                    .window_control_area(WindowControlArea::Drag)
                    .child(app_mark(&t))
                    .child(
                        div()
                            .text_size(theme::text_small())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text)
                            .child("League Auto Accept"),
                    ),
            )
            .child(WindowControls::new())
    }
}

/// Minimize / maximize / close, drawn as part of the custom title bar.
///
/// These use native `WindowControlArea` hit-testing so Windows performs the real non-client
/// actions — including restoring a maximized window and the Windows 11 snap-layout flyout on the
/// maximize button. Close is routed through `WM_CLOSE`, so the existing minimize-to-tray handler
/// in `main.rs` decides between hiding and quitting.
#[derive(IntoElement)]
struct WindowControls {}

impl WindowControls {
    fn new() -> Self {
        Self {}
    }
}

fn control_button(
    id: &'static str,
    glyph: &'static str,
    area: WindowControlArea,
    fg: Rgba,
    hover_bg: Rgba,
    hover_fg: Rgba,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(46.0))
        .h(px(38.0))
        .cursor_pointer()
        .text_size(px(11.0))
        .text_color(fg)
        .window_control_area(area)
        .hover(move |style| style.bg(hover_bg).text_color(hover_fg))
        .child(glyph)
}

impl RenderOnce for WindowControls {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx);
        div()
            .flex()
            .items_center()
            .h_full()
            .child(control_button(
                "win-min",
                "\u{2500}", // ─
                WindowControlArea::Min,
                t.text_muted,
                t.surface_hover,
                t.text,
            ))
            .child(control_button(
                "win-max",
                "\u{25A1}", // □
                WindowControlArea::Max,
                t.text_muted,
                t.surface_hover,
                t.text,
            ))
            .child(control_button(
                "win-close",
                "\u{2715}", // ✕
                WindowControlArea::Close,
                t.text_muted,
                t.danger,
                t.accent_fg,
            ))
    }
}

/// Left navigation rail with the app's pages and a pinned status footer.
#[derive(IntoElement)]
pub struct Sidebar {
    active: Screen,
    summary: SharedString,
    status_color: Rgba,
    status_detail: SharedString,
    on_nav: NavHandler,
}

impl Sidebar {
    pub fn new(
        active: Screen,
        summary: impl Into<SharedString>,
        status_color: Rgba,
        status_detail: impl Into<SharedString>,
        on_nav: impl Fn(Screen, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            active,
            summary: summary.into(),
            status_color,
            status_detail: status_detail.into(),
            on_nav: Rc::new(on_nav),
        }
    }
}

fn nav_item(screen: Screen, active: bool, t: &Theme, on_nav: NavHandler) -> impl IntoElement {
    let (bg, fg) = if active {
        (t.accent_subtle, t.text)
    } else {
        (t.sidebar_bg, t.text_muted)
    };
    div()
        .id(match screen {
            Screen::AutoAccept => "nav-auto-accept",
            Screen::Notifications => "nav-notifications",
            Screen::Settings => "nav-settings",
        })
        .relative()
        .flex()
        .items_center()
        .gap(theme::space_3())
        .px(theme::space_3())
        .py(theme::space_2())
        .rounded(theme::radius_md())
        .cursor_pointer()
        .bg(bg)
        .text_color(fg)
        .when(active, |this| {
            this.child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(8.0))
                    .bottom(px(8.0))
                    .w(px(2.0))
                    .rounded(px(1.0))
                    .bg(t.accent),
            )
        })
        .hover(|style| {
            if active {
                style
            } else {
                style.bg(t.surface_hover).text_color(t.text)
            }
        })
        .on_click(move |event, window, app| on_nav(screen, event, window, app))
        .child(
            div()
                .w(px(16.0))
                .text_center()
                .text_size(px(13.0))
                .child(screen.glyph()),
        )
        .child(
            div()
                .text_size(theme::text_body())
                .font_weight(FontWeight::MEDIUM)
                .child(screen.label()),
        )
}

impl RenderOnce for Sidebar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx);
        let active = self.active;
        let on_nav = self.on_nav.clone();
        let nav_items = Screen::ALL
            .iter()
            .map(|&screen| nav_item(screen, screen == active, &t, on_nav.clone()));

        div()
            .flex()
            .flex_col()
            .w(px(216.0))
            .h_full()
            .flex_shrink_0()
            .bg(t.sidebar_bg)
            .border_r_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .flex_1()
                    .min_h(px(0.0))
                    .p(theme::space_3())
                    .children(nav_items),
            )
            .child(Divider::horizontal())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::space_2())
                    .p(theme::space_3())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::space_2())
                            .child(div().size(px(8.0)).rounded_full().bg(self.status_color))
                            .child(
                                div()
                                    .text_size(theme::text_small())
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(t.text)
                                    .child(self.summary),
                            ),
                    )
                    .child(
                        div()
                            .text_size(theme::text_caption())
                            .text_color(t.text_faint)
                            .child(self.status_detail),
                    ),
            )
    }
}

/// A page's title block, with an optional right-aligned primary action.
#[derive(IntoElement)]
pub struct PageHeader {
    title: SharedString,
    subtitle: Option<SharedString>,
    action: Option<gpui::AnyElement>,
}

impl PageHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            action: None,
        }
    }

    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl RenderOnce for PageHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx);
        div()
            .flex()
            .items_start()
            .justify_between()
            .gap(theme::space_4())
            .px(theme::space_8())
            .pt(theme::space_6())
            .pb(theme::space_4())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(theme::text_title())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text)
                            .child(self.title),
                    )
                    .when_some(self.subtitle, |this, subtitle| {
                        this.child(
                            div()
                                .text_size(theme::text_small())
                                .text_color(t.text_muted)
                                .child(subtitle),
                        )
                    }),
            )
            .when_some(self.action, |this, action| this.child(action))
    }
}
