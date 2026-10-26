//! In-process bashkit interpreter for the `bash` tool.
//!
//! External commands are bridged to host executables via [`HostCommand`]
//! — no real bash process, so the same path works natively on Windows. Scripts
//! the interpreter cannot faithfully handle (parse errors, dynamic command
//! names, `eval`/`exec`/`source`, path-based or glob-shaped names, a declaration
//! operand bashkit mis-splits) make [`analyze_script`] return `Err`, falling
//! back to real `bash -c`.
//!
//! Wrapper builtins (`timeout`, `xargs`, `find -exec`, and the runners `env`,
//! `nice`, `sudo`, `flock`, `busybox`) hide commands in their arguments;
//! [`analyze_script`] extracts those names. `watch`/`parallel` stubs never run
//! commands, so those scripts fall back too. Mirrors bashkit 0.20.1: a wrapper
//! the interpreter starts answering itself must be extracted here (see
//! `every_interpreter_wrapper_is_handled`).
//!
//! [`HOST_PREFERRED`] hands host-tool builtins back to the host, and `which`/
//! `type` skip the rootfs's per-builtin `/bin` stubs (see [`is_command_file`]).
//!
//! `uname` is bridged to the host's own binary (MSYS's included) rather than
//! answered by bashkit's virtual `Linux … sandbox` builtin; a Windows host
//! without one reports its own facts ([`super::uname`]).
//!
//! A bridged name is resolved through `PATH`+`PATHEXT` before spawning — the
//! script's `$PATH` (native form) first, then the host's — so shell shims a
//! bare name hides (`npx` → `npx.cmd`) run like in real bash.
//!
//! Detaching a process is a request this tool cannot serve — background jobs
//! still end with the script, and the process group is killed on timeout — so
//! [`detach_request`] refuses those scripts with a `process` tool hint.
//!
//! Windows path translation is bidirectional, MSYS2-style:
//! [`convert_args_for_host`] rewrites VFS paths in bridged-command args to
//! native form before spawning, and [`rewrite_host_paths`] rewrites
//! host-style paths in builtin args to VFS form (`E:/...` → `/e/...`).
//! `$PATH` is seeded MSYS-style too (`/c/...`, `:`) and translated back on spawn.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use tokio_util::sync::CancellationToken;

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use bashkit::analysis::{AnalyzedCommand, ScriptAnalysis, analyze_with_limits};
use bashkit::parser::{
    Assignment, AssignmentValue, Command as ShellCommand, CompoundCommand, ListOperator, Parser,
    SimpleCommand, Word, WordPart,
};
use bashkit::{
    Bash, Builtin, BuiltinContext, Command, CommandContext, CommandInput, CommandOutput,
    CommandStatus, ExecResult, ExecutionLimits, FileSystem, HttpLimits, NetworkAllowlist,
    async_trait, vfs_join,
};

use super::{CANCEL_REASON, ChunkForwarder, OutStream, OutputSink, timeout_message};
use crate::lock;

/// Per-stream output cap (head-only backstop; crabot's own truncation is the visible limit).
const MAX_STREAM_BYTES: usize = 4 * 1024 * 1024;

/// Names registered for embedded Python.
const PYTHON_NAMES: [&str; 2] = ["python", "python3"];

/// Whether a working `python`/`python3` is on PATH (probed via `--version`)
fn host_python_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        PYTHON_NAMES.iter().copied().any(|name| {
            let mut cmd = std::process::Command::new(name);
            cmd.arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            super::detach_child(&mut cmd);
            cmd.status().is_ok_and(|s| s.success())
        })
    })
}

/// Cached set of every builtin this bashkit build can dispatch.
pub(crate) fn builtin_names() -> &'static HashSet<String> {
    static NAMES: OnceLock<HashSet<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut names: HashSet<String> = Bash::new().builtin_names().into_iter().collect();
        // `Bash::new()` skips builder-registered builtins; `build_bash`
        // registers them via `.cpython()` — only when the host has none.
        if !host_python_available() {
            names.extend(PYTHON_NAMES.map(String::from));
        }
        names
    })
}

/// Names whose in-process stand-in cannot serve a real workspace, so the host
/// binary wins where one exists: `make` (recipes run in the sandbox),
/// `openssl` (only `rand`), `man` (builtin help), and `ps`/`free`/`nproc`
/// (sandbox values, unlike crabot's own `uname`/`hostname`/`TZ`).
const HOST_PREFERRED: [&str; 6] = ["make", "openssl", "man", "ps", "free", "nproc"];

/// The [`HOST_PREFERRED`] names this host resolves; the rest keep bashkit's answer.
fn host_preferred() -> &'static HashSet<&'static str> {
    static NAMES: OnceLock<HashSet<&'static str>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let lists = super::host_path_lists(None);
        let cwd = std::env::current_dir().unwrap_or_default();
        HOST_PREFERRED
            .iter()
            .copied()
            .filter(|name| super::resolve_command(name, &lists, &cwd) != Path::new(name))
            .collect()
    })
}

/// Whether `which`, `type`, the path rewrite, and the bridge agree that `name`
/// is answered in-process.
fn is_interpreter_builtin(name: &str) -> bool {
    builtin_names().contains(name) && !host_preferred().contains(name)
}

/// Static analysis result: names to bridge plus the argument table the
/// Windows path rewrite reads.
pub(crate) struct ScriptPlan {
    external_names: Vec<String>,
    analysis: ScriptAnalysis,
}

/// Statically analyze a script for the in-process interpreter.
///
/// Returns `Err(())` when the script cannot run faithfully in-process (parse
/// error, dynamic/path-based/glob-shaped command names, opaque builtins,
/// wrapper arguments that hide a command). Otherwise returns the names to
/// bridge (empty when only builtins are used) and the argument table.
pub(crate) fn analyze_script(script: &str) -> Result<ScriptPlan, ()> {
    let analysis = analyze_with_limits(script, 100, 100_000).map_err(|_| ())?;
    if analysis.is_opaque() {
        return Err(());
    }
    if splits_declaration_operand(script) {
        return Err(());
    }
    let mut names = Vec::new();
    for command in &analysis.commands {
        let Some(name) = command.name.as_deref() else {
            continue; // dynamic names already made the analysis opaque
        };
        // Unsupported: opaque payload, path-based (`./s.sh`), glob-shaped (`$TOOL`).
        if is_unbridgeable(name) {
            return Err(());
        }
        // Wrappers hide a command in their arguments — extract it for bridging,
        // or fall back when it cannot run faithfully in-process.
        match name {
            "find" => collect_find_commands(command, &mut names)?,
            "timeout" => collect_timeout_command(command, &mut names)?,
            "xargs" => collect_xargs_command(command, &mut names)?,
            // Runners the interpreter answers itself run the command they wrap
            // through this interpreter — the wrapped name must be bridged.
            "env" => collect_env_command(command, &mut names)?,
            "nice" => collect_nice_command(command, &mut names)?,
            "sudo" => collect_sudo_command(command, &mut names)?,
            "flock" => collect_flock_command(command, &mut names)?,
            "busybox" => collect_busybox_command(command, &mut names)?,
            "watch" => return Err(()), // its stub never runs the wrapped command
            "parallel" => return Err(()), // its stub only reports a dry-run plan
            _ => {}
        }
        push_external(name, &mut names);
    }
    Ok(ScriptPlan {
        external_names: names,
        analysis,
    })
}

// ── Detach requests (`&`, `nohup`) ─────────────────────────────────

/// Commands that hand a process past the call — see [`detach_request`].
/// `disown` starts nothing, and `screen`/`tmux` have non-detaching uses.
const DETACH_WRAPPERS: [&str; 3] = ["nohup", "setsid", "daemonize"];

/// How a script asks for a process outliving the `bash` call.
#[derive(PartialEq, Eq, Debug)]
pub(crate) enum Detach {
    /// `cmd &` — the background operator.
    Background,
    /// A detaching wrapper (`nohup`, `setsid`, `daemonize`).
    Wrapper(&'static str),
}

impl Detach {
    /// Agent-facing refusal: what was detected, why it cannot work here, and
    /// the `process` call to use instead.
    pub(crate) fn message(self) -> String {
        let what = match self {
            Detach::Background => "`&` background job".to_string(),
            Detach::Wrapper(name) => format!("`{name}`"),
        };
        format!(
            "The bash tool refused this command ({what}) and did not run it: a backgrounded \
             process runs synchronously here and is killed when the call ends or times out. \
             Use the process tool instead — {{\"action\": \"start\", \"command\": \"...\"}} \
             returns a pid for `logs`, `status`, `input`, and `stop`."
        )
    }
}

/// The detach request in `script`, if any: a `&` background operator or a
/// detaching wrapper in command position.
///
/// Unparsable scripts yield `None` (real bash reports the syntax error).
pub(crate) fn detach_request(script: &str) -> Option<Detach> {
    // Cheap prescan: most scripts contain neither a `&` nor a wrapper name.
    if !script.contains('&') && !DETACH_WRAPPERS.iter().any(|name| script.contains(name)) {
        return None;
    }
    let parsed = Parser::new(script).parse().ok()?;
    detach_in_commands(&parsed.commands)
}

/// First detach request among `commands`, in source order.
fn detach_in_commands(commands: &[ShellCommand]) -> Option<Detach> {
    commands.iter().find_map(detach_in_command)
}

/// Detach request inside one command, recursing into every nested body.
fn detach_in_command(command: &ShellCommand) -> Option<Detach> {
    match command {
        ShellCommand::Simple(cmd) => detach_in_simple(cmd),
        ShellCommand::Pipeline(pipeline) => detach_in_commands(&pipeline.commands),
        // Source order: the leftmost signal names the message.
        ShellCommand::List(list) => detach_in_command(&list.first).or_else(|| {
            list.rest.iter().find_map(|(op, cmd)| match op {
                ListOperator::Background => Some(Detach::Background),
                _ => detach_in_command(cmd),
            })
        }),
        ShellCommand::Compound(compound, redirects) => compound_bodies(compound)
            .into_iter()
            .find_map(detach_in_commands)
            .or_else(|| detach_in_words(redirects.iter().map(|redirect| &redirect.target))),
        ShellCommand::Function(def) => detach_in_command(&def.body),
    }
}

/// Every command list a compound command holds, in source order.
fn compound_bodies(compound: &CompoundCommand) -> Vec<&[ShellCommand]> {
    match compound {
        CompoundCommand::If(stmt) => [&stmt.condition, &stmt.then_branch]
            .into_iter()
            .chain(
                stmt.elif_branches
                    .iter()
                    .flat_map(|(condition, body)| [condition, body]),
            )
            .chain(stmt.else_branch.iter())
            .map(Vec::as_slice)
            .collect(),
        CompoundCommand::For(stmt) => vec![&stmt.body],
        CompoundCommand::ArithmeticFor(stmt) => vec![&stmt.body],
        CompoundCommand::While(stmt) => vec![&stmt.condition, &stmt.body],
        CompoundCommand::Until(stmt) => vec![&stmt.condition, &stmt.body],
        CompoundCommand::Case(stmt) => stmt
            .cases
            .iter()
            .map(|item| item.commands.as_slice())
            .collect(),
        CompoundCommand::Select(stmt) => vec![&stmt.body],
        CompoundCommand::Subshell(commands) | CompoundCommand::BraceGroup(commands) => {
            vec![commands.as_slice()]
        }
        // The single command `time`/`coproc` wraps counts as a body of its own.
        CompoundCommand::Time(stmt) => stmt
            .command
            .as_deref()
            .map(std::slice::from_ref)
            .into_iter()
            .collect(),
        CompoundCommand::Coproc(stmt) => vec![std::slice::from_ref(&stmt.body)],
        CompoundCommand::Arithmetic(_) | CompoundCommand::Conditional(_) => Vec::new(),
    }
}

/// Detach request in a simple command: a wrapper name in command position, or
/// a command/process substitution in any word it carries — arguments, prefix
/// assignments (`LOG=$(nohup server &)`), and redirect targets.
fn detach_in_simple(cmd: &SimpleCommand) -> Option<Detach> {
    detach_wrapper(&cmd.name).map(Detach::Wrapper).or_else(|| {
        detach_in_words(std::iter::once(&cmd.name).chain(&cmd.args))
            .or_else(|| cmd.assignments.iter().find_map(detach_in_assignment))
            .or_else(|| detach_in_words(cmd.redirects.iter().map(|redirect| &redirect.target)))
    })
}

/// Detach request inside an assignment value (`LOG=$(nohup server &)`).
fn detach_in_assignment(assignment: &Assignment) -> Option<Detach> {
    match &assignment.value {
        AssignmentValue::Scalar(word) => detach_in_word(word),
        AssignmentValue::Array(words) => detach_in_words(words),
    }
}

/// First detach request among `words`, in source order.
fn detach_in_words<'a>(words: impl IntoIterator<Item = &'a Word>) -> Option<Detach> {
    words.into_iter().find_map(detach_in_word)
}

