//! Send GUI mouse and keyboard input with enigo.

use std::path::Path;
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

#[cfg(not(windows))]
use enigo::Coordinate;
use enigo::{Axis, Button, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::tools::focus::resolve_window;
use crate::tools::{CANCEL_REASON, Tool};

/// Most actions accepted in one call.
const MAX_ACTIONS: usize = 200;
/// Longest single wait.
const MAX_WAIT_MS: u64 = 30_000;
/// Longest cumulative wait in one call.
const MAX_TOTAL_WAIT_MS: u64 = 120_000;
/// Longest text typed in one call.
const MAX_TEXT_CHARS: usize = 2_000;
/// Most keys held together in one chord.
const MAX_CHORD_KEYS: usize = 8;
/// Largest single scroll distance, in wheel notches.
const MAX_SCROLL: u32 = 100;
/// Cancel-token poll interval while waiting.
const CANCEL_POLL: Duration = Duration::from_millis(25);
/// Pause after moving so hover effects settle before a click or drag press.
const MOVE_SETTLE: Duration = Duration::from_millis(10);
/// Gap between the two clicks of a double-click.
const DOUBLE_CLICK_GAP: Duration = Duration::from_millis(40);
/// Gesture time before the travel-dependent term.
const DRAG_BASE_MS: f64 = 120.0;
/// Extra gesture time per pixel of travel.
const DRAG_MS_PER_PX: f64 = 0.6;
/// Shortest drag gesture; a faster one reads as a click.
const DRAG_MIN_MS: f64 = 180.0;
/// Longest drag gesture, so a long drag stays responsive.
const DRAG_MAX_MS: f64 = 900.0;
/// Target pause between interpolated drag steps.
const DRAG_STEP_MS: u64 = 8;
/// Most interpolated steps one drag may use, bounding long gestures.
const DRAG_MAX_STEPS: u64 = 200;
/// Error when neither a target window nor an action is supplied.
const MISSING_TARGET: &str = "Provide at least one action or a `window_id`.";
/// Top-level `send_input` arguments.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    window_id: Option<Value>,
    actions: Option<Value>,
}

/// One action's parser keys and schema metadata.
struct ActionSpec {
    kind: &'static str,
    description: &'static str,
    required: &'static [&'static str],
    optional: &'static [&'static str],
}

impl ActionSpec {
    fn fields(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.required.iter().chain(self.optional).copied()
    }

    fn accepts(&self, field: &str) -> bool {
        field == "type" || self.required.contains(&field) || self.optional.contains(&field)
    }
}

const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        kind: "click",
        description: "Move to (x, y) and left-click.",
        required: &["x", "y"],
        optional: &[],
    },
    ActionSpec {
        kind: "double_click",
        description: "Move to (x, y) and left-double-click.",
        required: &["x", "y"],
        optional: &[],
    },
    ActionSpec {
        kind: "right_click",
        description: "Move to (x, y) and right-click.",
        required: &["x", "y"],
        optional: &[],
    },
    ActionSpec {
        kind: "move_to",
        description: "Move the pointer to (x, y).",
        required: &["x", "y"],
        optional: &[],
    },
    ActionSpec {
        kind: "scroll",
        description: "Move to (x, y), then scroll: +delta_y = down, +delta_x = right.",
        required: &["x", "y", "delta_y"],
        optional: &["delta_x"],
    },
    ActionSpec {
        kind: "drag",
        description: "Press a mouse button at (from_x, from_y), drag to (to_x, to_y), and release.",
        required: &["from_x", "from_y", "to_x", "to_y"],
        optional: &["button"],
    },
    ActionSpec {
        kind: "type_text",
        description: "Type text into the focused control; newlines and tabs become Enter and Tab.",
        required: &["text"],
        optional: &[],
    },
    ActionSpec {
        kind: "press_key",
        description: "Press keys as one chord and release them in reverse order, e.g. [\"CTRL\", \"O\"].",
        required: &["keys"],
        optional: &[],
    },
    ActionSpec {
        kind: "wait",
        description: "Pause before the next action.",
        required: &["duration_ms"],
        optional: &[],
    },
];

/// Serializes GUI input across session tabs and sub-agents.
static INPUT_LOCK: Mutex<()> = Mutex::new(());

