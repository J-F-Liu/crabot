# COMMANDS.md

Annotated reference of every command available in the crabot `bash` tool environment.
This environment is a custom bash build (5.2.15) in which all entries — including `json`, `csv`, `http` — are compiled-in builtins, so `compgen -b` (builtins) and `compgen -c` (commands) return the same list of 152 commands.

- Descriptions for standard bash builtins come from `help <cmd>`.
- Descriptions for custom builtins come from their usage strings (run the command bare to see usage).
- `whatis`/`apropos`/man pages are not installed, so well-known tools are described from general knowledge.

## Custom data / utility builtins

| Command    | Description                                                                            |
| ---------- | -------------------------------------------------------------------------------------- |
| `json`     | JSON processor: `get, set, keys, length, type, format, pretty`                         |
| `csv`      | CSV processor: `select, count, headers, filter, sort`                                  |
| `yaml`     | YAML processor: `get, keys, length, type`                                              |
| `tomlq`    | TOML query: `tomlq [-r] [-t] QUERY [FILE]`                                             |
| `http`     | HTTP client: `http [METHOD] URL [ITEMS...]`                                            |
| `dotenv`   | Load `.env` files (looks for `.env` in the current directory)                          |
| `glob`     | Glob matching: `glob [OPTIONS] pattern [string...]`                                    |
| `template` | Template engine (takes a template file or stdin)                                       |
| `semver`   | Semantic version operations: `compare, gt, lt, eq, gte, lte, parse, bump, valid, sort` |
| `verify`   | File hash verification: `verify [OPTIONS] file [expected-hash]`                        |
| `assert`   | Test assertions: `assert <test-expression> [message]`                                  |
| `retry`    | Retry commands: `retry [OPTIONS] -- command [args...]`                                 |
| `log`      | Structured logging: `log <level> <message> [key=value...]`                             |

## Files & directories

| Command    | Description                 |
| ---------- | --------------------------- |
| `ls`       | List directory              |
| `find`     | Search files                |
| `tree`     | Directory tree              |
| `stat`     | File metadata               |
| `file`     | Detect file type            |
| `du`       | Disk usage                  |
| `df`       | Filesystem usage            |
| `cp`       | Copy files                  |
| `mv`       | Move/rename files           |
| `rm`       | Remove files                |
| `rmdir`    | Remove empty directories    |
| `mkdir`    | Create directories          |
| `touch`    | Create/update files         |
| `chmod`    | Change permissions          |
| `chown`    | Change owner                |
| `ln`       | Create links                |
| `readlink` | Resolve symlinks            |
| `realpath` | Canonicalize paths          |
| `basename` | Strip directory from path   |
| `dirname`  | Strip filename from path    |
| `mktemp`   | Create temporary files/dirs |
| `mkfifo`   | Create FIFOs                |
| `split`    | Split files                 |
| `truncate` | Resize files                |
| `tee`      | Copy stdin to file + stdout |
| `patch`    | Apply diffs                 |

## Text processing

| Command    | Description                       |
| ---------- | --------------------------------- |
| `cat`      | Concatenate files                 |
| `head`     | First N lines                     |
| `tail`     | Last N lines                      |
| `less`     | Pager                             |
| `nl`       | Number lines                      |
| `grep`     | Pattern search                    |
| `rg`       | Fast search (ripgrep)             |
| `sed`      | Stream editor                     |
| `awk`      | Text processing language          |
| `cut`      | Extract fields                    |
| `tr`       | Translate characters              |
| `sort`     | Sort lines                        |
| `uniq`     | Filter duplicates                 |
| `wc`       | Count lines/words/bytes           |
| `diff`     | Compare files                     |
| `comm`     | Compare sorted lines              |
| `join`     | Join lines on a field             |
| `paste`    | Merge lines                       |
| `column`   | Columnate output                  |
| `fold`     | Wrap lines                        |
| `expand`   | Tabs → spaces                     |
| `unexpand` | Spaces → tabs                     |
| `tac`      | Reverse lines                     |
| `rev`      | Reverse characters                |
| `strings`  | Extract printable strings         |
| `xxd`      | Hex dump                          |
| `hexdump`  | Hex dump                          |
| `od`       | Octal dump                        |
| `numfmt`   | Number formatting                 |
| `iconv`    | Character encoding conversion     |
| `envsubst` | Environment variable substitution |
| `base64`   | Base64 encode/decode              |
| `yes`      | Repeat output                     |

