//! Screenshot tool: capture a monitor or window with XCap.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use image::RgbaImage;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use xcap::{Monitor, Window};

use crate::tools::{
    CANCEL_REASON, ImageAttachment, ImageBudget, Tool, current_tab_number, make_workspace_relative,
    prepare_for_request, route_images, tool_limits,
};

/// Workspace-relative folder holding captured screenshots.
const SCREENSHOT_DIR: &str = ".agent/screenshots";
/// Windows shown in one listing before the rest are summarized.
const MAX_LISTED_WINDOWS: usize = 40;
/// Matching windows named in an ambiguity error.
const MAX_CANDIDATES: usize = 10;
/// Longest target name embedded in a screenshot filename.
const MAX_LABEL_CHARS: usize = 32;
/// Longest one list/capture worker may run before the tool gives up.
const WORKER_TIMEOUT: Duration = Duration::from_secs(30);
/// Cancel-token poll interval while waiting for the worker.
const WORKER_POLL: Duration = Duration::from_millis(100);

pub struct ScreenshotTool;

impl Tool for ScreenshotTool {
    fn name(&self) -> &str {
        "screenshot"
    }

    fn description(&self) -> &str {
        "Capture a screenshot of a monitor or window and attach it to the conversation."
    }

    fn instruction(&self) -> &str {
        "Capture a monitor or window as a picture attached to the conversation. Omit both targets to list available monitors and windows. Otherwise, specify `monitor` (0-based index, name substring, or \"primary\") or `window` (title, app name, or ID). Screenshots are saved to `.agent/screenshots/` and automatically downscaled."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "monitor": {
                    "type": "string",
                    "description": "0-based monitor index, name substring, or \"primary\"."
                },
                "window": {
                    "type": "string",
                    "description": "Window title, title substring, app name, or numeric id."
                }
            }
        })
    }

    fn execute_inner(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        run(parse_target(args)?, workspace, cancel).map(|(text, _)| text)
    }

    fn execute_with_attachments(
        &self,
        args: &Value,
        workspace: &Path,
        cancel: &CancellationToken,
    ) -> (Result<String, String>, Vec<ImageAttachment>) {
        if cancel.is_cancelled() {
            return (Err(CANCEL_REASON.into()), Vec::new());
        }
        let target = match parse_target(args) {
            Ok(target) => target,
            Err(error) => return (Err(error), Vec::new()),
        };
        if !matches!(&target, Target::List)
            && let Some(reason) = vision_refusal()
        {
            return (Err(reason), Vec::new());
        }
        match run(target, workspace, cancel) {
            Ok((text, images)) => (Ok(text), images),
            Err(error) => (Err(error), Vec::new()),
        }
    }
}

/// What one call asked for: a target listing or one capture target.
enum Target {
    List,
    Monitor(String),
    Window(String),
}

/// A monitor's facts, used to select and describe it without the XCap handle.
struct MonitorFacts {
    name: String,
    friendly: String,
    width: u32,
    height: u32,
    position: Option<(i32, i32)>,
    scale: Option<f32>,
    primary: bool,
}

/// A window's facts, used to select and describe it without the XCap handle.
struct WindowFacts {
    id: u32,
    title: String,
    app: String,
    width: u32,
    height: u32,
    position: Option<(i32, i32)>,
    minimized: bool,
}

/// One captured picture plus its description and filename label.
struct Capture {
    image: RgbaImage,
    target: String,
    label: String,
}

