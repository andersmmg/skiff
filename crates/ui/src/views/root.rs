//! Root view: header, sidebar, task list, settings page.
//!
//! The add-download flow uses gpui-kit's Dialog and Input components, so text
//! editing — selection, clipboard paste, undo, IME — comes for free, and the
//! dialog renders through the window's Root overlay layer (no hand-rolled
//! overlay in this view). The settings page edits the persisted [`Settings`]
//! entity; engine-bound options (port, binary, limits) apply on restart.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Arc;

use engine::RpcClient;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::form::{Field, Form};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme, IconName, WindowExt as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, Div, Entity, Focusable as _, Stateful, WeakEntity,
    Window, div, px,
};
use state::{Connection, Download, Filter, Settings, Skiff, TaskList};

use crate::format;
use crate::views::{sidebar, task_row};

/// Which pane the main area shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Tasks,
    Settings,
}

pub struct Root {
    pub(crate) filter: Filter,
    pub(crate) page: Page,
    pub(crate) client: Arc<RpcClient>,
    tasks: Entity<TaskList>,
    connection: Entity<Connection>,
    url_input: Entity<InputState>,
    // Settings-page fields, filled from the settings entity when the page
    // opens and read back on save.
    dir_input: Entity<InputState>,
    concurrent_input: Entity<InputState>,
    connections_input: Entity<InputState>,
    split_input: Entity<InputState>,
    port_input: Entity<InputState>,
    secret_input: Entity<InputState>,
    binary_input: Entity<InputState>,
    /// Last save feedback: (message, is_error).
    save_state: Option<(String, bool)>,
}