pub struct SendInputTool;

impl Tool for SendInputTool {
    fn name(&self) -> &str {
        "send_input"
    }

    fn description(&self) -> &str {
        "Drive the desktop with mouse and keyboard actions in one call: click, double_click, right_click, move_to, scroll, drag, type_text, press_key, and wait. Actions run in order; coordinates are absolute screen pixels, so take a screenshot first. `window_id` (from the `screenshot` window list or a `process` `wait_window` report) is brought to the foreground first, and `actions` may then be empty; screenshot after focusing when the window may have moved. `press_key` takes a chord such as [\"CTRL\", \"O\"]; `type_text` types into the focused control."
    }

    fn schema(&self) -> Value {
        build_schema()
    }

    fn execute_inner(
        &self,
        args: &Value,
        _workspace: &Path,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        let Prepared { window_id, actions } = prepare(args)?;
        let _guard = lock_input(cancel)?;

        // Resolve first so a missing window never initializes input permissions.
        let target = window_id.map(|id| resolve_window(id, cancel)).transpose()?;

        if actions.is_empty() {
            let Some(target) = target else {
                return Err(MISSING_TARGET.into());
            };
            return Ok(target.focus(cancel)?.line);
        }

        let mut enigo = Enigo::new(&Settings::default()).map_err(|error| {
            format!(
                "Failed to initialize GUI input: {error}. macOS requires Accessibility permission; \
                 Linux requires an X11 or XWayland display."
            )
        })?;

        // Focus before actions so typing and shortcuts reach the window.
        let focused = target
            .as_ref()
            .map(|target| target.focus(cancel))
            .transpose()?;
        perform_actions(&actions, &mut enigo, cancel)?;

        let mut report = Vec::new();
        if let Some(focused) = focused.as_ref()
            && focused.restored
        {
            report.push(focused.line.clone());
        }
        report.push(format!(
            "Executed {} successfully.",
            count_actions(actions.len())
        ));
        Ok(report.join("\n"))
    }
}

/// Run actions in order, stopping at the first failure or cancel.
fn perform_actions(
    actions: &[Action],
    enigo: &mut Enigo,
    cancel: &CancellationToken,
) -> Result<(), String> {
    for (index, action) in actions.iter().enumerate() {
        let result = if cancel.is_cancelled() {
            Err(CANCEL_REASON.into())
        } else {
            action.perform(enigo, cancel)
        };
        if let Err(error) = result {
            // A cancel is a stop, not an action failure.
            let report = if cancel.is_cancelled() {
                cancelled(index, action.kind(), &error)
            } else {
                failure(index, action.kind(), &error)
            };
            return Err(format!("{report}\n{}", remaining(index)));
        }
        tracing::debug!(action = %action.summary(), "send_input action");
    }
    Ok(())
}

/// The `actions` array as a tagged union; each variant declares only its fields.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Action {
    Click {
        x: i32,
        y: i32,
    },
    DoubleClick {
        x: i32,
        y: i32,
    },
    RightClick {
        x: i32,
        y: i32,
    },
    MoveTo {
        x: i32,
        y: i32,
    },
    Scroll {
        x: i32,
        y: i32,
        delta_y: i32,
        #[serde(default, deserialize_with = "nullable_default")]
        delta_x: i32,
    },
    Drag {
        from_x: i32,
        from_y: i32,
        to_x: i32,
        to_y: i32,
        #[serde(default, deserialize_with = "nullable_default")]
        button: MouseButton,
    },
    TypeText {
        text: String,
    },
    PressKey {
        #[serde(deserialize_with = "deserialize_keys")]
        keys: Vec<Key>,
    },
    Wait {
        duration_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum MouseButton {
    #[default]
    Left,
    Middle,
    Right,
}

impl MouseButton {
    fn label(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Middle => "middle",
            Self::Right => "right",
        }
    }

    fn enigo(self) -> Button {
        match self {
            Self::Left => Button::Left,
            Self::Middle => Button::Middle,
            Self::Right => Button::Right,
        }
    }
}

