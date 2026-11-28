//! Deterministic output encoding for commands.
//!
//! PowerShell 5.1 writes its console code page, PowerShell 6+ UTF-8; native
//! Windows console tools use the OEM code page, the fallback for invalid UTF-8.
//! Command positions keep mere mentions from choosing a legacy encoding.

use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use bashkit::analysis::{ScriptAnalysis, analyze_with_limits};
use encoding_rs::{Encoding, UTF_8};

use super::{detach_child, host_path_lists, resolve_command, sanitize_child_env};

/// Maximum nested command payloads to inspect.
const MAX_PAYLOAD_DEPTH: usize = 8;
/// Parser limits matching `analyze_script`.
const PARSER_DEPTH: usize = 100;
const PARSER_FUEL: usize = 100_000;

/// Probe version and console encoding, written as UTF-8 bytes.
const PROBE: &str = r#"$s=[Console]::OpenStandardOutput();$b=[Text.Encoding]::UTF8.GetBytes("$($PSVersionTable.PSVersion.Major)`n$([Console]::OutputEncoding.WebName)`n");$s.Write($b,0,$b.Length);$s.Flush()"#;
/// Upper bound on the one-time PowerShell probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Which PowerShell family writes a command's output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PowerShell {
    /// Windows PowerShell (`powershell.exe`) writes its console code page.
    Windows,
    /// PowerShell 6+ (`pwsh`) writes UTF-8.
    Core,
}

/// Detect a script's encoding from its parse, or raw tokens when parsing failed.
pub(crate) fn script_encoding(
    script: &str,
    analysis: Option<&ScriptAnalysis>,
) -> &'static Encoding {
    let kind = match analysis {
        Some(analysis) => analyzed_powershell(analysis, MAX_PAYLOAD_DEPTH),
        None => mentioned_powershell(script),
    };
    powershell_encoding_for(kind)
}

/// Encoding for a directly spawned process or shell wrapper.
pub(crate) fn argv_encoding(program: &str, args: &[String]) -> &'static Encoding {
    host_command_encoding(program, args.iter().map(String::as_str))
}

/// Encoding for a host command and any shell payload it runs.
pub(crate) fn host_command_encoding<'a>(
    program: &str,
    args: impl IntoIterator<Item = &'a str>,
) -> &'static Encoding {
    let args: Vec<&str> = args.into_iter().collect();
    powershell_encoding_for(host_kind(program, &args, MAX_PAYLOAD_DEPTH))
}

/// UTF-8 unless the detected PowerShell writes a legacy code page.
fn powershell_encoding_for(kind: Option<PowerShell>) -> &'static Encoding {
    match kind {
        Some(PowerShell::Windows) => windows_powershell_encoding(),
        _ => UTF_8,
    }
}

/// Encoding of the hidden console a `CREATE_NO_WINDOW` child runs on, from the
/// system OEM code page. `None` when the page has no `encoding_rs` equivalent.
#[cfg(windows)]
fn console_encoding() -> Option<&'static Encoding> {
    static CACHE: OnceLock<Option<&'static Encoding>> = OnceLock::new();
    *CACHE.get_or_init(|| {
        // SAFETY: `GetOEMCP` takes no arguments and cannot fail.
        code_page_encoding(unsafe { windows_sys::Win32::Globalization::GetOEMCP() })
    })
}

#[cfg(not(windows))]
fn console_encoding() -> Option<&'static Encoding> {
    None
}

/// Legacy fallback for UTF-8 command output: native Windows console tools
/// write their localized messages in the console's OEM code page.
pub(crate) fn console_fallback(encoding: &'static Encoding) -> Option<&'static Encoding> {
    if encoding != UTF_8 {
        return None;
    }
    console_encoding().filter(|fallback| *fallback != UTF_8)
}

/// Map a Windows code page to the closest WHATWG encoding.
#[cfg(windows)]
fn code_page_encoding(code_page: u32) -> Option<&'static Encoding> {
    use encoding_rs::{BIG5, EUC_KR, GBK, IBM866, SHIFT_JIS, UTF_16BE, UTF_16LE, WINDOWS_874};

    match code_page {
        866 => Some(IBM866),
        874 => Some(WINDOWS_874),
        932 => Some(SHIFT_JIS),
        936 => Some(GBK),
        949 => Some(EUC_KR),
        950 => Some(BIG5),
        1200 => Some(UTF_16LE),
        1201 => Some(UTF_16BE),
        65001 => Some(UTF_8),
        1250..=1258 => {
            let label = format!("windows-{code_page}");
            Encoding::for_label(label.as_bytes())
        }
        _ => None,
    }
}

/// Legacy output wins when a script mixes both PowerShell families.
fn strongest(current: Option<PowerShell>, next: Option<PowerShell>) -> Option<PowerShell> {
    if current == Some(PowerShell::Windows) || next == Some(PowerShell::Windows) {
        Some(PowerShell::Windows)
    } else {
        current.or(next)
    }
}

/// Windows PowerShell when `name` is `powershell`, PowerShell Core when `pwsh`.
fn powershell_kind(name: &str) -> Option<PowerShell> {
    let stem = program_stem(name);
    if stem.eq_ignore_ascii_case("powershell") {
        Some(PowerShell::Windows)
    } else if stem.eq_ignore_ascii_case("pwsh") {
        Some(PowerShell::Core)
    } else {
        None
    }
}

