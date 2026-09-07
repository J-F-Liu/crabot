//! Translations for the left pane: system prompt, user prompt, sessions.
//! See `crate::i18n::zh` for conventions. Add new pairs here as you wrap
//! strings with `lang.tr("...")`.

pub static TABLE: &[(&str, &str)] = &[
    // ── pane labels (left_pane.rs) ──
    ("System Prompt", "系统提示词"),
    ("User Prompt", "用户提示词"),
    // ── system prompt fields (system_prompt.rs) ──
    ("Preamble", "前言"),
    ("Skills", "技能"),
    ("Tools", "工具"),
    ("Workspace", "工作区"),
    ("Date", "日期"),
    ("None selected", "未选择"),
    // ── user prompt area (user_prompt.rs) ──
    ("Workspace tree", "工作区文件树"),
    ("Recipes ▾", "配方 ▾"),
    ("Work mode", "工作模式"),
    ("Send", "发送"),
    ("Loading…", "加载中…"),
    ("No workspace selected.", "未选择工作区。"),
    // ── session picker (session_list.rs) ──
    ("Session", "会话"),
    // ── model config (model_config.rs) ──
    ("Open Settings", "打开设置"),
    ("Level", "级别"),
    // ── tool sections (tool_list.rs) ──
    ("MCP Tools", "MCP 工具"),
    ("Reconnect...", "重连中…"),
    ("command not found", "找不到命令"),
    ("failed to launch", "启动失败"),
    ("connection failed", "连接失败"),
    ("tool listing failed", "工具列表失败"),
    ("no tools", "无工具"),
    ("timed out", "超时"),
    // ── workspace picker pseudo-entry (system_prompt.rs) ──
    ("📁 Select new...", "📁 选择新文件…"),
];