/// The detaching wrapper a word names, when every part of it is literal.
fn detach_wrapper(word: &Word) -> Option<&'static str> {
    let text = literal_text(word)?;
    DETACH_WRAPPERS.into_iter().find(|wrapper| *wrapper == text)
}

/// The text a word holds, when every part of it is literal.
fn literal_text(word: &Word) -> Option<String> {
    word.parts
        .iter()
        .map(|part| match part {
            WordPart::Literal(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// Detach request inside a word's command and process substitutions.
fn detach_in_word(word: &Word) -> Option<Detach> {
    word.parts.iter().find_map(|part| match part {
        WordPart::CommandSubstitution(commands)
        | WordPart::ProcessSubstitution { commands, .. } => detach_in_commands(commands),
        _ => None,
    })
}

// ── Declaration operands bashkit 0.20 mis-splits ───────────────────

/// Builtins whose `name=value` operand goes through `interpreter/declare.rs`.
const DECLARATION_BUILTINS: [&str; 5] = ["export", "readonly", "declare", "typeset", "local"];

/// Whether a declaration builtin gets a mixed-quote operand
/// (`export PATH="/x":$PATH`) that bashkit 0.20 splits on `IFS`, truncating it
/// where bash keeps it whole — such scripts take the real-`bash` route.
///
/// Drop this guard once bashkit keeps the operand whole (0.18 did).
fn splits_declaration_operand(script: &str) -> bool {
    // Cheap prescan: a script naming no declaration builtin cannot assign one.
    if !DECLARATION_BUILTINS
        .iter()
        .any(|name| script.contains(name))
    {
        return false;
    }
    // A syntax error is `analyze_with_limits`' to report, not this walk's.
    Parser::new(script)
        .parse()
        .is_ok_and(|parsed| operand_splits_in_commands(&parsed.commands))
}

/// Whether any command, nested bodies included, carries a mis-split operand.
fn operand_splits_in_commands(commands: &[ShellCommand]) -> bool {
    commands.iter().any(command_splits_operand)
}

/// Whether one command carries a mis-split declaration operand.
fn command_splits_operand(command: &ShellCommand) -> bool {
    match command {
        ShellCommand::Simple(cmd) => simple_splits_operand(cmd),
        ShellCommand::Pipeline(pipeline) => operand_splits_in_commands(&pipeline.commands),
        ShellCommand::List(list) => {
            command_splits_operand(&list.first)
                || list.rest.iter().any(|(_, cmd)| command_splits_operand(cmd))
        }
        ShellCommand::Compound(compound, _) => compound_bodies(compound)
            .into_iter()
            .any(operand_splits_in_commands),
        ShellCommand::Function(def) => command_splits_operand(&def.body),
    }
}

/// Whether a declaration builtin's operand list holds a mis-split value.
fn simple_splits_operand(cmd: &SimpleCommand) -> bool {
    literal_text(&cmd.name).is_some_and(|name| {
        DECLARATION_BUILTINS.contains(&name.as_str()) && cmd.args.iter().any(operand_would_split)
    })
}

/// `name=…'…'$x`: the operand shape whose mixed quoting sends bashkit's
/// expansion down the splitting path its declaration rule skips.
fn operand_would_split(word: &Word) -> bool {
    let assignment = matches!(
        word.parts.first(),
        Some(WordPart::Literal(text)) if text.contains('=')
    );
    let quoted = |index: usize| word.part_quoted.get(index).copied().unwrap_or(word.quoted);
    let (mut quoted_part, mut unquoted_expansion) = (false, false);
    for (index, part) in word.parts.iter().enumerate() {
        if quoted(index) {
            quoted_part = true;
        } else if !matches!(part, WordPart::Literal(_)) {
            unquoted_expansion = true;
        }
    }
    // `x=/a:'b c'` keeps its value; `x="/a":$b` splits.
    assignment && quoted_part && unquoted_expansion
}

/// Names that must never be bridged: opaque builtins (`command`/`exec`),
/// interpreter re-entries (`eval`/`source`/`.`/`bash`/`sh`), and path-based or
/// glob-shaped names. `[` is exempt — its name is literally `[`.
///
/// Wrappers that are not builtins (`doas`, `setsid`, `stdbuf`) are bridged
/// themselves and spawn the wrapped command like real bash.
fn is_unbridgeable(name: &str) -> bool {
    matches!(
        name,
        "command" | "exec" | "eval" | "source" | "." | "bash" | "sh"
    ) || name.contains('/')
        || (name != "[" && name.contains(['$', '`', '*', '?', '[']))
}

/// Append `name` once, unless the interpreter answers it.
fn push_external(name: &str, names: &mut Vec<String>) {
    if !is_interpreter_builtin(name) && !names.iter().any(|n| n == name) {
        names.push(name.to_string());
    }
}

/// Register a wrapped command name; a non-literal or unbridgeable one falls back.
fn push_wrapped_arg(arg: Option<&str>, names: &mut Vec<String>) -> Result<(), ()> {
    let Some(cmd) = arg else {
        return Ok(()); // no command — wrapper default or bashkit's own error
    };
    if is_unbridgeable(cmd) {
        return Err(()); // cannot bridge — fall back
    }
    push_external(cmd, names);
    Ok(())
}

/// `env [-i] [-u NAME] [-C DIR] [NAME=VALUE]... [COMMAND [ARG]...]` — register
/// the COMMAND; assignment-only invocations keep the interpreter's `env`.
fn collect_env_command(command: &AnalyzedCommand, names: &mut Vec<String>) -> Result<(), ()> {
    let args = command.literal_args().ok_or(())?;
    if matches!(args.first().copied(), Some("--help" | "--version")) {
        return Ok(()); // help prints; nothing runs
    }
    let i = skip_options(&args, &ENV_OPTS)?;
    // `NAME=VALUE` operands configure the child; the first other one runs.
    match args[i..].iter().find(|arg| !is_env_assignment(arg)) {
        Some(command) => push_wrapped_arg(Some(command), names),
        None => Ok(()),
    }
}

/// `NAME=VALUE` as `env` reads one: any non-empty name before the `=`.
fn is_env_assignment(arg: &str) -> bool {
    arg.split_once('=')
        .is_some_and(|(name, _)| !name.is_empty())
}

/// `nice [-n N] [--adjustment=N] [COMMAND [ARG]...]` — register the COMMAND.
fn collect_nice_command(command: &AnalyzedCommand, names: &mut Vec<String>) -> Result<(), ()> {
    let args = command.literal_args().ok_or(())?;
    // A leading `--help`/`--version` is answered by the interpreter itself.
    if matches!(args.first().copied(), Some("--help" | "--version")) {
        return Ok(());
    }
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if *arg == "--" {
            i += 1;
            break;
        }
        if matches!(*arg, "-n" | "--adjustment") {
            i += 2; // flag and its value
        } else if is_nice_adjustment(arg) {
            i += 1; // `-n5`, `--adjustment=5`, `-5`
        } else if arg.starts_with('-') && arg.len() > 1 {
            return Err(()); // unknown option — bashkit would run it as the command
        } else {
            break; // COMMAND
        }
    }
    push_wrapped_arg(args.get(i).copied(), names)
}

/// A `nice` adjustment: `-n5`, `--adjustment=5`, or bare `-5`.
fn is_nice_adjustment(arg: &str) -> bool {
    fn digits(value: &str) -> bool {
        !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
    }
    arg.strip_prefix("--adjustment=").is_some()
        || arg.strip_prefix("-n").is_some_and(digits)
        || arg.strip_prefix('-').is_some_and(digits)
}

/// `sudo [-OPTIONS] [VAR=VALUE]... COMMAND [ARG]...` — register the COMMAND.
/// `-s`/`-i` hand it to a shell instead, and only real bash can run that.
fn collect_sudo_command(command: &AnalyzedCommand, names: &mut Vec<String>) -> Result<(), ()> {
    let args = command.literal_args().ok_or(())?;
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        match *arg {
            "--" => {
                i += 1;
                break;
            }
            // Options with a value.
            "-u" | "--user" | "-g" | "--group" | "-C" | "--close-from" | "-p" | "--prompt"
            | "-h" | "--host" | "-r" | "--role" | "-t" | "--type" | "-U" | "--other-user"
            | "-D" | "--chdir" | "-T" | "--command-timeout" => i += 2,
            // A login/interactive shell — fall back rather than script it here.
            "-s" | "--shell" | "-i" | "--login" => return Err(()),
            // The policy alone: nothing runs.
            "-l" | "--list" => return Ok(()),
            "-v" | "--validate" | "-k" | "--reset-timestamp" | "-K" | "--remove-timestamp"
                if args.len() == i + 1 =>
            {
                return Ok(());
            }
            a if a.starts_with("--") && a.len() > 2 => i += 1,
            a if a.starts_with('-') && a.len() > 1 => {
                // Clustered short flags (`-nE`); a value flag at the end of
                // the cluster takes the next operand (`-nu root`).
                if a[1..].contains(['s', 'i']) {
                    return Err(());
                }
                let last = a.as_bytes()[a.len() - 1];
                i += if b"ugCphrtUDT".contains(&last) { 2 } else { 1 };
            }
            _ => break, // COMMAND or VAR=VALUE
        }
    }
    // Leading `VAR=VALUE` operands go to the command's environment, not to it.
    let rest = args.get(i..).unwrap_or_default();
    let start = rest.iter().take_while(|arg| is_var_assignment(arg)).count();
    push_wrapped_arg(rest.get(start).copied(), names)
}

/// `NAME=VALUE` as `sudo` reads one: a shell identifier before the `=`.
fn is_var_assignment(arg: &str) -> bool {
    arg.split_once('=').is_some_and(|(name, _)| {
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// `flock [OPTIONS] FILE|FD [COMMAND [ARG]...]` — register the COMMAND;
/// `-c`/`--command` hands a shell string to real bash instead.
fn collect_flock_command(command: &AnalyzedCommand, names: &mut Vec<String>) -> Result<(), ()> {
    let args = command.literal_args().ok_or(())?;
    if matches!(args.first().copied(), Some("--help" | "--version")) {
        return Ok(()); // help prints; nothing runs
    }
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        match *arg {
            "--" => {
                i += 1;
                break;
            }
            "-s" | "--shared" | "-x" | "--exclusive" | "-u" | "--unlock" | "-n" | "--nb"
            | "--nonblock" | "-o" | "--close" | "-F" | "--no-fork" | "--verbose" => i += 1,
            "-w" | "--wait" | "--timeout" | "-E" | "--conflict-exit-code" => i += 2,
            a if a.starts_with("--wait=")
                || a.starts_with("--timeout=")
                || a.starts_with("--conflict-exit-code=") =>
            {
                i += 1
            }
            a if a.starts_with('-') && a.len() > 1 && a != "-c" => return Err(()),
            _ => break, // FILE|FD
        }
    }
    match args.get(i + 1).copied() {
        Some("-c" | "--command") => Err(()),
        command => push_wrapped_arg(command, names),
    }
}

/// `busybox APPLET [ARG]...` — register the applet, which the interpreter
/// answers as a builtin of that name.
fn collect_busybox_command(command: &AnalyzedCommand, names: &mut Vec<String>) -> Result<(), ()> {
    let args = command.literal_args().ok_or(())?;
    match args.first().copied() {
        // Applet listings and help print; nothing runs.
        None | Some("--list" | "--list-full" | "-l" | "--help" | "--version") => Ok(()),
        Some(applet) => push_wrapped_arg(Some(applet.rsplit('/').next().unwrap_or(applet)), names),
    }
}

/// Option surface of a bashkit wrapper builtin, mirroring its parser.
struct WrapperOpts {
    /// Flags with a separate value (`-k 5`, `--max-procs 4`).
    with_value: &'static [&'static str],
    /// Prefixes of attached-value flags (`-n5`, `--max-procs=4`).
    attached: &'static [&'static str],
    /// Flags consumed as-is (`--preserve-status`, `-0`).
    plain: &'static [&'static str],
    /// Skip unknown flags (timeout); otherwise they make bashkit error out.
    lenient: bool,
}

const TIMEOUT_OPTS: WrapperOpts = WrapperOpts {
    with_value: &["-k", "-s"],
    attached: &[],
    plain: &["--preserve-status"],
    lenient: true,
};

const ENV_OPTS: WrapperOpts = WrapperOpts {
    with_value: &["-u", "--unset", "-C", "--chdir"],
    attached: &["-u", "-C", "--unset=", "--chdir="],
    plain: &["-i", "--ignore-environment", "-0", "--null"],
    lenient: false,
};

const XARGS_OPTS: WrapperOpts = WrapperOpts {
    with_value: &["-I", "-n", "-d", "-P", "--max-procs", "--process-slot-var"],
    attached: &[
        "-I",
        "-n",
        "-d",
        "-P",
        "--max-procs=",
        "--process-slot-var=",
    ],
    plain: &["-0", "--help", "--version"],
    lenient: false,
};

/// Scan past `--` and the wrapper's option/value args; returns the first
/// operand's index. `Err` when an unknown option would make bashkit fail first.
fn skip_options(args: &[&str], opts: &WrapperOpts) -> Result<usize, ()> {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if arg == "--" {
            return Ok(i + 1); // end of options
        }
        if opts.with_value.contains(&arg) {
            i += 2; // flag + value
        } else if opts.attached.iter().any(|f| arg.starts_with(f)) {
            i += 1; // attached value (`-n5`, `--max-procs=4`)
        } else if opts.plain.contains(&arg) {
            i += 1;
        } else if arg.len() > 1 && arg.starts_with('-') {
            if !opts.lenient {
                return Err(()); // unknown option — bashkit errors; fall back
            }
            if arg.as_bytes()[1].is_ascii_digit() {
                break; // negative-looking DURATION (timeout)
            }
            i += 1; // timeout skips unknown flags
        } else {
            break; // COMMAND position
        }
    }
    Ok(i)
}