## Archives, hashes, network

| Command     | Description             |
| ----------- | ----------------------- |
| `tar`       | Archive tool            |
| `zip`       | Create ZIP archives     |
| `unzip`     | Extract ZIP archives    |
| `gzip`      | gzip compression        |
| `gunzip`    | gzip decompression      |
| `md5sum`    | MD5 hash files          |
| `sha1sum`   | SHA-1 hash files        |
| `sha256sum` | SHA-256 hash files      |
| `curl`      | Transfer data over URLs |
| `wget`      | Download files          |

## Shell & scripting core

| Command                   | Description                     |
| ------------------------- | ------------------------------- |
| `bash` / `sh`             | Shells                          |
| `cd`                      | Change directory                |
| `pwd`                     | Print working directory         |
| `echo`                    | Display text                    |
| `printf`                  | Formatted output                |
| `read`                    | Read input                      |
| `set` / `unset`           | Set/unset variables and options |
| `export`                  | Export variables                |
| `local`                   | Local variables                 |
| `readonly`                | Mark variables read-only        |
| `declare` / `typeset`     | Declare variables/attributes    |
| `shift`                   | Shift positional parameters     |
| `eval`                    | Evaluate arguments as command   |
| `exec`                    | Replace shell / redirect        |
| `source`                  | Execute file in current shell   |
| `exit`                    | Exit shell                      |
| `return`                  | Return from function            |
| `break` / `continue`      | Loop control                    |
| `true` / `false`          | Exit with status 0 / 1          |
| `test` / `[`              | Evaluate expressions            |
| `trap`                    | Signal handling                 |
| `getopts`                 | Parse options                   |
| `let`                     | Arithmetic evaluation           |
| `caller`                  | Call stack info                 |
| `times`                   | Process times                   |
| `wait`                    | Wait for jobs                   |
| `kill`                    | Send signals                    |
| `mapfile` / `readarray`   | Read lines into array           |
| `fc`                      | History editing                 |
| `history`                 | Command history                 |
| `hash`                    | Command hash table              |
| `type`                    | Describe command type           |
| `command`                 | Invoke command directly         |
| `compgen`                 | Completion generation           |
| `alias` / `unalias`       | Define/remove aliases           |
| `dirs` / `popd` / `pushd` | Directory stack                 |
| `shopt`                   | Shell options                   |
| `help`                    | Builtin documentation           |

## System info & process control

| Command    | Description                 |
| ---------- | --------------------------- |
| `uname`    | OS info                     |
| `hostname` | Host name                   |
| `id`       | User/group info             |
| `whoami`   | Current user                |
| `date`     | Date/time                   |
| `env`      | Environment variables       |
| `printenv` | Print environment variables |
| `xargs`    | Build and run commands      |
| `parallel` | Run jobs in parallel        |
| `timeout`  | Run with a time limit       |
| `watch`    | Repeat a command            |
| `sleep`    | Pause execution             |
| `seq`      | Print number sequences      |
| `shuf`     | Shuffle lines               |
| `expr`     | Evaluate expressions        |
| `bc`       | Calculator                  |
| `clear`    | Clear screen                |

## Runtimes

| Command              | Description               |
| -------------------- | ------------------------- |
| `python` / `python3` | Python 3.12.0 interpreter |

---

*Generated from `compgen -b` / `compgen -c` plus `help <cmd>` and usage strings. Update by re-running the same discovery commands in the bash tool environment.*