/// Accept `null` for a field that defaults when omitted.
fn nullable_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// Parse `["CTRL", "O"]` into enigo keys during deserialization.
fn deserialize_keys<'de, D>(deserializer: D) -> Result<Vec<Key>, D::Error>
where
    D: Deserializer<'de>,
{
    let names = Vec::<String>::deserialize(deserializer)?;
    if names.is_empty() {
        return Err(serde::de::Error::custom(
            "press_key requires at least one key.",
        ));
    }
    if names.len() > MAX_CHORD_KEYS {
        return Err(serde::de::Error::custom(format!(
            "press_key accepts at most {MAX_CHORD_KEYS} keys per chord."
        )));
    }
    names
        .iter()
        .map(|name| parse_key(name).map_err(serde::de::Error::custom))
        .collect()
}

impl Action {
    /// Schema `type` of this action, without its parameters.
    fn kind(&self) -> &'static str {
        match self {
            Self::Click { .. } => "click",
            Self::DoubleClick { .. } => "double_click",
            Self::RightClick { .. } => "right_click",
            Self::MoveTo { .. } => "move_to",
            Self::Scroll { .. } => "scroll",
            Self::Drag { .. } => "drag",
            Self::TypeText { .. } => "type_text",
            Self::PressKey { .. } => "press_key",
            Self::Wait { .. } => "wait",
        }
    }

    /// Validate this action; waits return their duration so callers can sum them.
    fn validate(&self) -> Result<Option<Duration>, String> {
        match self {
            Self::Scroll {
                delta_y, delta_x, ..
            } => {
                if *delta_y == 0 && *delta_x == 0 {
                    return Err("scroll requires a non-zero delta_y or delta_x.".into());
                }
                if delta_y.unsigned_abs() > MAX_SCROLL || delta_x.unsigned_abs() > MAX_SCROLL {
                    return Err(format!(
                        "scroll deltas are limited to ±{MAX_SCROLL} wheel notches."
                    ));
                }
                Ok(None)
            }
            Self::TypeText { text } => {
                if text.contains('\0') {
                    return Err("type_text cannot contain NUL characters.".into());
                }
                let chars = text.chars().count();
                if chars > MAX_TEXT_CHARS {
                    return Err(format!(
                        "type_text has {chars} characters; the limit is {MAX_TEXT_CHARS}."
                    ));
                }
                Ok(None)
            }
            Self::Wait { duration_ms } => {
                if *duration_ms > MAX_WAIT_MS {
                    return Err(format!("wait duration_ms is limited to {MAX_WAIT_MS}."));
                }
                Ok(Some(Duration::from_millis(*duration_ms)))
            }
            _ => Ok(None),
        }
    }

    fn summary(&self) -> String {
        match self {
            Self::Click { x, y } => format!("click ({x}, {y})"),
            Self::DoubleClick { x, y } => format!("double_click ({x}, {y})"),
            Self::RightClick { x, y } => format!("right_click ({x}, {y})"),
            Self::MoveTo { x, y } => format!("move_to ({x}, {y})"),
            Self::Scroll {
                x,
                y,
                delta_y,
                delta_x,
            } => format!("scroll ({x}, {y}) dy={delta_y} dx={delta_x}"),
            Self::Drag {
                from_x,
                from_y,
                to_x,
                to_y,
                button,
            } => format!(
                "drag {} ({from_x}, {from_y}) -> ({to_x}, {to_y})",
                button.label()
            ),
            Self::TypeText { text } => format!("type_text ({} chars)", text.chars().count()),
            Self::PressKey { keys } => {
                let names: Vec<String> = keys.iter().map(|key| format!("{key:?}")).collect();
                format!("press_key [{}]", names.join(", "))
            }
            Self::Wait { duration_ms } => format!("wait {duration_ms} ms"),
        }
    }

    fn perform(&self, enigo: &mut Enigo, cancel: &CancellationToken) -> Result<(), String> {
        match self {
            Self::Click { x, y } => click(enigo, *x, *y, Button::Left, cancel),
            Self::DoubleClick { x, y } => double_click(enigo, *x, *y, cancel),
            Self::RightClick { x, y } => click(enigo, *x, *y, Button::Right, cancel),
            Self::MoveTo { x, y } => move_to(enigo, *x, *y),
            Self::Scroll {
                x,
                y,
                delta_y,
                delta_x,
            } => {
                move_to(enigo, *x, *y)?;
                sleep_or_cancel(MOVE_SETTLE, cancel)?;
                scroll(enigo, *delta_y, Axis::Vertical)?;
                scroll(enigo, *delta_x, Axis::Horizontal)
            }
            Self::Drag {
                from_x,
                from_y,
                to_x,
                to_y,
                button,
            } => drag(enigo, (*from_x, *from_y), (*to_x, *to_y), *button, cancel),
            Self::TypeText { text } => type_text(enigo, text, cancel),
            Self::PressKey { keys } => press_chord(enigo, keys),
            Self::Wait { duration_ms } => {
                sleep_or_cancel(Duration::from_millis(*duration_ms), cancel)
            }
        }
    }
}