/// `timeout [OPTION] DURATION COMMAND [ARG]...` — register the wrapped
/// COMMAND (a missing one is bashkit's error to report).
fn collect_timeout_command(command: &AnalyzedCommand, names: &mut Vec<String>) -> Result<(), ()> {
    let args = command.literal_args().ok_or(())?;
    let i = skip_options(&args, &TIMEOUT_OPTS)?;
    push_wrapped_arg(args.get(i + 1).copied(), names)
}

/// `xargs [OPTION]... [COMMAND [ARG]...]` — register the wrapped COMMAND
/// (bashkit defaults to the `echo` builtin when absent).
fn collect_xargs_command(command: &AnalyzedCommand, names: &mut Vec<String>) -> Result<(), ()> {
    let args = command.literal_args().ok_or(())?;
    let i = skip_options(&args, &XARGS_OPTS)?;
    push_wrapped_arg(args.get(i).copied(), names)
}

/// `find [PATH]... [EXPRESSION]` — register the command of every
/// `-exec`/`-execdir` template (first template arg, up to `;`/`\;`/`+`).
/// Unknown predicates make bashkit fail before dispatching → fall back.
fn collect_find_commands(command: &AnalyzedCommand, names: &mut Vec<String>) -> Result<(), ()> {
    let args = command.literal_args().ok_or(())?;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        match arg {
            "-name" | "-path" | "-type" | "-maxdepth" | "-mindepth" | "-printf" => i += 2,
            "-print" | "-print0" | "-not" | "!" => i += 1,
            "-exec" | "-execdir" => {
                let mut cmd = None;
                i += 1;
                while i < args.len() && !matches!(args[i], ";" | "\\;" | "+") {
                    cmd.get_or_insert(args[i]);
                    i += 1;
                }
                if let Some(cmd) = cmd {
                    push_wrapped_arg(Some(cmd), names)?;
                }
                i += 1; // past the terminator
            }
            _ if arg.len() > 1 && arg.starts_with('-') => {
                return Err(()); // unknown predicate (`-delete`, `-ok`, …)
            }
            _ => i += 1, // search path
        }
    }
    Ok(())
}

/// Execute `command` through the in-process bashkit interpreter.
///
/// `plan.external_names` are bridged to host executables; the whole script
/// shares `timeout` and the caller's cancel flag. `sink` streams output live,
/// and timeout/cancel errors carry the partial output.
///
/// # Panics
///
/// Uses `Handle::block_on`: call only from `spawn_blocking`.
pub(crate) fn execute(
    command: &str,
    workspace: &Path,
    timeout: Duration,
    cancel: &CancellationToken,
    plan: ScriptPlan,
    sink: Option<OutputSink>,
) -> Result<String, String> {
    // Script-level forwarder: coalescing and partial capture apply per script.
    let forwarder = Arc::new(Mutex::new(ChunkForwarder::new(sink)));

    // One mount table for the rewrite and the interpreter.
    let home_mount = real_home_mount();
    let mount_specs = mounts(workspace, home_mount.as_ref());
    // Windows: rewrite host-style paths in builtin args to VFS form.
    let script = rewrite_host_paths(command, &plan.analysis, &mount_specs);

    let mut bash = build_bash(workspace, timeout, &plan, mount_specs, home_mount);

    let handle = tokio::runtime::Handle::current();
    let result = handle
        .block_on(async {
            // Flush time-due chunks while output is quiet.
            let _ticker = FlushTicker(tokio::spawn(flush_ticker(Arc::clone(&forwarder))));
            let push = |stream: OutStream, forwarder: Arc<Mutex<ChunkForwarder>>| {
                move |bytes: Vec<u8>| {
                    let forwarder = Arc::clone(&forwarder);
                    async move {
                        lock(&forwarder).push(stream, &bytes);
                        Ok::<(), bashkit::Error>(())
                    }
                }
            };
            let execution = bash
                .execute(&script)
                .capture(MAX_STREAM_BYTES)
                .stdout_fn(push(OutStream::Stdout, Arc::clone(&forwarder)))
                .stderr_fn(push(OutStream::Stderr, Arc::clone(&forwarder)));
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(error_with_partial(&forwarder, CANCEL_REASON)),
                result = execution => {
                    result.map_err(|error| script_error(error, &forwarder, timeout))
                }
                // Backstop in case an interpreter path misses its own deadline.
                _ = tokio::time::sleep(timeout + Duration::from_secs(1)) => {
                    Err(error_with_partial(&forwarder, &timeout_message(timeout)))
                }
            }
        })
        .map(|result| format_exec_result(&result));

    // Flush carried/coalesced bytes; the aborted ticker may still hold the lock.
    if let Some(mut guard) = super::try_lock_for(&forwarder, Duration::from_millis(200)) {
        guard.finish();
    }
    result
}

/// Map an interpreter failure to the tool error, keeping partial output.
fn script_error(
    error: bashkit::Error,
    forwarder: &Arc<Mutex<ChunkForwarder>>,
    timeout: Duration,
) -> String {
    use bashkit::{Error, ExecutionBudgetExceeded as Budget, LimitExceeded};
    match error {
        Error::Cancelled
        | Error::ResourceLimit(LimitExceeded::ExecutionBudget(Budget::Cancelled)) => {
            error_with_partial(forwarder, CANCEL_REASON)
        }
        Error::ResourceLimit(LimitExceeded::Timeout(_))
        | Error::ResourceLimit(LimitExceeded::ExecutionBudget(Budget::Deadline { .. })) => {
            error_with_partial(forwarder, &timeout_message(timeout))
        }
        error => error.to_string(),
    }
}

