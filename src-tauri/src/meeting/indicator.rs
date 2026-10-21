//! The floating "recording" pill shown while a meeting runs: elapsed time,
//! pause and stop, always on top, top-right of the screen, out of the way of
//! the dictation overlay. Its UI lives in `src/meeting-indicator/`.

use tauri::{AppHandle, Manager};

use super::{MeetingPhase, MeetingStatus};
use crate::settings::get_settings;

const LABEL: &str = "meeting_indicator";
const WIDTH: f64 = 236.0;
const HEIGHT: f64 = 52.0;
/// Distance from the top-right corner of the usable screen area.
const MARGIN: f64 = 12.0;

#[cfg(target_os = "macos")]
tauri_nspanel::tauri_panel! {
    panel!(MeetingIndicatorPanel {
        config: {
            can_become_key_window: false,
            is_floating_panel: true
        }
    })
}

/// Shows the indicator while a meeting records (if the user wants it) and
/// hides it otherwise.
pub(super) fn sync(app: &AppHandle, status: &MeetingStatus) {
    let wanted = matches!(status.phase, MeetingPhase::Recording | MeetingPhase::Paused)
        && get_settings(app).meeting.show_indicator;
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if wanted {
            show(&handle);
        } else if let Some(window) = handle.get_webview_window(LABEL) {
            let _ = window.hide();
        }
    });
}

fn position(app: &AppHandle) -> Option<(f64, f64)> {
    let monitor = app.primary_monitor().ok().flatten()?;
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let x = area.position.x as f64 / scale + area.size.width as f64 / scale - WIDTH - MARGIN;
    let y = area.position.y as f64 / scale + MARGIN;
    Some((x, y))
}

fn show(app: &AppHandle) {
    if app.get_webview_window(LABEL).is_none() {
        create(app);
    }
    if let Some(window) = app.get_webview_window(LABEL) {
        if let Some((x, y)) = position(app) {
            let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
        }
        let _ = window.show();
    }
}

#[cfg(target_os = "macos")]
fn create(app: &AppHandle) {
    use tauri_nspanel::{CollectionBehavior, PanelBuilder, PanelLevel, StyleMask};
    let (x, y) = position(app).unwrap_or((0.0, 0.0));
    let built = PanelBuilder::<_, MeetingIndicatorPanel>::new(app, LABEL)
        .url(tauri::WebviewUrl::App(
            "src/meeting-indicator/index.html".into(),
        ))
        .title("Meeting")
        .position(tauri::Position::Logical(tauri::LogicalPosition { x, y }))
        .level(PanelLevel::Status)
        .size(tauri::Size::Logical(tauri::LogicalSize {
            width: WIDTH,
            height: HEIGHT,
        }))
        .has_shadow(false)
        .transparent(true)
        .no_activate(true)
        .corner_radius(0.0)
        .style_mask(StyleMask::empty().borderless().nonactivating_panel())
        .with_window(|w| {
            w.decorations(false)
                .transparent(true)
                .focusable(false)
                .accept_first_mouse(true)
        })
        .collection_behavior(
            CollectionBehavior::new()
                .can_join_all_spaces()
                .full_screen_auxiliary(),
        )
        .build();
    if let Err(e) = built {
        log::error!("Failed to create the meeting indicator: {e}");
    }
}

#[cfg(not(target_os = "macos"))]
fn create(app: &AppHandle) {
    let (x, y) = position(app).unwrap_or((0.0, 0.0));
    let mut builder = tauri::WebviewWindowBuilder::new(
        app,
        LABEL,
        tauri::WebviewUrl::App("src/meeting-indicator/index.html".into()),
    )
    .title("Meeting")
    .position(x, y)
    .inner_size(WIDTH, HEIGHT)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .closable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .transparent(true)
    .shadow(false)
    .focused(false)
    .accept_first_mouse(true)
    .visible(false);
    if let Some(data_dir) = crate::portable::data_dir() {
        builder = builder.data_directory(data_dir.join("webview"));
    }
    if let Err(e) = builder.build() {
        log::error!("Failed to create the meeting indicator: {e}");
    }
}