/// One validated call: the window to focus and the actions to run.
#[derive(Debug)]
struct Prepared {
    window_id: Option<u32>,
    actions: Vec<Action>,
}

/// Parse and validate the arguments before any input runs.
fn prepare(args: &Value) -> Result<Prepared, String> {
    let args = Args::deserialize(args)
        .map_err(|error| format!("Invalid send_input arguments: {error}"))?;
    let window_id = parse_window_id(args.window_id.as_ref())?;
    let actions = parse_actions(args.actions.as_ref())?;
    if actions.is_empty() && window_id.is_none() {
        return Err(MISSING_TARGET.into());
    }
    if actions.len() > MAX_ACTIONS {
        return Err(format!(
            "`actions` has {} items; the limit is {MAX_ACTIONS}.",
            actions.len()
        ));
    }

    let mut total_wait = Duration::ZERO;
    for action in &actions {
        if let Some(wait) = action.validate()? {
            total_wait += wait;
            if total_wait > Duration::from_millis(MAX_TOTAL_WAIT_MS) {
                return Err(format!(
                    "Total wait exceeds the {MAX_TOTAL_WAIT_MS} ms limit."
                ));
            }
        }
    }
    Ok(Prepared { window_id, actions })
}

/// Parse the `actions` array; absent or `null` means a focus-only call.
fn parse_actions(raw: Option<&Value>) -> Result<Vec<Action>, String> {
    let Some(raw) = raw.filter(|raw| !raw.is_null()) else {
        return Ok(Vec::new());
    };
    let raw = match raw {
        Value::String(text) => {
            serde_json::from_str(text).map_err(|error| format!("Invalid actions: {error}"))?
        }
        _ => raw.clone(),
    };
    let items = raw
        .as_array()
        .ok_or_else(|| "Invalid actions: expected an array.".to_string())?;
    for (index, item) in items.iter().enumerate() {
        validate_action_fields(index, item)?;
    }
    serde_json::from_value(raw).map_err(|error| format!("Invalid actions: {error}"))
}

/// Reject unknown action keys before serde can ignore them.
fn validate_action_fields(index: usize, value: &Value) -> Result<(), String> {
    let action = index + 1;
    let object = value
        .as_object()
        .ok_or_else(|| format!("Action {action} must be a JSON object."))?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Action {action} requires a string `type`."))?;
    let spec = ACTIONS
        .iter()
        .find(|spec| spec.kind == kind)
        .ok_or_else(|| format!("Action {action} has unknown type `{kind}`."))?;
    if let Some(key) = object.keys().find(|key| !spec.accepts(key)) {
        let accepted = quoted_fields(std::iter::once("type").chain(spec.fields()));
        return Err(format!(
            "Unknown field `{key}` in action {action} ({kind}); accepted fields are {accepted}."
        ));
    }
    Ok(())
}