/// Run a screenshot call: list targets, or capture one and attach it.
fn run(
    target: Target,
    workspace: &Path,
    cancel: &CancellationToken,
) -> Result<(String, Vec<ImageAttachment>), String> {
    match target {
        Target::List => {
            let worker_cancel = cancel.clone();
            await_worker(
                move || list_targets(&worker_cancel),
                cancel,
                "Screenshot listing",
            )
            .map(|text| (text, Vec::new()))
        }
        Target::Monitor(query) => {
            let worker_cancel = cancel.clone();
            let capture = await_worker(
                move || capture_monitor(&query, &worker_cancel),
                cancel,
                "Screenshot capture",
            )?;
            save_capture(capture, workspace, cancel)
        }
        Target::Window(query) => {
            let worker_cancel = cancel.clone();
            let capture = await_worker(
                move || capture_window(&query, &worker_cancel),
                cancel,
                "Screenshot capture",
            )?;
            save_capture(capture, workspace, cancel)
        }
    }
}

/// Run one blocking list/capture task on a worker thread, returning early on
/// cancel or timeout; a timed-out task is detached and cannot be interrupted.
fn await_worker<T: Send + 'static>(
    task: impl FnOnce() -> Result<T, String> + Send + 'static,
    cancel: &CancellationToken,
    what: &str,
) -> Result<T, String> {
    if cancel.is_cancelled() {
        return Err(CANCEL_REASON.into());
    }
    let (sender, receiver) = mpsc::channel();
    let _worker = std::thread::Builder::new()
        .name("screenshot".into())
        .spawn(move || {
            let _ = sender.send(task());
        })
        .map_err(|error| format!("{what} failed: could not start the worker: {error}"))?;

    let deadline = Instant::now() + WORKER_TIMEOUT;
    loop {
        if cancel.is_cancelled() {
            return Err(CANCEL_REASON.into());
        }
        match receiver.recv_timeout(WORKER_POLL) {
            Ok(result) => return result,
            Err(RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                tracing::warn!("{what} timed out after {}s", WORKER_TIMEOUT.as_secs());
                return Err(format!(
                    "{what} timed out after {}s; the display may be locked or unresponsive",
                    WORKER_TIMEOUT.as_secs()
                ));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(format!("{what} failed: the worker panicked"));
            }
        }
    }
}

fn parse_target(args: &Value) -> Result<Target, String> {
    match (arg_text(args, "window"), arg_text(args, "monitor")) {
        (Some(_), Some(_)) => Err("Pass either 'window' or 'monitor', not both.".into()),
        (Some(query), None) => Ok(Target::Window(query)),
        (None, Some(query)) => Ok(Target::Monitor(query)),
        (None, None) => Ok(Target::List),
    }
}