/// Timeout/cancel error with captured partial output; falls back to the bare
/// reason when the forwarder lock stays held past [`super::CAPTURE_GRACE`].
fn error_with_partial(forwarder: &Arc<Mutex<ChunkForwarder>>, reason: &str) -> String {
    let Some(guard) = super::try_lock_for(forwarder, super::CAPTURE_GRACE) else {
        return reason.to_string();
    };
    let mut msg = reason.to_string();
    guard.append_partial_output(&mut msg);
    msg
}

/// Periodically tick the forwarder while the script runs.
async fn flush_ticker(forwarder: Arc<Mutex<ChunkForwarder>>) {
    loop {
        tokio::time::sleep(super::COALESCE_MS).await;
        lock(&forwarder).tick();
    }
}

/// Abort-on-drop guard so the ticker never outlives the script (including
/// when the enclosing future is dropped on timeout/cancel).
struct FlushTicker(tokio::task::JoinHandle<()>);

impl Drop for FlushTicker {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Build a bashkit `Bash` wired to the real workspace.
///
/// Applies the mount table, seeds env, bridges external command names, and
/// installs the path-aware `which`/`type` replacements.
fn build_bash(
    workspace: &Path,
    timeout: Duration,
    plan: &ScriptPlan,
    mount_specs: Vec<RealMount>,
    home_mount: Option<RealMount>,
) -> Bash {
    let workspace_vfs = super::convert_path_to_unix_style(workspace);
    let shared_mounts: Arc<[RealMount]> = Arc::from(mount_specs.clone());

    let mut builder = Bash::builder()
        .cwd(PathBuf::from(&workspace_vfs))
        .allowed_mount_paths(
            mount_specs
                .iter()
                .map(|m| m.host_path.clone())
                .collect::<Vec<_>>(),
        );
    for m in mount_specs {
        builder = if m.writable {
            builder.mount_real_readwrite_at(m.host_path, m.vfs_path)
        } else {
            builder.mount_real_readonly_at(m.host_path, m.vfs_path)
        };
    }

    builder = builder.limits(
        ExecutionLimits::default()
            .timeout(timeout)
            .max_commands(1_000_000)
            .max_stdout_bytes(MAX_STREAM_BYTES)
            .max_stderr_bytes(MAX_STREAM_BYTES),
    );

    // Real host identity — whoami/hostname/uname -n report the actual host.
    if let Some(username) = real_username() {
        builder = builder.username(username);
    }
    if let Some(hostname) = real_hostname() {
        builder = builder.hostname(hostname);
    }

    // curl/wget (http_client feature): open policy like the `fetch` tool,
    // limits raised to bashkit's cap (600s / 64 MB).
    builder = builder
        .network(NetworkAllowlist::allow_all().block_private_ips(false))
        .http_limits(HttpLimits {
            timeout: Duration::from_secs(600),
            max_response_bytes: 64 * 1024 * 1024,
        });

    // Route sandbox curl/wget through the system proxy when applied.
    if let Some(transport) = super::proxy::system_proxy_transport() {
        builder = builder.http_transport(transport);
    }

    // Seed env from host, minus HOME (seeded from the VFS home mount so `~`
    // expands to a POSIX path) and secrets (`*API_KEY` names). `PATH` is seeded
    // MSYS-style below, always as `PATH` whatever spelling the host uses.
    let mut host_path = None;
    for (key, value) in std::env::vars() {
        if super::is_path_env_key(&key) {
            host_path = Some(value);
        } else if key != "HOME" && !super::is_secret_env_key(&key) {
            builder = builder.env(key, value);
        }
    }
    if let Some(path) = host_path {
        builder = builder.env("PATH", super::convert_path_list_to_posix(&path));
    }
    if let Some(home) = &home_mount {
        builder = builder.env("HOME", home.vfs_path.to_string_lossy().into_owned());
    }

    // Seed bash platform variables — hosts without bash (native Windows)
    // export none of these. Seeded after host vars, so they always win.
    let (ostype, machine) = platform_labels();
    builder = builder
        .env("OSTYPE", ostype)
        .env("HOSTTYPE", std::env::consts::ARCH)
        .env("MACHTYPE", format!("{}-{machine}", std::env::consts::ARCH));
    // What the host bash exports — MSYS tools read their platform here.
    #[cfg(windows)]
    if let Some(msystem) = super::uname::host_msystem() {
        builder = builder.env("MSYSTEM", msystem);
    }

    // Embedded Python: real CPython 3.14 in a Wasm sandbox, registered only
    // when the host has none; each call is a fresh instance over the VFS,
    // capped at `CPythonLimits` and inheriting the script's own deadline.
    if !host_python_available() {
        builder =
            builder.cpython_with_limits(bashkit::CPythonLimits::default().max_duration(timeout));
    }

    // bashkit's own `which`/`type` search its virtual `/bin` and `/usr/bin`
    // stubs; these two report what actually runs — replace both.
    let lookup = Arc::new(CommandLookup {
        mounts: Arc::clone(&shared_mounts),
        functions: plan.analysis.functions.iter().cloned().collect(),
    });
    builder = builder
        .builtin(
            "which",
            Box::new(WhichBuiltin {
                lookup: Arc::clone(&lookup),
            }),
        )
        .builtin("type", Box::new(TypeBuiltin { lookup }));

    // Bridge each host command; bashkit's own `uname` reports a virtual platform.
    let host_command = |program: String| HostCommand {
        program,
        mounts: Arc::clone(&shared_mounts),
        home: home_mount.clone(),
    };
    for name in &plan.external_names {
        builder = builder.command(name.clone(), host_command(name.clone()));
    }
    // Unix always has `uname`; Windows may not, so the fallback covers it.
    #[cfg(windows)]
    {
        let host = super::uname::host_uname()
            .map(|path| host_command(path.to_string_lossy().into_owned()));
        builder = builder.command("uname", super::uname::Uname::new(host));
    }
    #[cfg(not(windows))]
    if let Some(uname) = super::uname::host_uname() {
        builder = builder.command("uname", host_command(uname.to_string_lossy().into_owned()));
    }

    // bashkit warns on stderr per read-write mount; silence it while building.
    let _lock = lock(&SILENCER_LOCK);
    let _silence = stderr_silencer::StderrSilencer::new();
    builder.build()
}

/// Serializes `Bash::build`: the stderr swap is process-wide.
static SILENCER_LOCK: Mutex<()> = Mutex::new(());

/// Host command bridged into the interpreter as a streaming command.
pub(crate) struct HostCommand {
    /// Program to spawn: a `PATH` name or the resolved host `uname` path.
    program: String,
    /// VFS mount table, shared with `build_bash` via Arc.
    mounts: Arc<[RealMount]>,
    /// Seeded home mount; maps the interpreter's VFS HOME to the host path.
    home: Option<RealMount>,
}

/// Mirror the script env into the child: native `PATH`, remapped HOME, no secrets.
fn apply_child_env(
    cmd: &mut std::process::Command,
    env: &HashMap<String, String>,
    path: Option<&str>,
    home: Option<&RealMount>,
    mounts: &[RealMount],
) {
    cmd.env_clear();
    for (key, value) in env {
        // The script's `PATH` is applied below; HOME maps back to its host path.
        if super::is_path_env_key(key) {
            continue;
        }
        if key == "HOME"
            && let Some(home) = home
            && value.as_str() == home.vfs_path.to_string_lossy()
        {
            // Only the seeded VFS spelling remaps; a script-assigned HOME passes through.
            cmd.env("HOME", &home.host_path);
        } else if !super::is_secret_env_key(key) {
            cmd.env(key, value);
        }
    }
    if let Some(path) = path {
        cmd.env("PATH", path_list_for_host(path, mounts));
    }
    // No host home: keep the inherited HOME as a fallback.
    if home.is_none()
        && let Some(home_dir) = std::env::var_os("HOME")
    {
        cmd.env("HOME", home_dir);
    }
}

impl HostCommand {
    /// Build the child command, or the reason it cannot run.
    fn prepare(&self, ctx: &CommandContext<'_>) -> Result<tokio::process::Command, String> {
        let cwd = resolve_cwd(ctx.cwd, &self.mounts)?;
        let path = ctx.env.get("PATH").map(String::as_str);
        let paths = path_lists_for_host(path, &self.mounts);
        let mut cmd =
            tokio::process::Command::new(super::resolve_command(&self.program, &paths, &cwd));
        cmd.args(convert_args_for_host(ctx.args, &self.mounts))
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        apply_child_env(
            cmd.as_std_mut(),
            ctx.env,
            path,
            self.home.as_ref(),
            &self.mounts,
        );
        super::detach_child(cmd.as_std_mut());
        Ok(cmd)
    }
}

#[async_trait]
impl Command for HostCommand {
    async fn execute(&self, ctx: CommandContext<'_>) -> bashkit::Result<CommandStatus> {
        let diagnostic = ctx.stderr.clone();
        let name = ctx.name;
        let mut cmd = match self.prepare(&ctx) {
            Ok(cmd) => cmd,
            Err(error) => {
                let _ = diagnostic.write_all(error.as_bytes()).await;
                return Ok(CommandStatus::with_code(1));
            }
        };
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                let message = if error.kind() == std::io::ErrorKind::NotFound {
                    format!("bash: {name}: command not found\n")
                } else {
                    format!("bash: {name}: {error}\n")
                };
                let _ = diagnostic.write_all(message.as_bytes()).await;
                return Ok(CommandStatus::with_code(127));
            }
        };

        let status = match wait_with_pipes(
            &mut child,
            ctx.stdin.clone(),
            ctx.stdout.clone(),
            ctx.stderr.clone(),
        )
        .await
        {
            Ok(status) => status,
            Err(bashkit::Error::Io(error)) => {
                let message = format!("bash: {name}: {error}\n");
                let _ = diagnostic.write_all(message.as_bytes()).await;
                return Ok(CommandStatus::with_code(1));
            }
            Err(error) => return Err(error),
        };
        Ok(CommandStatus::with_code(super::exit_code_of(&status)))
    }
}

/// Wait for `child` while pumping its stdio, so a full pipe can't block it.
async fn wait_with_pipes(
    child: &mut tokio::process::Child,
    stdin: CommandInput,
    stdout: CommandOutput,
    stderr: CommandOutput,
) -> bashkit::Result<std::process::ExitStatus> {
    let mut guard = KillTree(child.id());
    let mut feed = Box::pin(feed_stdin(stdin, child.stdin.take()));
    let mut out = Box::pin(pump(child.stdout.take(), stdout));
    let mut err = Box::pin(pump(child.stderr.take(), stderr));

    let (mut feed_done, mut out_done, mut err_done) = (false, false, false);
    let status = loop {
        tokio::select! {
            status = child.wait() => break status,
            result = &mut feed, if !feed_done => {
                feed_done = true;
                result?;
            }
            result = &mut out, if !out_done => {
                out_done = true;
                result?;
            }
            result = &mut err, if !err_done => {
                err_done = true;
                result?;
            }
        }
    };
    guard.0 = None; // reaped: never signal a reused pid
    drop(feed);
    // A grandchild may still hold the pipes open after the child exits.
    let drained = tokio::time::timeout(Duration::from_secs(2), async {
        if !out_done {
            out.as_mut().await?;
        }
        if !err_done {
            err.as_mut().await?;
        }
        Ok::<(), bashkit::Error>(())
    })
    .await;
    if let Ok(result) = drained {
        result?;
    }
    status.map_err(bashkit::Error::Io)
}

