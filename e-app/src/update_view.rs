//! The update notice — a small card shown bottom-right when a newer release is
//! available, or to report check/install progress.

use floem::reactive::{SignalGet, SignalUpdate};
use floem::views::{dyn_container, label, scroll, stack, Decorators};
use floem::IntoView;

use crate::state::AppState;
use crate::theme;
use crate::updater::{current_version, UpdateStatus};

/// Strip the markdown noise from release notes for a clean in-app display.
fn clean_notes(notes: &str) -> String {
    let mut out = Vec::new();
    for line in notes.lines() {
        let t = line.trim();
        // Drop the title, the full-changelog footer and italic blurbs.
        if t.is_empty()
            || t.starts_with("**e ")
            || t.starts_with("**Full changelog")
            || t.starts_with('_')
        {
            if out.last().map(|l: &String| l.is_empty()).unwrap_or(true) {
                continue;
            }
            out.push(String::new());
            continue;
        }
        let line = t
            .trim_start_matches("### ")
            .trim_start_matches("## ")
            .replace("**", "")
            .replace('`', "");
        out.push(line);
    }
    out.join("\n").trim().to_string()
}

fn btn(text: &'static str, primary: bool) -> impl IntoView {
    label(move || text.to_string()).style(move |s| {
        let s = s
            .padding_horiz(14.0)
            .padding_vert(6.0)
            .border_radius(7.0)
            .font_size(13.0)
            .cursor(floem::style::CursorStyle::Pointer);
        if primary {
            s.background(theme::accent())
                .color(floem::peniko::Color::from_rgb8(0x14, 0x16, 0x1b))
                .hover(|s| s.background(theme::accent()))
        } else {
            s.background(theme::bg())
                .color(theme::fg())
                .border(1.0)
                .border_color(theme::border())
                .hover(|s| s.background(theme::bg_hover()))
        }
    })
}

/// The body of the notice, which depends on the current update status.
fn notice_body(state: AppState) -> impl IntoView {
    dyn_container(
        move || state.update_status.get(),
        move |status| match status {
            UpdateStatus::Downloading => label(|| "Downloading update…".to_string())
                .style(|s| s.color(theme::fg()).font_size(14.0))
                .into_any(),

            UpdateStatus::Installed => stack((
                label(|| "Update installed".to_string())
                    .style(|s| s.color(theme::fg()).font_size(15.0)),
                label(|| "Restart e to start using the new version.".to_string())
                    .style(|s| s.color(theme::fg_dim()).font_size(13.0).margin_top(2.0)),
                stack((
                    btn("Restart now", true).on_click_stop(move |_| state.restart_app()),
                    btn("Later", false).on_click_stop(move |_| state.dismiss_update()),
                ))
                .style(|s| s.gap(8.0).margin_top(12.0)),
            ))
            .style(|s| s.flex_col())
            .into_any(),

            UpdateStatus::Failed(e) => stack((
                label(|| "Update failed".to_string())
                    .style(|s| s.color(theme::fg()).font_size(15.0)),
                label(move || e.clone())
                    .style(|s| s.color(theme::fg_dim()).font_size(12.0).margin_top(2.0)),
                stack((
                    btn("Retry", true).on_click_stop(move |_| state.install_update()),
                    btn("Dismiss", false).on_click_stop(move |_| state.dismiss_update()),
                ))
                .style(|s| s.gap(8.0).margin_top(12.0)),
            ))
            .style(|s| s.flex_col())
            .into_any(),

            UpdateStatus::UpToDate => stack((
                label(|| format!("You're up to date (e {})", current_version()))
                    .style(|s| s.color(theme::fg()).font_size(14.0)),
                btn("OK", false)
                    .on_click_stop(move |_| state.dismiss_update())
                    .style(|s| s.margin_top(12.0)),
            ))
            .style(|s| s.flex_col())
            .into_any(),

            UpdateStatus::CheckFailed(e) => stack((
                label(|| "Couldn't check for updates".to_string())
                    .style(|s| s.color(theme::fg()).font_size(15.0)),
                label(move || e.clone())
                    .style(|s| s.color(theme::fg_dim()).font_size(12.0).margin_top(2.0)),
                stack((
                    btn("Retry", true).on_click_stop(move |_| state.check_for_updates(true)),
                    btn("Dismiss", false).on_click_stop(move |_| state.dismiss_update()),
                ))
                .style(|s| s.gap(8.0).margin_top(12.0)),
            ))
            .style(|s| s.flex_col())
            .into_any(),

            // Idle / Checking with an available update.
            _ => {
                let info = state.update_info.get();
                let Some(info) = info else {
                    return label(String::new).into_any();
                };
                let version = info.version.clone();
                let notes = clean_notes(&info.notes);

                let header = label(move || format!("Update available — e {version}"))
                    .style(|s| s.color(theme::fg()).font_size(15.0));

                let toggle = label(|| "What's new".to_string())
                    .style(|s| {
                        s.color(theme::accent())
                            .font_size(12.0)
                            .margin_top(2.0)
                            .cursor(floem::style::CursorStyle::Pointer)
                    })
                    .on_click_stop(move |_| state.update_notes_open.update(|o| *o = !*o));

                let notes_view = dyn_container(
                    move || state.update_notes_open.get(),
                    move |open| {
                        if open {
                            let notes = notes.clone();
                            scroll(label(move || notes.clone()).style(|s| {
                                s.color(theme::fg_dim())
                                    .font_size(12.0)
                                    .line_height(1.4)
                                    .width(300.0)
                            }))
                            .style(|s| s.max_height(220.0).width(308.0).margin_top(8.0))
                            .into_any()
                        } else {
                            label(String::new).into_any()
                        }
                    },
                );

                let actions = stack((
                    btn("Update now", true).on_click_stop(move |_| state.install_update()),
                    btn("Later", false).on_click_stop(move |_| state.dismiss_update()),
                ))
                .style(|s| s.gap(8.0).margin_top(12.0));

                stack((header, toggle, notes_view, actions))
                    .style(|s| s.flex_col())
                    .into_any()
            }
        },
    )
}

pub fn update_notice(state: AppState) -> impl IntoView {
    let visible = move || {
        state.update_info.get().is_some()
            || matches!(
                state.update_status.get(),
                UpdateStatus::Downloading
                    | UpdateStatus::Installed
                    | UpdateStatus::Failed(_)
                    | UpdateStatus::CheckFailed(_)
                    | UpdateStatus::UpToDate
            )
    };

    let card = notice_body(state).style(|s| {
        s.flex_col()
            .width(340.0)
            .padding(16.0)
            .background(theme::bg_panel())
            .border(1.0)
            .border_color(theme::border())
            .border_radius(12.0)
    });

    stack((card,)).style(move |s| {
        let s = s
            .absolute()
            .inset_right(20.0)
            .inset_bottom(46.0)
            .z_index(50);
        if visible() {
            s
        } else {
            s.hide()
        }
    })
}