/// Raw-token fallback when command positions are unavailable.
fn mentioned_powershell(text: &str) -> Option<PowerShell> {
    text.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\'' | '`' | ';' | '|' | '&' | '(' | ')' | '<' | '>'
            )
    })
    .fold(None, |found, token| {
        strongest(found, powershell_kind(token))
    })
}

/// Find PowerShell in command positions and literal eval/exec/shell payloads.
fn analyzed_powershell(analysis: &ScriptAnalysis, depth: usize) -> Option<PowerShell> {
    let mut found = None;
    for command in &analysis.commands {
        let Some(name) = command.name.as_deref() else {
            continue; // dynamic names run unknown commands — a safe miss
        };
        found = strongest(found, powershell_kind(name));
        if depth == 0 {
            continue;
        }
        let Some(args) = command.literal_args() else {
            continue; // unknown arguments cannot be shifted into command positions
        };
        if let Some(payload) = shell_payload(name, &args) {
            found = strongest(found, payload_powershell(&payload, depth - 1));
        } else if name == "eval" {
            found = strongest(found, payload_powershell(&args.join(" "), depth - 1));
        } else if name == "exec"
            && let Some((program, rest)) = args.split_first()
        {
            found = strongest(found, host_kind(program, rest, depth - 1));
        }
    }
    found
}

/// Parse a command string, falling back to raw tokens.
fn payload_powershell(text: &str, depth: usize) -> Option<PowerShell> {
    if text.is_empty() {
        return None;
    }
    match analyze_with_limits(text, PARSER_DEPTH, PARSER_FUEL) {
        Ok(analysis) => analyzed_powershell(&analysis, depth),
        Err(_) => mentioned_powershell(text),
    }
}

/// Find PowerShell in a host argv and any shell payload.
fn host_kind(program: &str, args: &[&str], depth: usize) -> Option<PowerShell> {
    let mut kind = powershell_kind(program);
    if depth > 0
        && let Some(payload) = shell_payload(program, args)
    {
        kind = strongest(kind, payload_powershell(&payload, depth - 1));
    }
    kind
}

/// Extract a shell's command string; cmd executes all remaining arguments.
fn shell_payload(program: &str, args: &[&str]) -> Option<String> {
    let name = program_stem(program);
    if name.eq_ignore_ascii_case("cmd") {
        let i = args
            .iter()
            .position(|arg| arg.eq_ignore_ascii_case("/c") || arg.eq_ignore_ascii_case("/k"))?;
        return Some(args[i + 1..].join(" "));
    }
    if !["sh", "bash", "zsh", "dash"]
        .iter()
        .any(|shell| name.eq_ignore_ascii_case(shell))
    {
        return None;
    }
    let i = args.iter().position(|arg| is_c_option(arg))?;
    args.get(i + 1).map(|payload| (*payload).to_owned())
}

/// Match POSIX `-c` and short-option clusters ending in `c` (`-lc`).
fn is_c_option(arg: &str) -> bool {
    let Some(flags) = arg.strip_prefix('-') else {
        return false;
    };
    !flags.is_empty()
        && flags.ends_with('c')
        && !flags.starts_with('-')
        && flags.bytes().all(|b| b.is_ascii_lowercase())
}

/// Encoding reported by Windows PowerShell, probed once.
fn windows_powershell_encoding() -> &'static Encoding {
    static CACHE: OnceLock<&'static Encoding> = OnceLock::new();
    CACHE.get_or_init(|| select_powershell_encoding(probe_windows_powershell()))
}

/// Apply the version rule to a probe result; split out for tests.
fn select_powershell_encoding(
    probe: Option<(u32, Option<&'static Encoding>)>,
) -> &'static Encoding {
    match probe {
        // PowerShell 6+ writes UTF-8 regardless of the console code page.
        Some((major, _)) if major >= 6 => UTF_8,
        Some((_, Some(encoding))) => encoding,
        _ => UTF_8,
    }
}

/// Run the fixed probe with a bounded wait.
fn probe_windows_powershell() -> Option<(u32, Option<&'static Encoding>)> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let program = resolve_command("powershell", &host_path_lists(None), &cwd);
    let mut cmd = std::process::Command::new(program);
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", PROBE])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    sanitize_child_env(&mut cmd);
    detach_child(&mut cmd);

    let mut child = cmd.spawn().ok()?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_probe(&output.stdout)
}

/// Parse `major\nweb-name` from the probe's UTF-8 output.
fn parse_probe(bytes: &[u8]) -> Option<(u32, Option<&'static Encoding>)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let major = lines.next()?.parse().ok()?;
    let web = lines
        .next()
        .and_then(|name| Encoding::for_label(name.as_bytes()));
    Some((major, web))
}

/// Basename of a path with either separator.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Basename without a trailing `.exe`.
fn program_stem(name: &str) -> &str {
    let name = file_name(name);
    let cut = name.len().saturating_sub(4);
    if name
        .get(cut..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".exe"))
    {
        &name[..cut]
    } else {
        name
    }
}