/// Kill the child's process tree if the command is dropped before reaping.
struct KillTree(Option<u32>);

impl Drop for KillTree {
    fn drop(&mut self) {
        if let Some(pid) = self.0.take() {
            super::kill_process_tree(pid);
        }
    }
}

/// Feed the shell's stdin into the child; a closed child ends it.
async fn feed_stdin(
    input: CommandInput,
    mut stdin: Option<tokio::process::ChildStdin>,
) -> bashkit::Result<()> {
    use tokio::io::AsyncWriteExt;

    let Some(stdin) = stdin.as_mut() else {
        return Ok(());
    };
    while let Some(chunk) = input.read_chunk().await? {
        if stdin.write_all(&chunk).await.is_err() {
            break; // child closed stdin
        }
    }
    Ok(())
}

/// Copy a child pipe into a shell stream; a closed reader stops it.
async fn pump<R>(reader: Option<R>, out: CommandOutput) -> bashkit::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let Some(mut reader) = reader else {
        return Ok(());
    };
    let mut buf = [0u8; 8192];
    loop {
        let read = reader.read(&mut buf).await?;
        if read == 0 {
            return Ok(());
        }
        match out.write_all(&buf[..read]).await {
            Ok(()) => {}
            Err(bashkit::Error::Io(error)) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                return Ok(());
            }
            Err(error) => return Err(error),
        }
    }
}

// ── Introspection builtins (`which`, `type`) ────────────────────────
//
// bashkit answers from its virtual `/bin` stubs; these resolve through the
// script's `$PATH` (probing `PATHEXT`), then the host search, printing VFS paths.

/// What a command name resolves to, in the shell's lookup order.
enum CommandKind {
    Function,
    Keyword,
    Builtin,
    /// A file, as the VFS spells it.
    File(String),
}

impl CommandKind {
    /// The word `type -t` prints.
    fn label(&self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Keyword => "keyword",
            Self::Builtin => "builtin",
            Self::File(_) => "file",
        }
    }

    /// The line `type` prints.
    fn describe(&self, name: &str) -> String {
        match self {
            Self::Function => format!("{name} is a function"),
            Self::Keyword => format!("{name} is a shell keyword"),
            Self::Builtin => format!("{name} is a shell builtin"),
            Self::File(path) => format!("{name} is {path}"),
        }
    }
}

/// Command lookup shared by [`WhichBuiltin`] and [`TypeBuiltin`].
struct CommandLookup {
    /// VFS mount table, shared with [`HostCommand`].
    mounts: Arc<[RealMount]>,
    /// Functions the script defines — bashkit's analysis collects them, since
    /// a custom builtin cannot see the interpreter's own table.
    functions: HashSet<String>,
}

impl CommandLookup {
    /// The kind the interpreter dispatches itself; `None` when a file has to
    /// be found.
    fn dispatched(&self, name: &str) -> Option<CommandKind> {
        if self.functions.contains(name) {
            Some(CommandKind::Function)
        } else if is_keyword(name) {
            Some(CommandKind::Keyword)
        } else if is_interpreter_builtin(name) {
            Some(CommandKind::Builtin)
        } else {
            None
        }
    }

    /// The operand as the VFS spells it: a native Windows path
    /// (`C:\tools\tool.exe`) resolves through the mount table, everything else
    /// passes through. [`rewrite_host_paths`] converts most host-shaped args,
    /// but `type`'s operands are protected from it — the lookup owns those.
    fn vfs_operand<'a>(&self, name: &'a str) -> Cow<'a, str> {
        #[cfg(windows)]
        if let Some(vfs) = host_arg_to_vfs(name, &self.mounts) {
            return Cow::Owned(vfs);
        }
        Cow::Borrowed(name)
    }

    /// Every file `name` resolves to, in search order; `all` keeps looking
    /// past the first hit (`which -a`).
    async fn files(&self, name: &str, ctx: &BuiltinContext<'_>, all: bool) -> Vec<String> {
        let extensions = path_extensions(ctx.env);
        let operand = self.vfs_operand(name);
        let name = operand.as_ref();
        // A path-shaped operand names one file (`./tool`, `/tools/tool`).
        if name.contains('/') {
            let (dir, file) = split_operand(name, ctx.cwd);
            return self
                .probe(&dir, &file, &extensions, &ctx.fs)
                .await
                .into_iter()
                .collect();
        }
        let path = ctx
            .variables
            .get("PATH")
            .or_else(|| ctx.env.get("PATH"))
            .map(String::as_str);
        let mut found = Vec::new();
        for dir in path_dirs(path, ctx.cwd) {
            if let Some(file) = self.probe(&dir, name, &extensions, &ctx.fs).await {
                found.push(file);
                if !all {
                    break;
                }
            }
        }
        // Last in search order is the host search a bridged command spawns
        // through: it still reaches what no mount covers (`/usr/bin` on
        // Windows). `-a` lists it too, minus a file the VFS already found under
        // another spelling of the same file (`/tmp/x` vs `/c/…/Temp/x`).
        if (found.is_empty() || all)
            && let Some(file) = self.host_file(name, path, ctx)
            && !found
                .iter()
                .any(|seen| same_file(seen, &file, &self.mounts))
        {
            found.push(file);
        }
        found
    }

    /// The first runnable `name` in `dir`, in VFS spelling ([`vfs_join`] keeps
    /// `/` separators, bashkit #2425): the name itself, then its `PATHEXT`
    /// spellings (`npx` → `npx.cmd`) when it has none.
    async fn probe(
        &self,
        dir: &Path,
        name: &str,
        extensions: &[String],
        fs: &Arc<dyn FileSystem>,
    ) -> Option<String> {
        let mut spellings = vec![name.to_string()];
        if Path::new(name).extension().is_none() {
            spellings.extend(extensions.iter().map(|ext| format!("{name}{ext}")));
        }
        for spelling in spellings {
            let candidate = vfs_join(dir, &spelling);
            if is_command_file(&candidate, &self.mounts, fs).await {
                return Some(super::convert_path_to_unix_style(&candidate));
            }
        }
        None
    }

    /// The host executable `name` resolves to, in VFS spelling; `None` when the
    /// host search finds nothing.
    fn host_file(
        &self,
        name: &str,
        path: Option<&str>,
        ctx: &BuiltinContext<'_>,
    ) -> Option<String> {
        // An unmounted cwd has no host answer either — the bridge refused to
        // spawn there ([`HostCommand`]) — and a stand-in cwd would only
        // resolve relative `$PATH` entries against the wrong directory.
        let cwd = resolve_cwd(ctx.cwd, &self.mounts).ok()?;
        let paths = path_lists_for_host(path, &self.mounts);
        let resolved = super::resolve_command(name, &paths, &cwd);
        let host = resolved.to_string_lossy();
        // `resolve_command` echoes a name it could not resolve: a bare name
        // resolves through `$PATH` alone, so the echo means a miss; a path-shaped
        // operand (UNC, root-relative) is echoed only when it is not there.
        if host == name && (is_bare_name(name) || !resolved.is_file()) {
            return None;
        }
        // Windows: the mount table spells a native path best (`E:\x` → `/tmp/x`);
        // the conversion below covers what it does not map (`E:\x` → `/e/x`).
        #[cfg(windows)]
        if let Some(vfs) = host_arg_to_vfs(&host, &self.mounts) {
            return Some(vfs);
        }
        Some(super::convert_path_to_unix_style(&resolved))
    }
}

/// `which` — a file on the script's `$PATH` (a bridged host command reports the
/// executable that runs), else a name the interpreter dispatches itself.
struct WhichBuiltin {
    lookup: Arc<CommandLookup>,
}

#[async_trait]
impl Builtin for WhichBuiltin {
    async fn execute(&self, ctx: BuiltinContext<'_>) -> bashkit::Result<ExecResult> {
        let (options, names) = split_options(ctx.args);
        // Only `-a`/`--all` change the answer; the other options are ignored.
        let all = options.iter().any(|o| matches!(*o, "-a" | "--all"));
        let mut output = String::new();
        let mut all_found = true;
        for name in names {
            let mut files = self.lookup.files(name, &ctx, all).await;
            if files.is_empty() {
                // No file, but the interpreter may run it itself (`json`, a
                // function): report the name, as bashkit's `which` did.
                if self.lookup.dispatched(name).is_none() {
                    all_found = false;
                    continue;
                }
                files.push(name.to_string());
            }
            for file in &files {
                // `-a` prints a line per match; a path prints as the VFS spells it.
                let _ = writeln!(output, "{file}");
            }
        }
        Ok(ExecResult {
            stdout: output.into(),
            exit_code: if all_found { 0 } else { 1 },
            ..Default::default()
        })
    }
}

/// The usage line `type` errors carry.
const TYPE_USAGE: &str = "type [-afptP] name [name ...]";

/// `type` — describe a command: the interpreter's builtins, keywords, and
/// functions, or the file `$PATH` resolves to.
struct TypeBuiltin {
    lookup: Arc<CommandLookup>,
}

#[async_trait]
impl Builtin for TypeBuiltin {
    async fn execute(&self, ctx: BuiltinContext<'_>) -> bashkit::Result<ExecResult> {
        // `-t` kind word, `-p`/`-P` file, `-a` all matches, `-f` no functions.
        let (options, names) = split_options(ctx.args);
        let mut type_only = false;
        let mut path_only = false;
        let mut show_all = false;
        let mut no_functions = false;
        for option in options {
            for c in option[1..].chars() {
                match c {
                    't' => type_only = true,
                    'p' | 'P' => path_only = true,
                    'a' => show_all = true,
                    'f' => no_functions = true,
                    _ => {
                        return Ok(ExecResult::err(
                            format!(
                                "bash: type: -{c}: invalid option\ntype: usage: {TYPE_USAGE}\n"
                            ),
                            1,
                        ));
                    }
                }
            }
        }
        if names.is_empty() {
            return Ok(ExecResult::err(
                format!("bash: type: usage: {TYPE_USAGE}\n"),
                1,
            ));
        }

        let mut output = String::new();
        let mut errors = String::new();
        let mut all_found = true;
        for name in names {
            let mut kinds: Vec<CommandKind> = Vec::new();
            // `-p`/`-P` take the file alone (`-f` drops functions).
            if !path_only
                && let Some(kind) = self.lookup.dispatched(name)
                && !(no_functions && matches!(kind, CommandKind::Function))
            {
                kinds.push(kind);
            }
            if kinds.is_empty() || show_all {
                kinds.extend(
                    self.lookup
                        .files(name, &ctx, show_all)
                        .await
                        .into_iter()
                        .map(CommandKind::File),
                );
            }
            let Some(first) = kinds.first() else {
                all_found = false;
                // Bash reports a plain lookup failure only; `-t`/`-p` stay silent.
                if !type_only && !path_only {
                    let _ = writeln!(errors, "bash: type: {name}: not found");
                }
                continue;
            };
            if type_only {
                let _ = writeln!(output, "{}", first.label());
            } else {
                for kind in &kinds {
                    let _ = writeln!(output, "{}", kind.describe(name));
                }
            }
        }

        Ok(ExecResult {
            stdout: output.into(),
            stderr: errors.into(),
            exit_code: if all_found { 0 } else { 1 },
            ..Default::default()
        })
    }
}