fn quoted_fields<'a>(fields: impl IntoIterator<Item = &'a str>) -> String {
    fields
        .into_iter()
        .map(|field| format!("`{field}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parse the optional window id; models often send the id as a string.
fn parse_window_id(raw: Option<&Value>) -> Result<Option<u32>, String> {
    let Some(raw) = raw.filter(|raw| !raw.is_null()) else {
        return Ok(None);
    };
    let parsed = match raw {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    };
    let id = parsed
        .and_then(|id| u32::try_from(id).ok())
        .filter(|id| *id != 0)
        .ok_or_else(|| {
            format!(
                "Invalid window_id {raw}: use the numeric window id listed by `screenshot` or \
                 `process` `wait_window`."
            )
        })?;
    Ok(Some(id))
}

/// Key names listed when `press_key` gets an unknown key.
#[cfg(not(target_os = "macos"))]
const KEY_NAMES: &str = "CTRL, SHIFT, ALT, META, ENTER, ESC, TAB, SPACE, BACKSPACE, DELETE, \
                         INSERT, HOME, END, PAGEUP, PAGEDOWN, UP, DOWN, LEFT, or RIGHT";
/// Key names listed when `press_key` gets an unknown key; macOS has no Insert.
#[cfg(target_os = "macos")]
const KEY_NAMES: &str = "CTRL, SHIFT, ALT, META, ENTER, ESC, TAB, SPACE, BACKSPACE, DELETE, \
                         HOME, END, PAGEUP, PAGEDOWN, UP, DOWN, LEFT, or RIGHT";

/// Parse one key name or single character.
fn parse_key(raw: &str) -> Result<Key, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("press_key contains an empty key name.".into());
    }
    let upper = name.to_ascii_uppercase();
    let key = match upper.as_str() {
        "CTRL" | "CONTROL" => Key::Control,
        "SHIFT" => Key::Shift,
        "ALT" | "OPTION" => Key::Alt,
        "META" | "CMD" | "COMMAND" | "SUPER" | "WIN" | "WINDOWS" | "GUI" => Key::Meta,
        "ENTER" | "RETURN" => Key::Return,
        "ESC" | "ESCAPE" => Key::Escape,
        "TAB" => Key::Tab,
        "SPACE" => Key::Space,
        "BACKSPACE" | "BACK" => Key::Backspace,
        "DELETE" | "DEL" => Key::Delete,
        "HOME" => Key::Home,
        "END" => Key::End,
        "PAGEUP" | "PAGE_UP" | "PGUP" => Key::PageUp,
        "PAGEDOWN" | "PAGE_DOWN" | "PGDN" => Key::PageDown,
        "UP" | "UPARROW" | "ARROWUP" => Key::UpArrow,
        "DOWN" | "DOWNARROW" | "ARROWDOWN" => Key::DownArrow,
        "LEFT" | "LEFTARROW" | "ARROWLEFT" => Key::LeftArrow,
        "RIGHT" | "RIGHTARROW" | "ARROWRIGHT" => Key::RightArrow,
        "CAPSLOCK" | "CAPS" => Key::CapsLock,
        "INSERT" | "INS" => insert_key()?,
        _ => {
            if let Some(key) = function_key(&upper) {
                key
            } else if let Some(ch) = single_char(name) {
                character_key(ch)
            } else {
                return Err(format!(
                    "Unknown key \"{raw}\". Use a single character, F1-F20, or a name such as \
                     {KEY_NAMES}."
                ));
            }
        }
    };
    Ok(key)
}

/// The Insert key, which enigo provides on Windows and Linux only.
#[cfg(not(target_os = "macos"))]
fn insert_key() -> Result<Key, String> {
    Ok(Key::Insert)
}

/// Report Insert as unsupported instead of as an unknown name.
#[cfg(target_os = "macos")]
fn insert_key() -> Result<Key, String> {
    Err("INSERT is not available on macOS; Mac keyboards have no Insert key.".into())
}

fn function_key(name: &str) -> Option<Key> {
    const F_KEYS: [Key; 20] = [
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
        Key::F13,
        Key::F14,
        Key::F15,
        Key::F16,
        Key::F17,
        Key::F18,
        Key::F19,
        Key::F20,
    ];
    let number: usize = name.strip_prefix('F')?.parse().ok()?;
    F_KEYS.get(number.checked_sub(1)?).copied()
}

fn single_char(name: &str) -> Option<char> {
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Some(ch),
        _ => None,
    }
}

/// Letters are lowercased so `["CTRL", "O"]` reads as a shortcut, not Shift+O.
fn character_key(ch: char) -> Key {
    match ch {
        '\n' | '\r' => Key::Return,
        '\t' => Key::Tab,
        ' ' => Key::Space,
        _ if ch.is_ascii_alphabetic() => Key::Unicode(ch.to_ascii_lowercase()),
        _ => Key::Unicode(ch),
    }
}

