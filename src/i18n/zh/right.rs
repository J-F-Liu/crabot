//! Translations for the right pane: sections, processes, files, export.
//! See `crate::i18n::zh` for conventions. Add new pairs here as you wrap
//! strings with `lang.tr("...")`.

pub static TABLE: &[(&str, &str)] = &[
    // ── right_pane.rs: context window section ──
    ("Context window: {:.1}%", "上下文窗口：{:.1}%"),
    ("Context window ({cw})", "上下文窗口（{cw}）"),
    ("Context window", "上下文窗口"),
    ("Prompt tokens:", "填充词元数："),
    ("Cached tokens:", "缓存词元数："),
    ("Fill ratio:", "填充比例："),
    // ── token usage section ──
    ("Token Usage", "词元用量"),
    ("Token Usage: {}", "词元用量：{}"),
    ("Input tokens:", "输入词元数："),
    ("Output tokens:", "输出词元数："),
    ("Cache read:", "缓存读取："),
    ("Cache write:", "缓存写入："),
    ("Session cost:", "会话成本："),
    ("Num Requests:", "请求数："),
    ("Last Response:", "最后响应："),
    // ── todo / processes / files sections ──
    ("Todo List", "待办列表"),
    ("Running Processes", "运行中的进程"),
    ("Running Processes: {}", "运行中的进程：{}"),
    ("Accessed Files", "访问过的文件"),
    ("Modified Files", "修改过的文件"),
    ("Revert", "还原"),
    // ("Revert All", "全部还原") is already provided by the main table.
    ("Session {n} · ", "会话 {n} · "),
    // ── top toggles & ACP status line ──
    ("ACP Server", "ACP 服务器"),
    ("Dark theme", "深色主题"),
    ("stdio (host-spawned)", "stdio（宿主进程启动）"),
    ("Starting…", "启动中…"),
    ("Restart", "重启"),
    // ── expand/collapse tooltips ──
    ("Collapse", "折叠"),
    ("Expand", "展开"),
    // ── file-revert error messages (app/snapshot.rs) ──
    ("Failed to resolve path '{path}'", "无法解析路径 '{path}'"),
    (
        "Failed to read snapshot for '{path}': {e}",
        "读取 '{path}' 的快照失败：{e}",
    ),
    ("Failed to create parent dir: {e}", "创建父目录失败：{e}"),
    ("Failed to restore '{path}': {e}", "还原 '{path}' 失败：{e}"),
    ("Failed to delete '{path}': {e}", "删除 '{path}' 失败：{e}"),
    (
        "No snapshot for '{path}' — nothing to revert",
        "没有 '{path}' 的快照，无法还原",
    ),
    (
        "No workspace set — cannot revert '{raw}'",
        "未设置工作区，无法还原 '{raw}'",
    ),
    (
        "No workspace set — cannot revert files.",
        "未设置工作区，无法还原文件。",
    ),
    ("Revert task failed: {e}", "还原任务失败：{e}"),
];