/// Split args into option words and operands; `--` ends the options.
fn split_options(args: &[String]) -> (Vec<&str>, Vec<&str>) {
    let mut options = Vec::new();
    let mut operands = Vec::new();
    let mut terminated = false;
    for arg in args {
        if terminated || !arg.starts_with('-') || arg == "-" {
            operands.push(arg.as_str());
        } else if arg == "--" {
            terminated = true;
        } else {
            options.push(arg.as_str());
        }
    }
    (options, operands)
}

/// `PATH` directories in order; a relative entry resolves against the cwd.
fn path_dirs(path: Option<&str>, cwd: &Path) -> Vec<PathBuf> {
    let Some(path) = path else {
        return Vec::new();
    };
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| vfs_join(cwd, dir))
        .collect()
}

/// Extensions probed on Windows (`cargo` → `cargo.exe`), from the script's
/// `PATHEXT`; empty on Unix, where a name is the file name.
fn path_extensions(env: &HashMap<String, String>) -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    env.get("PATHEXT")
        .map_or(".COM;.EXE;.BAT;.CMD", String::as_str)
        .split(';')
        .filter(|ext| ext.len() > 1 && ext.starts_with('.'))
        .map(str::to_ascii_lowercase) // the filesystem ignores case
        .collect()
}

/// Whether the VFS candidate is a runnable file: inside a mount, not a directory,
/// and executable (Windows: the host resolver decides instead).
async fn is_command_file(vfs: &Path, mounts: &[RealMount], fs: &Arc<dyn FileSystem>) -> bool {
    // A memory-only VFS file has nothing to spawn.
    let Some(host) = resolve_vfs(vfs, mounts) else {
        return false;
    };
    let Ok(meta) = fs.stat(vfs).await else {
        return false;
    };
    if meta.file_type.is_dir() {
        return false;
    }
    // The default rootfs answers `/bin`/`/usr/bin` with a per-builtin stub;
    // without the host file behind it, there is nothing to spawn.
    if !host.is_file() {
        return false;
    }
    if cfg!(windows) {
        // `which_in` applies the rule `PATH` probing uses for an absolute path:
        // a real executable image — the VFS cannot tell a PE from data. It also
        // appends `PATHEXT` spellings and re-cases the match from the directory
        // listing, so compare names: an extensionless file is a command only when
        // its own image runs, not a sibling's.
        which::which_in(&host, None::<&str>, Path::new(".")).is_ok_and(|found| {
            match (found.file_name(), host.file_name()) {
                (Some(found), Some(candidate)) => {
                    host_eq(&found.to_string_lossy(), &candidate.to_string_lossy())
                }
                _ => false,
            }
        })
    } else {
        meta.mode & 0o111 != 0
    }
}

/// Whether `name` is a plain command name — no path separator, so `$PATH` is the
/// only place it resolves; `which_in` resolves every other shape as a path.
fn is_bare_name(name: &str) -> bool {
    if name.contains('/') {
        return false;
    }
    #[cfg(windows)]
    if name.contains('\\') {
        return false;
    }
    true
}

/// Whether two VFS spellings name the same file: the mount table can reach one
/// file from several roots (`/tmp/x` and `/c/…/Temp/x`).
fn same_file(a: &str, b: &str, mounts: &[RealMount]) -> bool {
    if a == b {
        return true;
    }
    match (
        resolve_vfs(Path::new(a), mounts),
        resolve_vfs(Path::new(b), mounts),
    ) {
        (Some(a), Some(b)) => host_eq(&a.to_string_lossy(), &b.to_string_lossy()),
        _ => false,
    }
}

/// Whether two host-visible names are the same; the host filesystem ignores
/// case on Windows.
fn host_eq(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Directory and file name of a path-shaped operand (`./tool`, `/tools/tool`);
/// a relative directory resolves against the cwd.
fn split_operand(name: &str, cwd: &Path) -> (PathBuf, String) {
    let path = Path::new(name);
    let file = path
        .file_name()
        .map(|file| file.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = match path.parent() {
        Some(parent) if parent.has_root() => parent.to_path_buf(),
        Some(parent) if !parent.as_os_str().is_empty() => vfs_join(cwd, parent),
        _ => cwd.to_path_buf(),
    };
    (dir, file)
}

/// Whether `name` is a shell keyword — a copy of bashkit's private
/// `interpreter::is_keyword`, so `type` agrees with the dispatch.
fn is_keyword(name: &str) -> bool {
    matches!(
        name,
        "if" | "then"
            | "else"
            | "elif"
            | "fi"
            | "for"
            | "while"
            | "until"
            | "do"
            | "done"
            | "case"
            | "esac"
            | "in"
            | "function"
            | "select"
            | "time"
            | "{"
            | "}"
            | "[["
            | "]]"
            | "!"
    )
}

/// Match a VFS path against a mount's VFS prefix; returns the corresponding
/// host path when the prefix matches, `None` otherwise.
fn match_mount(vfs_cwd: &Path, vfs_prefix: &str, host_root: &Path) -> Option<PathBuf> {
    let mut components = vfs_cwd.components();
    if components.next() != Some(Component::RootDir) {
        return None;
    }
    let prefix_parts: Vec<&str> = vfs_prefix.split('/').filter(|p| !p.is_empty()).collect();
    for expected in &prefix_parts {
        match components.next() {
            Some(Component::Normal(actual)) if actual == *expected => continue,
            _ => return None,
        }
    }
    Some(host_root.join(components.as_path()))
}

/// Translate the bashkit VFS cwd to the real host path via the mount table.
fn resolve_cwd(vfs_cwd: &Path, mount_specs: &[RealMount]) -> Result<PathBuf, String> {
    resolve_vfs(vfs_cwd, mount_specs)
        .ok_or_else(|| format!("host command outside mapped cwd: {}", vfs_cwd.display()))
}

/// Resolve a VFS absolute path through the mount table; `None` when unmapped.
fn resolve_vfs(vfs: &Path, mount_specs: &[RealMount]) -> Option<PathBuf> {
    mount_specs
        .iter()
        .find_map(|m| match_mount(vfs, &m.vfs_path.to_string_lossy(), &m.host_path))
}

/// Translate the interpreter's MSYS-style `PATH` back to the native form a host
/// child needs — MSYS2 does the same for its own native children.
#[cfg(windows)]
fn path_list_for_host(value: &str, mounts: &[RealMount]) -> String {
    super::map_path_list(value, ";", |entry| convert_posix_arg(entry, mounts))
}

/// Native form on Unix: the interpreter's `PATH` already is the host's own.
#[cfg(not(windows))]
fn path_list_for_host(value: &str, _mounts: &[RealMount]) -> String {
    value.to_string()
}

/// `PATH` lists a command is resolved through: the script's own, then the
/// host's — the system environment.
fn path_lists_for_host(path: Option<&str>, mounts: &[RealMount]) -> Vec<String> {
    path.map(|path| path_list_for_host(path, mounts))
        .into_iter()
        .chain(super::host_path_lists(None))
        .collect()
}

/// Rewrite VFS absolute paths in host-command args to native paths,
/// MSYS2-style; identity on Unix, where VFS paths are the real paths.
#[cfg(windows)]
fn convert_args_for_host(args: &[String], mounts: &[RealMount]) -> Vec<String> {
    args.iter()
        .map(|arg| convert_arg_for_host(arg, mounts))
        .collect()
}

/// Identity on Unix: VFS paths are already the real host paths, so the args
/// pass through without copying.
#[cfg(not(windows))]
fn convert_args_for_host<'a>(args: &'a [String], _mounts: &[RealMount]) -> &'a [String] {
    args
}

/// Convert one argument: standalone POSIX absolute paths and the value part
/// of `--opt=<path>` forms; everything else passes through untouched.
#[cfg(windows)]
fn convert_arg_for_host(arg: &str, mounts: &[RealMount]) -> String {
    // Quoted globs stay VFS-shaped: the pattern matches VFS names, not host paths.
    if arg.contains(['*', '?']) {
        return arg.to_string();
    }
    match arg.split_once('=') {
        Some((flag, value)) if flag.starts_with("--") => {
            format!("{flag}={}", convert_posix_arg(value, mounts))
        }
        _ => convert_posix_arg(arg, mounts),
    }
}

/// Convert a POSIX absolute path to its host form; other strings unchanged.
#[cfg(windows)]
fn convert_posix_arg(value: &str, mounts: &[RealMount]) -> String {
    // Drive-letter paths (`/d/...`) convert directly, MSYS2-style: a native
    // tool never accepts `/d/...`. UNC and other absolutes follow below.
    if let Some(win) = value
        .strip_prefix('/')
        .and_then(super::drive_style_to_windows)
    {
        return win;
    }
    if value.starts_with('/') {
        if let Some(host) = resolve_vfs(Path::new(value), mounts) {
            return host.to_string_lossy().into_owned();
        }
        if value.starts_with("//") {
            // MSYS2 keeps both slashes: `//d/x` is UNC `\\d\x` (server `d`,
            // share `x`), NOT the drive form `/d/x` — verified against the
            // MSYS2 runtime itself (`cygpath -w //d/x` → `\\d\x`).
            return value.replace('/', "\\");
        }
    }
    value.to_string()
}

/// Rewrite host-style paths in builtin args to VFS form. A string is replaced
/// only when EVERY remaining occurrence is a convertible builtin arg — the
/// census counts arg occurrences, so heredoc bodies, comments, and protected
/// positions (patterns, output text, bridged commands) skip the string.
/// Longest first: rewriting `E:/x/y` removes the occurrence of `E:/x` inside
/// it, so a shorter arg that is a string prefix of a longer one still
/// converts (`cat E:/x/y E:/x`).
#[cfg(windows)]
fn rewrite_host_paths<'a>(
    script: &'a str,
    analysis: &ScriptAnalysis,
    mounts: &[RealMount],
) -> Cow<'a, str> {
    // Fast path: every convertible host path contains a drive `:`.
    if !script.contains(':') {
        return Cow::Borrowed(script);
    }
    let mut convertible: Vec<_> = classify_args(analysis)
        .into_iter()
        .filter(|(_, (class, _))| *class == Convert)
        .collect();
    convertible.sort_by_key(|(arg, _)| std::cmp::Reverse(arg.len()));
    let mut rewritten = script.to_string();
    for (arg, (_, count)) in convertible {
        if rewritten.matches(&arg).count() != count {
            continue;
        }
        if let Some(vfs) = host_arg_to_vfs(&arg, mounts) {
            rewritten = rewritten.replace(&arg, &vfs);
        }
    }
    if rewritten == script {
        Cow::Borrowed(script)
    } else {
        Cow::Owned(rewritten)
    }
}