/// Move the pointer to absolute screen coordinates.
#[cfg(not(windows))]
fn move_to(enigo: &mut Enigo, x: i32, y: i32) -> Result<(), String> {
    enigo
        .move_mouse(x, y, Coordinate::Abs)
        .map_err(|error| format!("move to ({x}, {y}) failed: {error}"))
}

/// Move across the virtual desktop; enigo's Windows move only handles the
/// primary monitor.
#[cfg(windows)]
fn move_to(_enigo: &mut Enigo, x: i32, y: i32) -> Result<(), String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, mouse_event,
    };

    let (origin_x, origin_y, width, height) = virtual_screen()?;
    let inside = x >= origin_x && y >= origin_y && x < origin_x + width && y < origin_y + height;
    if !inside {
        return Err(format!(
            "move to ({x}, {y}) failed: outside the virtual screen \
             ({origin_x}, {origin_y}) {width}x{height}"
        ));
    }
    // SAFETY: mouse_event only injects a validated pointer move.
    unsafe {
        mouse_event(
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
            normalize_axis(x, origin_x, width),
            normalize_axis(y, origin_y, height),
            0,
            0,
        );
    }
    Ok(())
}

/// The virtual desktop rect in physical screen pixels, covering every monitor.
#[cfg(windows)]
fn virtual_screen() -> Result<(i32, i32, i32, i32), String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    // SAFETY: `GetSystemMetrics` only reads display metrics.
    let (x, y, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    if width <= 1 || height <= 1 {
        return Err("The display reports no usable virtual screen.".into());
    }
    Ok((x, y, width, height))
}

/// Scale one axis into the 0-65535 absolute space Windows expects.
#[cfg(windows)]
fn normalize_axis(value: i32, origin: i32, extent: i32) -> i32 {
    let span = (i64::from(extent) - 1).max(1);
    let offset = i64::from(value) - i64::from(origin);
    ((offset * 65_535 + span / 2) / span) as i32
}

fn click(
    enigo: &mut Enigo,
    x: i32,
    y: i32,
    button: Button,
    cancel: &CancellationToken,
) -> Result<(), String> {
    move_to(enigo, x, y)?;
    sleep_or_cancel(MOVE_SETTLE, cancel)?;
    click_button(enigo, button)
}

fn double_click(
    enigo: &mut Enigo,
    x: i32,
    y: i32,
    cancel: &CancellationToken,
) -> Result<(), String> {
    click(enigo, x, y, Button::Left, cancel)?;
    sleep_or_cancel(DOUBLE_CLICK_GAP, cancel)?;
    click_button(enigo, Button::Left)
}

fn click_button(enigo: &mut Enigo, button: Button) -> Result<(), String> {
    enigo
        .button(button, Direction::Click)
        .map_err(|error| format!("mouse click failed: {error}"))
}

fn scroll(enigo: &mut Enigo, length: i32, axis: Axis) -> Result<(), String> {
    if length == 0 {
        return Ok(());
    }
    enigo
        .scroll(length, axis)
        .map_err(|error| format!("scroll failed: {error}"))
}

fn drag(
    enigo: &mut Enigo,
    from: (i32, i32),
    to: (i32, i32),
    button: MouseButton,
    cancel: &CancellationToken,
) -> Result<(), String> {
    let (from_x, from_y) = from;
    let (to_x, to_y) = to;
    move_to(enigo, from_x, from_y)?;
    sleep_or_cancel(MOVE_SETTLE, cancel)?;
    let button = button.enigo();
    enigo
        .button(button, Direction::Press)
        .map_err(|error| format!("drag press at ({from_x}, {from_y}) failed: {error}"))?;

    // Release even when the move is cancelled or fails, so the button is not left down.
    let moved = drag_steps(enigo, from, to, cancel);
    let released = enigo
        .button(button, Direction::Release)
        .map_err(|error| format!("drag release at ({to_x}, {to_y}) failed: {error}"));
    match (moved, released) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(moved), Ok(())) => Err(moved),
        (Ok(()), Err(released)) => Err(released),
        (Err(moved), Err(released)) => Err(format!("{moved}; {released}")),
    }
}