impl Root {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let skiff = Skiff::global(cx);
        let tasks = skiff.tasks.clone();
        let connection = skiff.connection.clone();
        let settings = skiff.settings.clone();
        let client = skiff.client.clone();
        cx.observe(&tasks, |_, _, cx| cx.notify()).detach();
        cx.observe(&connection, |_, _, cx| cx.notify()).detach();
        // Refresh on save so the footer's RPC port and the form stay in sync.
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        let url_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://…   magnet:?xt=…   ed2k://…   file.torrent")
        });
        let mut field = |placeholder: &str, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let dir_input = field("~/Downloads", cx);
        let concurrent_input = field("4", cx);
        let connections_input = field("8", cx);
        let split_input = field("8", cx);
        let port_input = field("6801", cx);
        let secret_input = field("RPC secret", cx);
        let binary_input = field("auto-detect aria2-next", cx);
        Self {
            filter: Filter::All,
            page: Page::Tasks,
            client,
            tasks,
            connection,
            url_input,
            dir_input,
            concurrent_input,
            connections_input,
            split_input,
            port_input,
            secret_input,
            binary_input,
            save_state: None,
        }
    }

    fn open_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Start fresh each time the dialog opens.
        self.url_input
            .update(cx, |state, cx| state.set_value("", window, cx));

        let input = self.url_input.clone();
        let client = self.client.clone();
        let root = cx.weak_entity();
        // The builder may re-run when the dialog layer rebuilds; focus the
        // input only on the first build of this opening.
        let initial_focus = Cell::new(true);

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                let focus = input.focus_handle(cx);
                focus.focus(window, cx);
            }
            let (input_ok, client_ok, root_ok) = (input.clone(), client.clone(), root.clone());
            let (input_add, client_add, root_add) = (input.clone(), client.clone(), root.clone());
            dialog
                .title("New download")
                .w(px(560.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(Input::new(&input))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Press Enter or Add to queue. Ctrl+V pastes from the clipboard."),
                        ),
                )
                .on_ok(move |_, _window, cx| submit_add(&input_ok, &client_ok, &root_ok, cx))
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("add-cancel")
                                .outline()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("add-submit")
                                .primary()
                                .label("Add")
                                .on_click(move |_, window, cx| {
                                    if submit_add(&input_add, &client_add, &root_add, cx) {
                                        window.close_dialog(cx);
                                    }
                                }),
                        ),
                )
        });
    }

    fn header(&self, cx: &mut Context<Self>) -> Div {
        let (download, upload) = {
            let tasks = self.tasks.read(cx);
            (
                format::speed(tasks.stats.download_speed()),
                format::speed(tasks.stats.upload_speed()),
            )
        };
        let muted = cx.theme().muted_foreground;

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .py_2()
            .bg(cx.theme().secondary)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .child("Skiff"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("↓ {download}   ↑ {upload}")),
                    ),
            )
            .child(
                Button::new("add-download")
                    .primary()
                    .icon(IconName::Plus)
                    .label("Add")
                    .on_click(cx.listener(|this, _, window, cx| this.open_add(window, cx))),
            )
    }

    fn task_list(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let downloads: Vec<Download> = self
            .tasks
            .read(cx)
            .visible(self.filter)
            .cloned()
            .collect();
        if downloads.is_empty() {
            let muted = cx.theme().muted_foreground;
            return div()
                .id("task-list-empty")
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(muted)
                .child("Nothing here yet — press Add to start a download.");
        }
        let rows: Vec<AnyElement> = downloads
            .iter()
            .map(|download| task_row::render(download, cx).into_any_element())
            .collect();
        div()
            .id("task-list")
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .children(rows)
    }

    /// Fill the form inputs from the persisted settings (page open).
    pub(crate) fn load_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = Skiff::global(cx).settings.read(cx).clone();
        let mut set = |state: &Entity<InputState>, value: String, cx: &mut Context<Self>| {
            state
                .update(cx, |input, cx| input.set_value(value, window, cx));
        };
        set(
            &self.dir_input,
            settings.download_dir.display().to_string(),
            cx,
        );
        set(&self.concurrent_input, settings.max_concurrent.to_string(), cx);
        set(&self.connections_input, settings.connection_limit.to_string(), cx);
        set(&self.split_input, settings.split.to_string(), cx);
        set(&self.port_input, settings.rpc_port.to_string(), cx);
        set(&self.secret_input, settings.rpc_secret.clone(), cx);
        set(
            &self.binary_input,
            settings
                .engine_binary
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            cx,
        );
        self.save_state = None;
        cx.notify();
    }

    /// Validate the form and persist it; engine-bound options need a restart.
    fn save_settings(&mut self, cx: &mut Context<Self>) {
        match self.collect_settings(cx) {
            Ok(settings) => {
                let config = settings.engine_config();
                let entity = Skiff::global(cx).settings.clone();
                entity.update(cx, |current, cx| {
                    *current = settings;
                    cx.notify();
                });
                entity.read(cx).save();
                // Engine-bound options apply live: the supervisor kills the
                // sidecar and respawns it from the new config, and unfinished
                // tasks come back from the engine session — no app restart.
                Skiff::global(cx).reconfigure.send(config);
                self.save_state =
                    Some(("Saved — engine restarting with the new settings".into(), false));
            }
            Err(error) => self.save_state = Some((error, true)),
        }
        cx.notify();
    }

    fn collect_settings(&self, cx: &App) -> Result<Settings, String> {
        let dir = input_text(&self.dir_input, cx);
        if dir.trim().is_empty() {
            return Err("Download directory cannot be empty".into());
        }
        let port = parse_number::<u16>(&self.port_input, "RPC port", cx)?;
        if !(1024..=65535).contains(&port) {
            return Err("RPC port must be between 1024 and 65535".into());
        }
        let secret = input_text(&self.secret_input, cx).trim().to_string();
        if secret.is_empty() {
            return Err("RPC secret cannot be empty".into());
        }
        let max_concurrent =
            parse_number::<u32>(&self.concurrent_input, "Max concurrent downloads", cx)?;
        let connection_limit =
            parse_number::<u32>(&self.connections_input, "Connections per server", cx)?;
        let split = parse_number::<u32>(&self.split_input, "Split", cx)?;
        if max_concurrent == 0 || connection_limit == 0 || split == 0 {
            return Err("Limits must be at least 1".into());
        }
        let binary = input_text(&self.binary_input, cx);
        let binary = binary.trim();
        Ok(Settings {
            download_dir: PathBuf::from(dir.trim()),
            max_concurrent,
            connection_limit,
            split,
            rpc_port: port,
            rpc_secret: secret,
            engine_binary: (!binary.is_empty()).then(|| PathBuf::from(binary)),
        })
    }

    fn settings_page(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let muted = cx.theme().muted_foreground;
        let success = cx.theme().success;
        let danger = cx.theme().danger;
        let port = Skiff::global(cx).settings.read(cx).rpc_port;

        let section = |title: &'static str, fields: Form| {
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        .text_color(muted)
                        .child(title),
                )
                .child(fields)
        };

        let copy = cx.listener(|this, _, _window, cx| {
            let secret = input_text(&this.secret_input, cx);
            cx.write_to_clipboard(ClipboardItem::new_string(secret));
            this.save_state = Some(("Secret copied to clipboard".into(), false));
            cx.notify();
        });
        // Regenerate fills the field; persisting stays one Save click away so
        // the on-disk secret never changes without the user seeing it.
        let regenerate = cx.listener(|this, _, window, cx| {
            let secret: String = (0..32).map(|_| fastrand::alphanumeric()).collect();
            this.secret_input
                .update(cx, |input, cx| input.set_value(secret, window, cx));
            this.save_state =
                Some(("New secret generated — press Save to apply".into(), false));
            cx.notify();
        });

        div()
            .id("settings-page")
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_8()
                    .p_6()
                    .max_w(px(720.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .child("Settings"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("Engine RPC listens on 127.0.0.1 only."),
                            ),
                    )
                    .child(section(
                        "Downloads",
                        Form::vertical()
                            .child(
                                Field::new()
                                    .label("Download directory")
                                    .child(Input::new(&self.dir_input)),
                            )
                            .child(
                                Field::new()
                                    .label("Max concurrent downloads")
                                    .child(Input::new(&self.concurrent_input)),
                            )
                            .child(
                                Field::new()
                                    .label("Connections per server")
                                    .child(Input::new(&self.connections_input)),
                            )
                            .child(
                                Field::new()
                                    .label("Split")
                                    .child(Input::new(&self.split_input)),
                            ),
                    ))
                    .child(section(
                        "Engine RPC (browser extensions)",
                        Form::vertical()
                            .child(
                                Field::new()
                                    .label("RPC port")
                                    .description(format!(
                                        "Extensions point at http://127.0.0.1:{port}/jsonrpc"
                                    ))
                                    .child(Input::new(&self.port_input)),
                            )
                            .child(
                                Field::new()
                                    .label("RPC secret")
                                    .description("First RPC parameter: token:<secret>")
                                    .child(
                                        div()
                                            .flex()
                                            .gap_2()
                                            .child(div().flex_1().min_w(px(0.)).child(Input::new(&self.secret_input)))
                                            .child(
                                                Button::new("copy-secret")
                                                    .outline()
                                                    .icon(IconName::Copy)
                                                    .label("Copy")
                                                    .on_click(copy),
                                            )
                                            .child(
                                                Button::new("regenerate-secret")
                                                    .outline()
                                                    .icon(IconName::RotateCw)
                                                    .label("Regenerate")
                                                    .on_click(regenerate),
                                            ),
                                    ),
                            ),
                    ))
                    .child(section(
                        "Engine",
                        Form::vertical().child(
                            Field::new()
                                .label("Engine binary")
                                .description("Path to aria2-next; blank to auto-detect")
                                .child(Input::new(&self.binary_input)),
                        ),
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                Button::new("save-settings")
                                    .primary()
                                    .label("Save")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.save_settings(cx)
                                    })),
                            )
                            .child(
                                div().text_xs().child(match &self.save_state {
                                    Some((message, true)) => {
                                        div().text_color(danger).child(message.clone())
                                    }
                                    Some((message, false)) => {
                                        div().text_color(success).child(message.clone())
                                    }
                                    None => div(),
                                }),
                            ),
                    ),
            )
    }
}