/// Identity on Unix: VFS paths are the real host paths, so nothing rewrites.
#[cfg(not(windows))]
fn rewrite_host_paths<'a>(
    script: &'a str,
    _analysis: &ScriptAnalysis,
    _mounts: &[RealMount],
) -> Cow<'a, str> {
    Cow::Borrowed(script)
}

/// Per-string census: class (`Protect` wins) and occurrence count.
#[cfg(windows)]
type ArgCensus = HashMap<String, (ArgClass, usize)>;

/// Per-argument classification for the rewrite.
#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum ArgClass {
    /// May be rewritten to VFS form (host-shaped builtin file operand).
    Convert,
    /// Never rewritten.
    Protect,
}

#[cfg(windows)]
use ArgClass::{Convert, Protect};

/// Census of every literal argument: `Protect` on any occurrence wins.
#[cfg(windows)]
fn classify_args(analysis: &ScriptAnalysis) -> ArgCensus {
    let mut census = HashMap::new();
    for command in &analysis.commands {
        let Some(name) = command.name.as_deref() else {
            continue; // dynamic names already made the analysis opaque
        };
        // Bridged host commands keep native paths; protect-all builtins take
        // no file operands.
        if !is_interpreter_builtin(name) || PROTECT_ALL_BUILTINS.contains(&name) {
            for arg in command.args.iter().flatten() {
                add_census(&mut census, arg, Protect);
            }
        } else if name == "find" {
            classify_find_args(command, &mut census);
        } else if let Some(opts) = pattern_first_opts(name) {
            classify_pattern_first_args(command, &opts, &mut census);
        } else {
            classify_convert_all_args(command, &mut census);
        }
    }
    // Redirects are opened by the interpreter itself (VFS), even for bridged
    // commands, so their paths always convert.
    for redirect in &analysis.redirects {
        if let Some(path) = &redirect.path {
            add_census(&mut census, path, Convert);
        }
    }
    census
}

/// Record one literal argument occurrence; `Protect` on any occurrence wins.
#[cfg(windows)]
fn add_census(census: &mut ArgCensus, arg: &str, class: ArgClass) {
    let entry = census.entry(arg.to_string()).or_insert((Convert, 0));
    entry.1 += 1;
    if class == Protect {
        entry.0 = Protect;
    }
}

/// Builtins whose args are output/format text, variable handling, or wrapped
/// commands — never rewritten, even when host-path-shaped.
#[cfg(windows)]
const PROTECT_ALL_BUILTINS: &[&str] = &[
    "echo", "printf", "test", "[", "expr", // output/format text
    // Wrappers: args carry bridged commands.
    "env", "timeout", "xargs", "nice", "sudo", "flock", "busybox",
    // (`watch`/`parallel` need no entry: their stubs fail analysis first)
    "read", "declare", "typeset", "local", "export", "unset", "set", "shopt", "return", "exit",
    "wait", "kill", "sleep", "seq", "true", "false", "shift", "let", "alias", "unalias", "type",
    "hash", "help", "history", "umask", "ulimit", // variables & flow control
];

/// Option table of a pattern-first builtin: the first bare operand is the
/// pattern/program (protected), later operands are files (converted).
#[cfg(windows)]
struct PatternFirstOpts {
    /// `(flag, class)` — the next arg is the flag's value (pattern source,
    /// non-path value, or file).
    value: &'static [(&'static str, ArgClass)],
    /// `(prefix, class)` — attached value (`--flag=<value>`).
    attached: &'static [(&'static str, ArgClass)],
    /// Of the above, the flags supplying the pattern/program: once one is
    /// seen, the first bare operand is a file.
    program: &'static [&'static str],
}

#[cfg(windows)]
fn pattern_first_opts(name: &str) -> Option<PatternFirstOpts> {
    let opts = match name {
        "grep" | "rg" => PatternFirstOpts {
            value: &[
                ("-e", Protect),
                ("--regexp", Protect),
                ("-A", Protect),
                ("-B", Protect),
                ("-C", Protect),
                ("-m", Protect),
                ("--after-context", Protect),
                ("--before-context", Protect),
                ("--context", Protect),
                ("--max-count", Protect),
                ("-f", Convert),
                ("--file", Convert),
            ],
            attached: &[
                ("--regexp=", Protect),
                ("--include=", Protect),
                ("--exclude=", Protect),
                ("--exclude-dir=", Protect),
                ("--color=", Protect),
                ("--file=", Convert),
            ],
            program: &["-e", "--regexp", "-f", "--file", "--regexp=", "--file="],
        },
        "sed" => PatternFirstOpts {
            value: &[
                ("-e", Protect),
                ("--expression", Protect),
                ("-f", Convert),
                ("--file", Convert),
            ],
            attached: &[
                ("--expression=", Protect),
                ("--in-place=", Protect),
                ("--file=", Convert),
            ],
            program: &[
                "-e",
                "--expression",
                "-f",
                "--file",
                "--expression=",
                "--file=",
            ],
        },
        "awk" => PatternFirstOpts {
            value: &[
                ("-F", Protect),
                ("-v", Protect),
                ("--assign", Protect),
                ("-f", Convert),
                ("--file", Convert),
            ],
            attached: &[("--field-separator=", Protect), ("--file=", Convert)],
            program: &["-f", "--file", "--file="],
        },
        // No `jq` arm: the jq feature is off in this build, so jq is bridged
        // (args protected) — re-add if the feature is ever enabled.
        _ => return None,
    };
    Some(opts)
}

/// Scan a pattern-first builtin's args: flags consume their values with the
/// flag's class; the first bare operand is the pattern/program, later bare
/// operands are files.
#[cfg(windows)]
fn classify_pattern_first_args(
    command: &AnalyzedCommand,
    opts: &PatternFirstOpts,
    census: &mut ArgCensus,
) {
    let mut program_supplied = false;
    let mut operand_seen = false;
    let args = command.args.as_slice();
    let mut i = 0;
    while i < args.len() {
        let Some(arg) = args[i].as_deref() else {
            i += 1;
            continue;
        };
        if let Some((_, class)) = opts.value.iter().find(|(flag, _)| *flag == arg) {
            program_supplied |= opts.program.contains(&arg);
            if let Some(value) = args.get(i + 1).and_then(|a| a.as_deref()) {
                add_census(census, value, *class);
            }
            i += 2;
            continue;
        }
        if let Some((prefix, class)) = opts
            .attached
            .iter()
            .find(|(prefix, _)| arg.starts_with(prefix))
        {
            program_supplied |= opts.program.contains(prefix);
            add_census(census, &arg[prefix.len()..], *class);
            i += 1;
            continue;
        }
        if arg.starts_with('-') {
            i += 1; // plain flag
            continue;
        }
        // Bare operand: the first is the pattern/program, later ones are
        // files — unless a pattern flag already supplied it.
        let class = if operand_seen || program_supplied {
            Convert
        } else {
            Protect
        };
        operand_seen = true;
        add_census(census, arg, class);
        i += 1;
    }
}

/// `find [PATH]... [EXPRESSION]` — leading bare operands are search paths
/// (converted); from the first flag on, args are predicates, patterns, or
/// `-exec` templates for bridged commands (protected).
#[cfg(windows)]
fn classify_find_args(command: &AnalyzedCommand, census: &mut ArgCensus) {
    let mut expression = false;
    for arg in command.args.iter().flatten() {
        if expression || arg.starts_with('-') || matches!(arg.as_str(), "!" | "(" | ")") {
            expression = true;
            add_census(census, arg, Protect);
        } else {
            add_census(census, arg, Convert);
        }
    }
}

/// Attached-value flags whose value is a file path (`--file=<path>`, …).
#[cfg(windows)]
const ATTACHED_PATH_FLAGS: &[&str] = &[
    "--file=",
    "--files-from=",
    "--exclude-from=",
    "--output=",
    "--log-file=",
];

/// `key=value` and `--flag=value` args are data (protected), except the
/// attached path-flag allowlist above, whose value converts.
#[cfg(windows)]
fn classify_convert_all_args(command: &AnalyzedCommand, census: &mut ArgCensus) {
    for arg in command.args.iter().flatten() {
        if let Some(value) = ATTACHED_PATH_FLAGS.iter().find_map(|f| arg.strip_prefix(f)) {
            add_census(census, value, Convert);
        } else if arg.contains('=') {
            add_census(census, arg, Protect);
        } else {
            add_census(census, arg, Convert);
        }
    }
}

/// VFS spelling of a host-style path arg: deepest mount-table match first
/// (canonical case and routing), MSYS drive form (`C:\x` → `/c/x`) as
/// fallback. `None` for non-host shapes and unmounted UNC paths.
#[cfg(windows)]
fn host_arg_to_vfs(arg: &str, mounts: &[RealMount]) -> Option<String> {
    let drive = super::is_drive_path(arg);
    if !drive && !arg.starts_with("\\\\") {
        return None; // VFS-shaped, relative, or otherwise not a host path
    }
    if let Some(vfs) = mounted_vfs(arg, mounts) {
        return Some(vfs);
    }
    // Unmounted drives still convert, MSYS-style; unmounted UNC has no VFS
    // spelling. `convert_path_to_unix_style` also maps verbatim `\\?\C:\`.
    drive.then(|| super::convert_path_to_unix_style(Path::new(arg)))
}

/// Deepest case-insensitive mount match for a host path; returns the VFS
/// spelling (canonical mount path + original-case remainder).
#[cfg(windows)]
fn mounted_vfs(arg: &str, mounts: &[RealMount]) -> Option<String> {
    let lower = arg.to_ascii_lowercase().replace('\\', "/");
    let mut best: Option<(&RealMount, usize, &str)> = None;
    for mount in mounts {
        let host = mount
            .host_path
            .to_string_lossy()
            .to_ascii_lowercase()
            .replace('\\', "/");
        let host = host.trim_end_matches('/');
        let Some(rest) = lower.strip_prefix(host) else {
            continue;
        };
        if !rest.is_empty() && !rest.starts_with('/') {
            continue; // component boundary: `d:\tmp2` is not under `d:\tmp`
        }
        if best.is_none_or(|(_, depth, _)| host.len() > depth) {
            best = Some((mount, host.len(), &arg[host.len()..]));
        }
    }
    let (mount, _, rest) = best?;
    let rest = rest.trim_start_matches(['/', '\\']);
    if rest.is_empty() {
        return Some(mount.vfs_path.to_string_lossy().into_owned());
    }
    Some(format!(
        "{}/{}",
        mount.vfs_path.to_string_lossy(),
        rest.replace('\\', "/")
    ))
}

