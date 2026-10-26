//! `uname` for the in-process interpreter.
//!
//! bashkit's builtin reports a virtual `Linux … sandbox` platform, so the host's
//! own binary is bridged instead — on Unix it is always on `PATH`, and on
//! Windows the one a real bash runs is located too (MSYS's). A Windows host with
//! no binary at all answers from its own facts.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::host_path_lists;

#[cfg(windows)]
use super::{detach_child, resolve_command};
#[cfg(windows)]
use std::process::Stdio;

/// The host's own `uname`, resolved through the host `PATH`; Windows probes the
/// MSYS binary a real bash runs next (see [`msys_uname`]).
pub(crate) fn host_uname() -> Option<&'static Path> {
    static UNAME: OnceLock<Option<PathBuf>> = OnceLock::new();
    UNAME
        .get_or_init(|| {
            let lists = host_path_lists(None);
            let found = which::which_in("uname", lists.first(), Path::new(".")).ok();
            #[cfg(windows)]
            let found = found.or_else(msys_uname);
            found
        })
        .as_deref()
}

/// The `uname` a real bash runs: `usr\bin\uname.exe` of the root its `bash`
/// belongs to (`Git\bin\bash.exe` → `Git\usr\bin\uname.exe`), which the MSYS
/// runtime puts on bash's own `PATH` as `/usr/bin` — out of reach above.
#[cfg(windows)]
fn msys_uname() -> Option<PathBuf> {
    let bin = host_bash()?.parent()?.to_path_buf(); // `<root>\bin` or `<root>\usr\bin`
    [bin.clone(), bin.join("../usr/bin")]
        .into_iter()
        .flat_map(|dir| ["uname.exe", "uname"].map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

/// The host `bash` a `bash -c` fallback would run, when it exists.
#[cfg(windows)]
fn host_bash() -> Option<PathBuf> {
    let bash = resolve_command("bash", &host_path_lists(None), Path::new("."));
    bash.is_file().then_some(bash)
}

/// The `MSYSTEM` a real bash exports (`MINGW64`, `MSYS`, …), probed once.
///
/// MSYS binaries key their identity off it, so a bridged one reports what
/// `/usr/bin/…` reports only when it sees the same value.
#[cfg(windows)]
pub(crate) fn host_msystem() -> Option<&'static str> {
    static MSYSTEM: OnceLock<Option<String>> = OnceLock::new();
    MSYSTEM
        .get_or_init(|| {
            if let Ok(value) = std::env::var("MSYSTEM") {
                return (!value.is_empty()).then_some(value);
            }
            // Git for Windows' bash sets it itself, even for `--noprofile`.
            let mut cmd = std::process::Command::new(host_bash()?);
            cmd.args(["--noprofile", "--norc", "-c", "printf %s \"$MSYSTEM\""])
                .stdin(Stdio::null())
                .stderr(Stdio::null());
            detach_child(&mut cmd);
            let output = cmd.output().ok()?;
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (!value.is_empty()).then_some(value)
        })
        .as_deref()
}

#[cfg(windows)]
pub(crate) use fallback::Uname;

/// `uname` on a Windows host that has no binary to run: the host's own facts, in
/// GNU's option syntax. Windows-only — Unix always has `uname` on `PATH`.
#[cfg(windows)]
mod fallback {
    use std::sync::OnceLock;

    use bashkit::{Command, CommandContext, CommandStatus, ExecResult, async_trait};

    use super::super::bash_kit::{HostCommand, real_hostname};
    use super::host_msystem;

    /// `uname` — the host's own binary where there is one, host facts otherwise.
    pub(crate) struct Uname {
        /// The bridged host command; `None` answers from [`host_facts`].
        host: Option<HostCommand>,
    }

    impl Uname {
        /// `host` is the bridged host binary, when one was found.
        pub(crate) fn new(host: Option<HostCommand>) -> Self {
            Self { host }
        }
    }

    #[async_trait]
    impl Command for Uname {
        async fn execute(&self, ctx: CommandContext<'_>) -> bashkit::Result<CommandStatus> {
            match &self.host {
                Some(host) => host.execute(ctx).await,
                None => {
                    let result = answer(ctx.args, &host_facts());
                    // Empty writes fail on a closed (`n>&-`) destination.
                    if !result.stdout.is_empty() {
                        ctx.stdout.write_all(result.stdout.as_bytes()).await?;
                    }
                    if !result.stderr.is_empty() {
                        ctx.stderr.write_all(result.stderr.as_bytes()).await?;
                    }
                    Ok(CommandStatus::with_code(result.exit_code))
                }
            }
        }
    }

    /// The help text `uname --help` prints.
    const HELP: &str = "Usage: uname [OPTION]...\nPrint host system information.\n\n  -a, --all\t\t\tprint all information\n  -s, --kernel-name\t\tprint the kernel name\n  -n, --nodename\t\tprint the network node hostname\n  -r, --kernel-release\t\tprint the kernel release\n  -v, --kernel-version\t\tprint the kernel version\n  -m, --machine\t\t\tprint the machine hardware name\n  -o, --operating-system\tprint the operating system\n      --help\tdisplay this help and exit\n      --version\toutput version information and exit\n";

    /// What `uname --version` prints.
    const VERSION: &str = concat!("uname (crabot) ", env!("CARGO_PKG_VERSION"), "\n");

    /// Every field `uname` prints for `-a`, in GNU's order (`-i` repeats `-p`).
    const FIELDS: &str = "snrvmpio";

    /// Answer `uname [OPTION]...` from `facts`, mirroring GNU uname's options.
    fn answer(args: &[String], facts: &Facts) -> ExecResult {
        let mut all = false;
        let mut fields = String::new(); // field letters asked for by name
        let mut operand = None; // rejected once the options are read, like getopt
        let mut options_done = false;
        for arg in args {
            if options_done {
                operand = operand.or(Some(arg));
            } else if arg == "-a" || arg == "--all" {
                all = true;
            } else if arg == "--help" {
                return ExecResult::ok(HELP);
            } else if arg == "--version" {
                return ExecResult::ok(VERSION);
            } else if let Some(long) = arg.strip_prefix("--") {
                if long.is_empty() {
                    options_done = true; // `--` ends the options
                } else if let Some(field) = option_field(long) {
                    select_field(&mut fields, field);
                } else {
                    return invalid_option(arg);
                }
            } else if let Some(bundle) = arg.strip_prefix('-').filter(|b| !b.is_empty()) {
                // A short bundle (`-sn`), one letter at a time.
                for letter in bundle.chars() {
                    if letter == 'a' {
                        all = true;
                    } else if let Some(field) = option_field(&letter.to_string()) {
                        select_field(&mut fields, field);
                    } else {
                        return invalid_option(&format!("-{letter}"));
                    }
                }
            } else {
                operand = operand.or(Some(arg)); // a bare word, or a lone `-`
            }
        }
        if let Some(arg) = operand {
            return extra_operand(arg);
        }
        // `-a` asks for every field, no option at all for the kernel name; the
        // fields print in GNU's own order, whatever order the options came in.
        let asked: String = if all {
            FIELDS.to_string()
        } else if fields.is_empty() {
            "s".to_string()
        } else {
            FIELDS
                .chars()
                .filter(|field| fields.contains(*field))
                .collect()
        };
        let mut parts = Vec::new();
        for field in asked.chars() {
            // `-a` leaves a field the host cannot name out (`uname -p` on MSYS);
            // asking for it by name prints `unknown`, like GNU uname.
            match field_fact(facts, field) {
                Some(fact) => parts.push(fact),
                None if !all && fields.contains(field) => parts.push("unknown"),
                None => {}
            }
        }
        ExecResult::ok(format!("{}\n", parts.join(" ")))
    }

    /// The field a `uname` option asks for, by name without dashes (`s`,
    /// `kernel-name`); `-i`/`--hardware-platform` mean the processor, as in GNU.
    fn option_field(name: &str) -> Option<char> {
        Some(match name {
            "s" | "kernel-name" => 's',
            "n" | "nodename" => 'n',
            "r" | "kernel-release" => 'r',
            "v" | "kernel-version" => 'v',
            "m" | "machine" => 'm',
            "p" | "i" | "processor" | "hardware-platform" => 'p',
            "o" | "operating-system" => 'o',
            _ => return None,
        })
    }

    /// The fact a field letter prints; `None` when the host cannot name one.
    fn field_fact(facts: &Facts, field: char) -> Option<&str> {
        match field {
            's' => Some(&facts.kernel),
            'n' => Some(&facts.nodename),
            'r' => Some(&facts.release),
            'v' => Some(&facts.version),
            'm' => Some(&facts.machine),
            'o' => Some(&facts.os),
            _ => facts.processor.as_deref(), // `-p`
        }
    }

    /// Ask for `field` once.
    fn select_field(fields: &mut String, field: char) {
        if !fields.contains(field) {
            fields.push(field);
        }
    }

    /// GNU-style invalid-option error (bashkit's helper is crate-private).
    fn invalid_option(arg: &str) -> ExecResult {
        let message = match arg.strip_prefix("--") {
            Some(long) => format!("uname: unrecognized option '--{long}'\n"),
            None => {
                let ch = arg.trim_start_matches('-').chars().next().unwrap_or('-');
                format!("uname: invalid option -- '{ch}'\n")
            }
        };
        ExecResult::err(message, 1)
    }

    /// GNU-style error for an argument that is not an option.
    fn extra_operand(arg: &str) -> ExecResult {
        ExecResult::err(format!("uname: extra operand '{arg}'\n"), 1)
    }

    /// What a synthetic `uname` prints: the host's own platform, read without
    /// running a host `uname` binary.
    struct Facts {
        kernel: String,
        nodename: String,
        release: String,
        version: String,
        machine: String,
        /// `None` when the host cannot name a processor.
        processor: Option<String>,
        os: String,
    }

    /// Host facts on Windows, where no `uname` syscall exists: an MSYS-style
    /// identity (crabot's emulated platform) from the NT version, architecture,
    /// and machine name.
    fn host_facts() -> Facts {
        let (major, minor, build) = nt_version();
        let base = format!("{major}.{minor}");
        // `MINGW64_NT-10.0-26300 … 10.0.26300 …`: the build is dropped when the
        // registry could not be read.
        let (release, suffix) = if build.is_empty() {
            (base.clone(), base)
        } else {
            (format!("{base}.{build}"), format!("{base}-{build}"))
        };
        Facts {
            kernel: format!("{}_NT-{suffix}", host_msystem().unwrap_or("MSYS")),
            nodename: real_hostname().unwrap_or_else(|| "localhost".to_string()),
            release,
            version: "unknown".to_string(),
            machine: std::env::consts::ARCH.to_string(),
            processor: None,
            os: "Msys".to_string(),
        }
    }

    /// Windows version and build (`10`, `0`, `26300`) from the registry, read once.
    fn nt_version() -> (u32, u32, String) {
        static VERSION: OnceLock<(u32, u32, String)> = OnceLock::new();
        VERSION
            .get_or_init(|| {
                const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
                let Ok(key) = windows_registry::LOCAL_MACHINE.open(KEY) else {
                    return (10, 0, String::new());
                };
                // Windows 10+ spells its version as these two DWORDs.
                (
                    key.get_u32("CurrentMajorVersionNumber").unwrap_or(10),
                    key.get_u32("CurrentMinorVersionNumber").unwrap_or(0),
                    key.get_string("CurrentBuildNumber").unwrap_or_default(),
                )
            })
            .clone()
    }
}
