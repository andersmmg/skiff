//! Sidebar: filter navigation, settings entry, engine status, built on
//! gpui-kit's Sidebar component so hover, collapse, and theming come from
//! the library.

use gpui_kit::component::button::Button;
use gpui_kit::component::sidebar::{
    Sidebar, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem,
};
use gpui_kit::component::{ActiveTheme, IconName, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{Context, div, px};
use state::{ConnectionStatus, Counts, Filter, Skiff};

use crate::{Page, Root};

pub fn render(
    filter: Filter,
    page: Page,
    counts: &Counts,
    status: &ConnectionStatus,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let (dot, label) = match status {
        ConnectionStatus::Connecting => (cx.theme().warning, "Engine starting…"),
        ConnectionStatus::Connected => (cx.theme().success, "Engine connected"),
        ConnectionStatus::Lost(_) => (cx.theme().danger, "Engine lost"),
    };
    let muted = cx.theme().muted_foreground;
    // The stable extension-facing RPC endpoint, for copying into browser
    // extensions (the secret lives in the settings file).
    let rpc_port = Skiff::global(cx).settings.read(cx).rpc_port;

    let mut menu = SidebarMenu::new();
    for item in Filter::ALL {
        let count = counts.of(item);
        let selected = item == filter && page == Page::Tasks;
        let icon = match item {
            Filter::All => IconName::Inbox,
            Filter::Active => IconName::Play,
            Filter::Waiting => IconName::Loader,
            Filter::Paused => IconName::Pause,
            Filter::Complete => IconName::CircleCheck,
            Filter::Failed => IconName::CircleX,
        };
        menu = menu.child(
            SidebarMenuItem::new(item.label())
                .icon(icon)
                .active(selected)
                // An inline pill, not the kit's `Badge`: that one is an
                // absolutely-positioned overlay meant for icon corners, so
                // inside this row it collapsed to zero width and its red
                // bubble got clipped at the sidebar edge.
                .suffix(move |_, cx| {
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(count > 0, |this| {
                            this.min_w(px(18.))
                                .h(px(16.))
                                .px_1()
                                .rounded_full()
                                .bg(cx.theme().secondary)
                                .text_color(cx.theme().muted_foreground)
                                .text_size(px(10.))
                                .child(if count > 99 {
                                    "99+".to_string()
                                } else {
                                    count.to_string()
                                })
                        })
                })
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.filter = item;
                    this.page = Page::Tasks;
                    cx.notify();
                })),
        );
    }

    let settings_menu = SidebarMenu::new().child(
        SidebarMenuItem::new("Settings")
            .icon(IconName::Settings)
            .active(page == Page::Settings)
            .on_click(cx.listener(|this, _, window, cx| {
                this.page = Page::Settings;
                this.load_settings(window, cx);
            })),
    );

    let clear = Button::new("clear-finished")
        .xsmall()
        .outline()
        .label("Clear finished")
        .on_click(cx.listener(|this, _, _window, _cx| {
            let call = this.client.clone();
            let run = call.clone();
            call.spawn(async move {
                match run.purge_download_result().await {
                    Ok(_) => log::debug!("purged finished download results"),
                    Err(error) => log::warn!("purge failed: {error}"),
                }
            });
        }));

    Sidebar::new("nav")
        .w(px(240.))
        .header(
            SidebarHeader::new().child(
                div()
                    .text_xs()
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .child("DOWNLOADS"),
            ),
        )
        .child(SidebarGroup::new("Filters").child(menu))
        .child(SidebarGroup::new("App").child(settings_menu))
        .footer(SidebarFooter::new().child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_xs()
                                .child(div().w(px(8.)).h(px(8.)).rounded_full().bg(dot))
                                .child(label),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(format!("RPC :{rpc_port}")),
                        ),
                )
                .child(clear),
        ))
}