/// Move in small increments so drag-aware applications see motion events.
fn drag_steps(
    enigo: &mut Enigo,
    from: (i32, i32),
    to: (i32, i32),
    cancel: &CancellationToken,
) -> Result<(), String> {
    let (from_x, from_y) = from;
    let (to_x, to_y) = to;
    let dx = i64::from(to_x) - i64::from(from_x);
    let dy = i64::from(to_y) - i64::from(from_y);
    let (steps, duration) = drag_timing(dx.abs().max(dy.abs()));
    let start = Instant::now();
    for step in 1..=steps {
        if cancel.is_cancelled() {
            return Err(CANCEL_REASON.into());
        }
        let x = (i64::from(from_x) + dx * step / steps) as i32;
        let y = (i64::from(from_y) + dy * step / steps) as i32;
        move_to(enigo, x, y)?;
        if step < steps {
            // Pace from the gesture start so a slow step never stretches the drag.
            let deadline = start + duration * step as u32 / steps as u32;
            let remaining = deadline.saturating_duration_since(Instant::now());
            sleep_or_cancel(remaining, cancel)?;
        }
    }
    Ok(())
}

/// Human-ish gesture pacing: a distance-scaled duration split into small steps.
fn drag_timing(distance: i64) -> (i64, Duration) {
    let ms = DRAG_BASE_MS + DRAG_MS_PER_PX * distance as f64;
    let ms = ms.clamp(DRAG_MIN_MS, DRAG_MAX_MS).round() as u64;
    let steps = (ms / DRAG_STEP_MS).clamp(1, DRAG_MAX_STEPS) as i64;
    (steps, Duration::from_millis(ms))
}

fn type_text(enigo: &mut Enigo, text: &str, cancel: &CancellationToken) -> Result<(), String> {
    let mut chunk = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if cancel.is_cancelled() {
            return Err(CANCEL_REASON.into());
        }
        match ch {
            '\r' | '\n' => {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                flush_text(enigo, &mut chunk)?;
                press_once(enigo, Key::Return, "Return")?;
            }
            '\t' => {
                flush_text(enigo, &mut chunk)?;
                press_once(enigo, Key::Tab, "Tab")?;
            }
            _ => {
                chunk.push(ch);
                if chunk.len() >= 512 {
                    flush_text(enigo, &mut chunk)?;
                }
            }
        }
    }
    flush_text(enigo, &mut chunk)
}

fn flush_text(enigo: &mut Enigo, chunk: &mut String) -> Result<(), String> {
    if chunk.is_empty() {
        return Ok(());
    }
    let text = std::mem::take(chunk);
    enigo
        .text(&text)
        .map_err(|error| format!("typing text failed: {error}"))
}

fn press_once(enigo: &mut Enigo, key: Key, name: &str) -> Result<(), String> {
    enigo
        .key(key, Direction::Click)
        .map_err(|error| format!("typing {name} failed: {error}"))
}

fn press_chord(enigo: &mut Enigo, keys: &[Key]) -> Result<(), String> {
    for (pressed, key) in keys.iter().enumerate() {
        if let Err(error) = enigo.key(*key, Direction::Press) {
            let release = release_keys(enigo, &keys[..pressed]);
            return Err(match release {
                Ok(()) => format!("pressing {key:?} failed: {error}"),
                Err(release_error) => {
                    format!("pressing {key:?} failed: {error}; {release_error}")
                }
            });
        }
    }
    release_keys(enigo, keys)
}