fn input_text(state: &Entity<InputState>, cx: &App) -> String {
    state.read(cx).value().to_string()
}

fn parse_number<T: std::str::FromStr>(
    state: &Entity<InputState>,
    label: &str,
    cx: &App,
) -> Result<T, String> {
    input_text(state, cx)
        .trim()
        .parse()
        .map_err(|_| format!("{label} must be a number"))
}

/// Read the URL from the input and queue it. Returns whether the dialog may
/// close (a rejected submit keeps it open for editing).
fn submit_add(
    input: &Entity<InputState>,
    client: &Arc<RpcClient>,
    root: &WeakEntity<Root>,
    cx: &mut App,
) -> bool {
    let url = input.read(cx).value();
    let url = url.trim().to_owned();
    if url.is_empty() {
        return false;
    }
    if let Some(root) = root.upgrade() {
        let _ = root.update(cx, |this, cx| {
            this.filter = Filter::All;
            cx.notify();
        });
    }
    // reqwest needs tokio's reactor; GPUI's executor has none, so the call
    // goes through the engine runtime instead.
    let call = Arc::clone(client);
    let run = Arc::clone(client);
    call.spawn(async move {
        match run.add_uri(&[url.clone()], serde_json::json!({})).await {
            Ok(gid) => log::info!("queued {url} as {gid}"),
            Err(error) => log::warn!("cannot queue {url}: {error}"),
        }
    });
    true
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let counts = self.tasks.read(cx).counts();
        let status = self.connection.read(cx).status.clone();
        let background = cx.theme().background;
        let foreground = cx.theme().foreground;

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(background)
            .text_color(foreground)
            .text_sm()
            .child(self.header(cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.))
                    .child(sidebar::render(
                        self.filter,
                        self.page,
                        &counts,
                        &status,
                        cx,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .child(match self.page {
                                Page::Tasks => self.task_list(cx).into_any_element(),
                                Page::Settings => self.settings_page(cx).into_any_element(),
                            }),
                    ),
            )
    }
}
