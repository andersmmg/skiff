//! One download: name, progress bar, speed, status, and per-task actions —
//! composed from gpui-kit's Progress and Button components.

use gpui_kit::component::button::Button;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{ActiveTheme, IconName, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Div, SharedString, div, px};
use state::{Download, DownloadStatus};

use crate::format;
use crate::Root;

pub fn render(download: &Download, cx: &mut Context<Root>) -> impl IntoElement {
    let (status_label, status_color) = match download.status {
        DownloadStatus::Active => ("Downloading", cx.theme().primary),
        DownloadStatus::Waiting => ("Queued", cx.theme().muted_foreground),
        DownloadStatus::Paused => ("Paused", cx.theme().warning),
        DownloadStatus::Complete => ("Complete", cx.theme().success),
        DownloadStatus::Failed => ("Failed", cx.theme().danger),
        DownloadStatus::Removed => ("Removed", cx.theme().muted_foreground),
    };
    let border = cx.theme().border;
    let muted = cx.theme().muted_foreground;
    let hover_bg = cx.theme().muted;
    let danger = cx.theme().danger;

    let progress = download.progress();
    let indeterminate = download.total == 0
        && matches!(
            download.status,
            DownloadStatus::Active | DownloadStatus::Waiting
        );

    let mut row = div()
        .id(SharedString::from(download.gid.clone()))
        .flex()
        .flex_col()
        .gap_2()
        .px_4()
        .py_3()
        .border_b_1()
        .border_color(border)
        .hover(move |style| style.bg(hover_bg));

    // Title line: name on the left, speeds and status chip on the right.
    row = row.child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .truncate()
                    .child(download.name.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_xs()
                    .text_color(muted)
                    .child(speed_line(download))
                    .child(
                        div()
                            .px_2()
                            .py(px(2.))
                            .rounded_full()
                            .text_color(status_color)
                            .border_1()
                            .border_color(status_color)
                            .child(status_label),
                    ),
            ),
    );

    // Progress bar.
    row = row.child(
        Progress::new(format!("progress-{}", download.gid))
            .value(progress * 100.0)
            .loading(indeterminate)
            .color(status_color),
    );

    // Third line: size/percent/ETA on the left, actions on the right.
    row = row.child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .text_xs()
            .text_color(muted)
            .child(div().child(detail_line(download)))
            .child(actions(download, cx)),
    );

    if let Some(error) = &download.error {
        row = row.child(div().text_xs().text_color(danger).child(error.clone()));
    }

    row
}

fn speed_line(download: &Download) -> String {
    match download.status {
        DownloadStatus::Active => format!("↓ {}", format::speed(download.download_speed)),
        // Upload figures only ever apply to torrents, and a seeding task
        // with nobody leeching has nothing worth showing.
        DownloadStatus::Complete
            if download.torrent && download.upload_speed > 0 =>
        {
            format!("↑ {}", format::speed(download.upload_speed))
        }
        _ => String::new(),
    }
}

fn detail_line(download: &Download) -> String {
    match download.status {
        // Finished: the headline number is the file on disk. Upload totals
        // are a torrent-only detail.
        DownloadStatus::Complete => {
            let size = if download.total > 0 {
                format::bytes(download.total)
            } else if download.completed > 0 {
                format::bytes(download.completed)
            } else {
                "unknown size".to_string()
            };
            if download.torrent && download.uploaded > 0 {
                format!("{size} · {} uploaded", format::bytes(download.uploaded))
            } else {
                size
            }
        }
        DownloadStatus::Failed | DownloadStatus::Removed => String::new(),
        _ => {
            let mut parts = vec![
                format!(
                    "{} / {}",
                    format::bytes(download.completed),
                    if download.total == 0 {
                        "unknown size".to_string()
                    } else {
                        format::bytes(download.total)
                    }
                ),
                format!("{:.1}%", download.progress() * 100.0),
            ];
            if download.status == DownloadStatus::Active
                && download.download_speed > 0
                && download.total > download.completed
            {
                let seconds = (download.total - download.completed) / download.download_speed;
                parts.push(format!("{} left", format::duration(seconds)));
            }
            if download.connections > 1 {
                parts.push(format!("{} connections", download.connections));
            }
            parts.join(" · ")
        }
    }
}

fn actions(download: &Download, cx: &mut Context<Root>) -> Div {
    let mut bar = div().flex().gap_2();

    let pause_target = match download.status {
        DownloadStatus::Active | DownloadStatus::Waiting => Some(false),
        DownloadStatus::Paused => Some(true),
        _ => None,
    };
    if let Some(resume) = pause_target {
        let gid = download.gid.clone();
        let label = if resume { "Resume" } else { "Pause" };
        let icon = if resume {
            IconName::Play
        } else {
            IconName::Pause
        };
        bar = bar.child(
            Button::new(format!("task-toggle-{gid}"))
                .xsmall()
                .outline()
                .icon(icon)
                .label(label)
                .on_click(cx.listener(move |this, _, _window, _cx| {
                    let call = this.client.clone();
                    let gid = gid.clone();
                    let run = call.clone();
                    call.spawn(async move {
                        let result = if resume {
                            run.unpause(&gid).await
                        } else {
                            run.pause(&gid).await
                        };
                        if let Err(error) = result {
                            log::warn!("pause/unpause {gid} failed: {error}");
                        }
                    });
                })),
        );
    }

    if !matches!(
        download.status,
        DownloadStatus::Complete | DownloadStatus::Removed
    ) {
        let gid = download.gid.clone();
        bar = bar.child(
            Button::new(format!("task-remove-{gid}"))
                .xsmall()
                .outline()
                .icon(IconName::Close)
                .label("Remove")
                .on_click(cx.listener(move |this, _, _window, _cx| {
                    let call = this.client.clone();
                    let gid = gid.clone();
                    let run = call.clone();
                    call.spawn(async move {
                        // A queued task answers to remove; a started one may
                        // demand the forced variant.
                        let result = match run.remove(&gid).await {
                            Ok(_) => Ok(()),
                            Err(_) => run.force_remove(&gid).await.map(|_| ()),
                        };
                        if let Err(error) = result {
                            log::warn!("remove {gid} failed: {error}");
                        }
                    });
                })),
        );
    }

    bar
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished(torrent: bool) -> Download {
        Download {
            gid: "gid1".into(),
            name: "f.zip".into(),
            uri: None,
            status: DownloadStatus::Complete,
            total: 1024 * 1024,
            completed: 1024 * 1024,
            uploaded: 2048,
            download_speed: 0,
            upload_speed: 0,
            connections: 1,
            torrent,
            error: None,
        }
    }

    #[test]
    fn complete_non_torrent_shows_final_size_without_upload() {
        let download = finished(false);
        assert_eq!(detail_line(&download), "1.0 MiB");
        // No upload row, no upload chip — not even at zero.
        assert!(!detail_line(&download).contains("uploaded"));
        assert!(speed_line(&download).is_empty());
    }

    #[test]
    fn complete_torrent_shows_uploaded_details() {
        let mut download = finished(true);
        assert_eq!(detail_line(&download), "1.0 MiB · 2.0 KiB uploaded");
        // Seeding with traffic: the chip appears; idle seeding stays quiet.
        download.upload_speed = 1024;
        assert_eq!(speed_line(&download), "↑ 1.0 KiB/s");
        download.upload_speed = 0;
        assert!(speed_line(&download).is_empty());
    }

    #[test]
    fn complete_without_reported_size_says_unknown() {
        let mut download = finished(false);
        download.total = 0;
        download.completed = 0;
        assert_eq!(detail_line(&download), "unknown size");
    }
}