/// VFS mount table in match order (specific mounts before broad ones).
/// Single source of truth for `build_bash` and `HostCommand`.
fn mounts(workspace: &Path, home: Option<&RealMount>) -> Vec<RealMount> {
    let mut list = vec![
        RealMount::rw(workspace, super::convert_path_to_unix_style(workspace)),
        // Shared tmp dir: the file tools and the `bash -c` fallback agree here.
        RealMount::rw(super::tmp_host_dir(), "/tmp"),
    ];
    if let Some(home) = home {
        list.insert(1, home.clone());
    }
    list.extend(readonly_roots());
    list
}

/// Read-only fallback mounts, appended last: `/` on Unix, every present
/// drive at its drive-letter path (`/c`, `/d`, …) on Windows — no `/`
/// catch-all there, so unmapped paths error instead of being mangled.
#[cfg(unix)]
fn readonly_roots() -> Vec<RealMount> {
    vec![RealMount::ro("/", "/")]
}

/// Every present drive mounted read-only at its drive-letter VFS path
/// (`C:\` → `/c`), like `convert_path_to_unix_style` produces.
#[cfg(windows)]
fn readonly_roots() -> Vec<RealMount> {
    present_drive_letters()
        .into_iter()
        .map(|letter| {
            RealMount::ro(
                format!("{letter}:\\"),
                format!("/{}", letter.to_ascii_lowercase()),
            )
        })
        .collect()
}

/// Letters of every drive present on this host with a usable root.
/// `GetDriveTypeW` never probes media, so drives that still fail to open
/// (empty card readers, stale network shares) are filtered by bashkit's
/// build-time canonicalize.
#[cfg(windows)]
fn present_drive_letters() -> Vec<char> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};

    const DRIVE_UNKNOWN: u32 = 0;
    const DRIVE_NO_ROOT_DIR: u32 = 1;
    // SAFETY: GetLogicalDrives takes no arguments; bit i is drive 'A' + i.
    let mask = unsafe { GetLogicalDrives() };
    (0..26)
        .filter(|&i| mask & (1 << i) != 0)
        .map(|i| char::from(b'A' + i as u8))
        .filter(|&letter| {
            let root = format!("{letter}:\\");
            let wide: Vec<u16> = root.encode_utf16().chain([0]).collect();
            // SAFETY: `root` is a valid drive root; the result is a plain u32.
            let kind = unsafe { GetDriveTypeW(wide.as_ptr()) };
            !matches!(kind, DRIVE_UNKNOWN | DRIVE_NO_ROOT_DIR)
        })
        .collect()
}

/// A real host directory mounted into the VFS (see [`mounts`]).
#[derive(Clone)]
struct RealMount {
    host_path: PathBuf,
    vfs_path: PathBuf,
    writable: bool,
}

impl RealMount {
    fn rw(host_path: impl Into<PathBuf>, vfs_path: impl Into<PathBuf>) -> Self {
        Self {
            host_path: host_path.into(),
            vfs_path: vfs_path.into(),
            writable: true,
        }
    }

    fn ro(host_path: impl Into<PathBuf>, vfs_path: impl Into<PathBuf>) -> Self {
        Self {
            host_path: host_path.into(),
            vfs_path: vfs_path.into(),
            writable: false,
        }
    }
}

/// Resolve the real host home directory and its POSIX VFS mount path.
fn real_home_mount() -> Option<RealMount> {
    let host_home = home_host_path()?;
    let vfs_path = super::convert_path_to_unix_style(&host_home);
    Some(RealMount::rw(host_home, vfs_path))
}

/// Real host username: `USER` (Unix/MSYS) or `USERNAME` (native Windows).
fn real_username() -> Option<String> {
    ["USER", "USERNAME"]
        .into_iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
}

/// Real host hostname via the OS (`gethostname` / `GetComputerNameW`).
pub(crate) fn real_hostname() -> Option<String> {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .filter(|h| !h.is_empty())
}

/// Bash's `OSTYPE` and the vendor+OS half of `MACHTYPE` for the host
/// (`msys`/`pc-msys` on Windows, `darwin`/`apple-darwin` on macOS, …).
fn platform_labels() -> (&'static str, &'static str) {
    match std::env::consts::OS {
        "windows" => ("msys", "pc-msys"),
        "macos" => ("darwin", "apple-darwin"),
        "linux" => ("linux-gnu", "pc-linux-gnu"),
        "freebsd" => ("freebsd", "pc-freebsd"),
        _ => ("unknown", "unknown"),
    }
}

/// Resolve the real host home directory, preferring `$HOME` then `USERPROFILE`.
fn home_host_path() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        // Git Bash exports HOME as an MSYS path like `/c/Users/...`.
        #[cfg(windows)]
        if let Some(win) = super::convert_path_to_windows_style(&home)
            && win.is_dir()
        {
            return Some(win);
        }
        let p = PathBuf::from(&home);
        if p.is_absolute() && p.is_dir() {
            return Some(p);
        }
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let p = PathBuf::from(&profile);
        if p.is_absolute() && p.is_dir() {
            return Some(p);
        }
    }
    None
}

/// Format an `ExecResult` like `format_command_output`, using the raw bytes
/// (`StreamData`'s `Deref` already ran `from_utf8_lossy`) and surfacing
/// bashkit's head truncation so the final marker stays accurate.
fn format_exec_result(result: &ExecResult) -> String {
    let stdout = super::decode_plain(result.stdout.as_bytes());
    let stderr = super::decode_plain(result.stderr.as_bytes());
    let mut output = super::combine_output(&stdout, &stderr, result.exit_code);
    if result.stdout_truncated || result.stderr_truncated {
        if !output.is_empty() {
            output.push('\n');
        }
        let _ = std::fmt::Write::write_fmt(
            &mut output,
            format_args!(
                "[bashkit truncated output at {MAX_STREAM_BYTES} bytes per stream — tail lost]"
            ),
        );
    }
    super::truncate_output(output)
}

/// Drain a captured-stderr pipe, re-emitting everything except bashkit
/// warnings expected by design: writable mounts (cannot be silenced) and
/// drive roots that failed to mount (unreadable media probed every build).
fn drain_and_reemit(mut file: std::fs::File) {
    const EXPECTED: [&str; 2] = [
        "bashkit: warning: writable mount",
        "bashkit: warning: failed to canonicalize mount path",
    ];
    let mut buf = Vec::new();
    let _ = std::io::Read::read_to_end(&mut file, &mut buf);
    for line in String::from_utf8_lossy(&buf).lines() {
        if !EXPECTED.iter().any(|p| line.starts_with(*p)) {
            eprintln!("{line}");
        }
    }
}

/// Swap stderr to a pipe during `Bash::build`; on drop, restore stderr and
/// re-emit the captured output via `drain_and_reemit`.
#[cfg(unix)]
mod stderr_silencer {
    use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};

    pub(super) struct StderrSilencer {
        saved: Option<OwnedFd>,
        read_end: Option<OwnedFd>,
    }

    impl StderrSilencer {
        pub(super) fn new() -> Self {
            let mut fds = [0i32; 2];
            // SAFETY: pipe() creates two fresh fds, wrapped in OwnedFd at once.
            let (read_end, write_end) = if unsafe { libc::pipe(fds.as_mut_ptr()) } == 0 {
                unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
            } else {
                return Self {
                    saved: None,
                    read_end: None,
                };
            };
            // SAFETY: dup() duplicates the stderr fd.
            let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
            if saved < 0 {
                return Self {
                    saved: None,
                    read_end: None,
                }; // write_end closed here
            }
            // SAFETY: saved is a fresh fd, now owned.
            let saved = unsafe { OwnedFd::from_raw_fd(saved) };
            // SAFETY: dup2 on raw fds; STDERR_FILENO then holds a dup of the
            // write end, so dropping `write_end` keeps the pipe open.
            unsafe { libc::dup2(write_end.as_raw_fd(), libc::STDERR_FILENO) };
            drop(write_end);
            Self {
                saved: Some(saved),
                read_end: Some(read_end),
            }
        }
    }

    impl Drop for StderrSilencer {
        fn drop(&mut self) {
            // Restore stderr — this dup2 also closes the pipe write end held
            // in STDERR_FILENO, so the drain below reaches EOF.
            if let Some(saved) = self.saved.take() {
                // SAFETY: saved is the fd captured in `new`.
                unsafe { libc::dup2(saved.as_raw_fd(), libc::STDERR_FILENO) };
                // saved (OwnedFd) dropped here → closes the duplicate.
            }
            if let Some(read_end) = self.read_end.take() {
                super::drain_and_reemit(std::fs::File::from(read_end));
            }
        }
    }
}

/// Same as the Unix variant via `SetStdHandle` — std re-queries
/// `STD_ERROR_HANDLE` per write, so the swap takes effect immediately.
#[cfg(windows)]
mod stderr_silencer {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, SetStdHandle};
    use windows_sys::Win32::System::Pipes::CreatePipe;

    pub(super) struct StderrSilencer {
        saved: Option<HANDLE>,
        read_end: Option<OwnedHandle>,
        /// Held open during `build()`; closed on drop before reading the pipe.
        write_end: Option<OwnedHandle>,
    }

    impl StderrSilencer {
        pub(super) fn new() -> Self {
            let mut read: HANDLE = std::ptr::null_mut();
            let mut write: HANDLE = std::ptr::null_mut();
            // SAFETY: CreatePipe with valid out-pointers; null attrs → defaults.
            if unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), 0) } == 0 {
                return Self {
                    saved: None,
                    read_end: None,
                    write_end: None,
                };
            }
            // SAFETY: handles are freshly created, wrapped in OwnedHandle.
            let read_end = unsafe { OwnedHandle::from_raw_handle(read) };
            let write_end = unsafe { OwnedHandle::from_raw_handle(write) };
            // SAFETY: GetStdHandle/SetStdHandle with valid constants.
            let saved = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
            unsafe { SetStdHandle(STD_ERROR_HANDLE, write_end.as_raw_handle()) };
            Self {
                saved: Some(saved),
                read_end: Some(read_end),
                write_end: Some(write_end),
            }
        }
    }

    impl Drop for StderrSilencer {
        fn drop(&mut self) {
            // Restore stderr first, then close the write end so reads reach EOF.
            if let Some(saved) = self.saved.take() {
                // SAFETY: restores the handle captured in `new`.
                unsafe { SetStdHandle(STD_ERROR_HANDLE, saved) };
            }
            self.write_end.take();
            if let Some(read_end) = self.read_end.take() {
                super::drain_and_reemit(std::fs::File::from(read_end));
            }
        }
    }
}