/// An optional string target; numbers are accepted for window ids.
fn arg_text(args: &Value, key: &str) -> Option<String> {
    let text = match args.get(key)? {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

/// Refuse a capture for a model that cannot consume pictures; listing stays allowed.
fn vision_refusal() -> Option<String> {
    let route = current_tab_number().and_then(route_images)?;
    (!route.vision).then(|| {
        format!(
            "Cannot take a screenshot: model \"{}\" does not accept image input; switch to an image-capable model",
            route.model_id
        )
    })
}

fn monitor_facts(monitor: &Monitor) -> MonitorFacts {
    let position = monitor.x().ok().zip(monitor.y().ok());
    MonitorFacts {
        name: monitor.name().unwrap_or_default(),
        friendly: monitor.friendly_name().unwrap_or_default(),
        width: monitor.width().unwrap_or_default(),
        height: monitor.height().unwrap_or_default(),
        position,
        scale: monitor.scale_factor().ok(),
        primary: monitor.is_primary().unwrap_or(false),
    }
}

fn window_facts(window: &Window) -> WindowFacts {
    let position = window.x().ok().zip(window.y().ok());
    WindowFacts {
        id: window.id().unwrap_or_default(),
        title: window.title().unwrap_or_default(),
        app: window.app_name().unwrap_or_default(),
        width: window.width().unwrap_or_default(),
        height: window.height().unwrap_or_default(),
        position,
        minimized: window.is_minimized().unwrap_or(false),
    }
}

fn monitor_name(facts: &MonitorFacts) -> &str {
    if !facts.friendly.is_empty() {
        &facts.friendly
    } else if !facts.name.is_empty() {
        &facts.name
    } else {
        "unknown monitor"
    }
}

fn window_title(facts: &WindowFacts) -> &str {
    if facts.title.is_empty() {
        &facts.app
    } else {
        &facts.title
    }
}

/// App-name suffix, omitted when the app is already shown as the title.
fn app_text(facts: &WindowFacts) -> String {
    if facts.app.is_empty() || facts.app == window_title(facts) {
        String::new()
    } else {
        format!(", app \"{}\"", facts.app)
    }
}

/// List monitors and windows for discovery when no target is given.
fn list_targets(cancel: &CancellationToken) -> Result<String, String> {
    if cancel.is_cancelled() {
        return Err(CANCEL_REASON.into());
    }
    let monitors =
        Monitor::all().map_err(|error| format!("Failed to enumerate monitors: {error}"))?;
    if cancel.is_cancelled() {
        return Err(CANCEL_REASON.into());
    }
    let monitor_facts: Vec<MonitorFacts> = monitors.iter().map(monitor_facts).collect();
    let windows = match Window::all() {
        Ok(windows) => {
            let facts: Vec<WindowFacts> = windows.iter().map(window_facts).collect();
            format_windows(&facts)
        }
        Err(error) => format!("Windows:\n  (failed to enumerate: {error})"),
    };
    Ok(format!("{}\n\n{windows}", format_monitors(&monitor_facts)))
}

fn format_monitors(facts: &[MonitorFacts]) -> String {
    if facts.is_empty() {
        return "Monitors:\n  (none found)".into();
    }
    let mut lines = vec!["Monitors:".to_string()];
    lines.extend(
        facts
            .iter()
            .enumerate()
            .map(|(index, facts)| monitor_line(index, facts)),
    );
    lines.join("\n")
}

fn monitor_line(index: usize, facts: &MonitorFacts) -> String {
    let mut line = format!(
        "  [{index}] \"{}\" {}x{}",
        monitor_name(facts),
        facts.width,
        facts.height
    );
    line.push_str(&position_text(facts.position));
    if let Some(scale) = facts.scale {
        line.push_str(&format!(", scale {scale:.2}"));
    }
    if facts.primary {
        line.push_str(", primary");
    }
    line
}

fn format_windows(facts: &[WindowFacts]) -> String {
    let listed: Vec<&WindowFacts> = facts
        .iter()
        .filter(|facts| !facts.title.is_empty() || !facts.app.is_empty())
        .collect();
    if listed.is_empty() {
        return "Windows:\n  (none found; on macOS, grant Screen Recording permission)".into();
    }
    let mut lines = vec![format!("Windows ({}):", listed.len())];
    lines.extend(
        listed
            .iter()
            .take(MAX_LISTED_WINDOWS)
            .map(|facts| window_line(facts)),
    );
    if listed.len() > MAX_LISTED_WINDOWS {
        lines.push(format!(
            "  … {} more windows",
            listed.len() - MAX_LISTED_WINDOWS
        ));
    }
    lines.join("\n")
}

fn window_line(facts: &WindowFacts) -> String {
    let mut line = format!(
        "  id {} \"{}\"{}",
        facts.id,
        window_title(facts),
        app_text(facts)
    );
    line.push_str(&position_text(facts.position));
    line.push_str(&format!(", {}x{}", facts.width, facts.height));
    if facts.minimized {
        line.push_str(", minimized");
    }
    line
}

fn capture_monitor(query: &str, cancel: &CancellationToken) -> Result<Capture, String> {
    if cancel.is_cancelled() {
        return Err(CANCEL_REASON.into());
    }
    let monitors =
        Monitor::all().map_err(|error| format!("Failed to enumerate monitors: {error}"))?;
    if monitors.is_empty() {
        return Err("No monitors found.".into());
    }
    let facts: Vec<MonitorFacts> = monitors.iter().map(monitor_facts).collect();
    let index = select_monitor(&facts, query)?;
    if cancel.is_cancelled() {
        return Err(CANCEL_REASON.into());
    }
    let selected = &facts[index];
    let image = monitors[index].capture_image().map_err(|error| {
        format!(
            "Failed to capture monitor {index} \"{}\": {error}",
            monitor_name(selected)
        )
    })?;
    check_image(&image)?;
    Ok(Capture {
        label: format!("monitor-{index}"),
        target: format!(
            "monitor {index} \"{}\"{}, {}x{}",
            monitor_name(selected),
            position_text(selected.position),
            selected.width,
            selected.height
        ),
        image,
    })
}

fn capture_window(query: &str, cancel: &CancellationToken) -> Result<Capture, String> {
    if cancel.is_cancelled() {
        return Err(CANCEL_REASON.into());
    }
    let windows = Window::all().map_err(|error| format!("Failed to enumerate windows: {error}"))?;
    let facts: Vec<WindowFacts> = windows.iter().map(window_facts).collect();
    let index = select_window(&facts, query)?;
    if cancel.is_cancelled() {
        return Err(CANCEL_REASON.into());
    }
    let selected = &facts[index];
    let image = windows[index].capture_image().map_err(|error| {
        format!(
            "Failed to capture window \"{}\": {error}",
            window_title(selected)
        )
    })?;
    check_image(&image)?;
    Ok(Capture {
        label: format!("window-{}", sanitize_label(window_title(selected))),
        target: format!(
            "window \"{}\" (id {}{}){}, {}x{}",
            window_title(selected),
            selected.id,
            app_text(selected),
            position_text(selected.position),
            selected.width,
            selected.height
        ),
        image,
    })
}

fn check_image(image: &RgbaImage) -> Result<(), String> {
    if image.width() == 0 || image.height() == 0 {
        return Err(
            "The screen capture is empty (0x0); the display may be locked or unavailable.".into(),
        );
    }
    Ok(())
}

fn position_text(position: Option<(i32, i32)>) -> String {
    position
        .map(|(x, y)| format!(" at ({x},{y})"))
        .unwrap_or_default()
}

fn select_monitor(facts: &[MonitorFacts], query: &str) -> Result<usize, String> {
    if let Ok(index) = query.parse::<usize>() {
        return if index < facts.len() {
            Ok(index)
        } else {
            Err(format!(
                "Monitor index {index} is out of range (0..={}).\n{}",
                facts.len().saturating_sub(1),
                format_monitors(facts)
            ))
        };
    }
    if query.eq_ignore_ascii_case("primary") {
        return Ok(facts.iter().position(|facts| facts.primary).unwrap_or(0));
    }
    let lower = query.to_lowercase();
    let matches: Vec<usize> = facts
        .iter()
        .enumerate()
        .filter(|(_, facts)| {
            facts.friendly.to_lowercase().contains(&lower)
                || facts.name.to_lowercase().contains(&lower)
        })
        .map(|(index, _)| index)
        .collect();
    match matches.as_slice() {
        [only] => Ok(*only),
        [] => Err(format!(
            "No monitor matched \"{query}\".\n{}",
            format_monitors(facts)
        )),
        _ => {
            let candidates: Vec<String> = matches
                .iter()
                .map(|&index| monitor_line(index, &facts[index]))
                .collect();
            Err(format!(
                "Multiple monitors match \"{query}\":\n{}\nPass the 0-based index instead.",
                candidates.join("\n")
            ))
        }
    }
}

fn select_window(facts: &[WindowFacts], query: &str) -> Result<usize, String> {
    let query = query.trim();
    let lower = query.to_lowercase();
    if let Ok(id) = query.parse::<u32>()
        && let Some(index) = facts.iter().position(|facts| facts.id == id)
    {
        return ensure_window_visible(facts, index);
    }

    // Rank: exact title, exact app, title substring, app substring.
    let rank = |facts: &WindowFacts| {
        let title = facts.title.to_lowercase();
        let app = facts.app.to_lowercase();
        if title == lower {
            Some(0)
        } else if app == lower {
            Some(1)
        } else if title.contains(&lower) {
            Some(2)
        } else if app.contains(&lower) {
            Some(3)
        } else {
            None
        }
    };
    let ranked: Vec<(u8, usize)> = facts
        .iter()
        .enumerate()
        .filter_map(|(index, facts)| rank(facts).map(|rank| (rank, index)))
        .collect();
    let Some(best) = ranked.iter().map(|(rank, _)| *rank).min() else {
        return Err(format!(
            "No window matched \"{query}\".\n{}",
            format_windows(facts)
        ));
    };
    let matches = ranked
        .into_iter()
        .filter(|(rank, _)| *rank == best)
        .map(|(_, index)| index)
        .collect();
    choose_window(facts, matches, query)
}

fn choose_window(facts: &[WindowFacts], matches: Vec<usize>, query: &str) -> Result<usize, String> {
    if let [only] = matches.as_slice() {
        return ensure_window_visible(facts, *only);
    }
    let visible: Vec<usize> = matches
        .iter()
        .copied()
        .filter(|&index| !facts[index].minimized)
        .collect();
    if let [only] = visible.as_slice() {
        return Ok(*only);
    }
    let mut listing = matches
        .iter()
        .take(MAX_CANDIDATES)
        .map(|&index| window_line(&facts[index]))
        .collect::<Vec<_>>()
        .join("\n");
    let more = matches.len().saturating_sub(MAX_CANDIDATES);
    if more > 0 {
        listing.push_str(&format!("\n  … {more} more matches"));
    }
    Err(format!(
        "Multiple windows match \"{query}\":\n{listing}\nPass a more specific title or the window id."
    ))
}

fn ensure_window_visible(facts: &[WindowFacts], index: usize) -> Result<usize, String> {
    let window = &facts[index];
    if window.minimized {
        return Err(format!(
            "Window \"{}\" (id {}) is minimized; restore it before capturing.",
            window_title(window),
            window.id
        ));
    }
    Ok(index)
}

fn save_capture(
    capture: Capture,
    workspace: &Path,
    cancel: &CancellationToken,
) -> Result<(String, Vec<ImageAttachment>), String> {
    if cancel.is_cancelled() {
        return Err(CANCEL_REASON.into());
    }
    let Capture {
        image,
        target,
        label,
    } = capture;
    let dir = workspace.join(SCREENSHOT_DIR);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Failed to create {}: {error}", dir.display()))?;
    let path = unique_path(&dir, &label);
    image
        .save(&path)
        .map_err(|error| format!("Failed to save screenshot to {}: {error}", path.display()))?;

    let display = make_workspace_relative(&path, workspace);
    let budget = current_tab_number()
        .and_then(route_images)
        .map(|route| route.budget)
        .unwrap_or_else(|| ImageBudget::from_limits(&tool_limits()));
    let prepared = prepare_for_request(&path, &display, &budget)?;
    let attachment = ImageAttachment::from_prepared(display, prepared);
    let text = format!("[Screenshot] {}\nCaptured {target}.", attachment.summary());
    Ok((text, vec![attachment]))
}

/// A timestamped, process-unique PNG path under `dir`.
fn unique_path(dir: &Path, label: &str) -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    loop {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("{stamp}-{sequence:04}-{label}.png"));
        if !path.exists() {
            return path;
        }
    }
}

/// A filename-safe, length-bounded version of a window title or app name.
fn sanitize_label(raw: &str) -> String {
    let label: String = raw
        .chars()
        .take(MAX_LABEL_CHARS)
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let label = label
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if label.is_empty() {
        "screenshot".to_string()
    } else {
        label
    }
}