fn release_keys(enigo: &mut Enigo, keys: &[Key]) -> Result<(), String> {
    let mut first_error = None;
    for key in keys.iter().rev() {
        if let Err(error) = enigo.key(*key, Direction::Release)
            && first_error.is_none()
        {
            first_error = Some(format!("releasing {key:?} failed: {error}"));
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn sleep_or_cancel(duration: Duration, cancel: &CancellationToken) -> Result<(), String> {
    let deadline = Instant::now() + duration;
    loop {
        if cancel.is_cancelled() {
            return Err(CANCEL_REASON.into());
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(());
        };
        std::thread::sleep(remaining.min(CANCEL_POLL));
    }
}

/// Hold the process-wide GUI input lock, polling the cancel token while blocked.
fn lock_input(cancel: &CancellationToken) -> Result<MutexGuard<'static, ()>, String> {
    loop {
        if cancel.is_cancelled() {
            return Err(CANCEL_REASON.into());
        }
        match INPUT_LOCK.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(poisoned)) => return Ok(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => std::thread::sleep(CANCEL_POLL),
        }
    }
}

/// Failure report for the action at `index` (0-based).
fn failure(index: usize, kind: &str, error: &str) -> String {
    format!(
        "Failed at action {} ({kind}): {}.",
        index + 1,
        error.trim_end_matches('.')
    )
}

/// A user cancel reads as a stop, not an action failure.
fn cancelled(index: usize, kind: &str, error: &str) -> String {
    // Keep a secondary failure (e.g. a drag release) without repeating the stop reason.
    let detail = error
        .strip_prefix(CANCEL_REASON)
        .unwrap_or(error)
        .trim_start_matches([' ', ';', ':'])
        .trim_end_matches('.');
    let detail = if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    };
    format!("{CANCEL_REASON} at action {} ({kind}){detail}.", index + 1)
}

/// The untaken tail of a run that stopped after `completed` actions.
fn remaining(completed: usize) -> String {
    format!(
        "{} completed; remaining actions were not executed.",
        count_actions(completed)
    )
}

/// "1 action" / "4 actions".
fn count_actions(count: usize) -> String {
    if count == 1 {
        "1 action".to_string()
    } else {
        format!("{count} actions")
    }
}

fn build_schema() -> Value {
    let actions: Vec<Value> = ACTIONS.iter().map(action_schema).collect();
    json!({
        "type": "object",
        "properties": {
            "window_id": {
                "type": "integer",
                "description": "Window to bring to the foreground before the actions, from the `screenshot` window list or a `process` `wait_window` report. Accepts a number or a numeric string."
            },
            "actions": {
                "type": "array",
                "description": "Ordered GUI actions. Each object has a `type` plus only that action's fields; may be empty when `window_id` is given.",
                "maxItems": MAX_ACTIONS,
                "items": {"anyOf": actions}
            }
        },
        "additionalProperties": false
    })
}

/// One tagged-union branch: a `type` discriminator plus the action's fields.
fn action_schema(spec: &ActionSpec) -> Value {
    let mut properties: Map<String, Value> = spec
        .fields()
        .map(|field| (field.to_string(), field_schema(field)))
        .collect();
    properties.insert(
        "type".into(),
        json!({"const": spec.kind, "description": "Action kind."}),
    );
    let required: Vec<&str> = std::iter::once("type")
        .chain(spec.required.iter().copied())
        .collect();
    json!({
        "type": "object",
        "description": spec.description,
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn field_schema(field: &str) -> Value {
    match field {
        "x" | "from_x" | "to_x" => integer("Absolute screen x coordinate in pixels."),
        "y" | "from_y" | "to_y" => integer("Absolute screen y coordinate in pixels."),
        "delta_y" => integer("Vertical notches; positive scrolls down."),
        "delta_x" => json!({
            "type": "integer",
            "default": 0,
            "description": "Horizontal notches; positive scrolls right. Defaults to 0."
        }),
        "button" => json!({
            "type": "string",
            "enum": ["left", "right", "middle"],
            "default": "left",
            "description": "Mouse button to drag with. Defaults to left."
        }),
        "text" => json!({
            "type": "string",
            "description": "Text to type, including newlines and tabs."
        }),
        "keys" => json!({
            "type": "array",
            "description": "Chord keys: CTRL, SHIFT, ALT, META, ENTER, ESC, TAB, F1-F20, or one character.",
            "minItems": 1,
            "maxItems": MAX_CHORD_KEYS,
            "items": {"type": "string"}
        }),
        "duration_ms" => json!({
            "type": "integer",
            "minimum": 0,
            "maximum": MAX_WAIT_MS,
            "description": "Milliseconds to wait."
        }),
        _ => unreachable!("missing schema for action field `{field}`"),
    }
}

fn integer(description: &str) -> Value {
    json!({"type": "integer", "description": description})
}
