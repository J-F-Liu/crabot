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
    ("Attachment ▾", "附件 ▾"),
    ("Attach text file", "附加文本文件"),
    ("Attach image file", "附加图片文件"),
    ("Attachment not added", "附件未添加"),
    ("Attachment not sent", "附件未发送"),
    ("Some files were not attached:", "以下文件未能附加："),
    (
        "Attachment limit reached ({} per prompt).",
        "已达到每条提示 {} 个附件的上限。",
    ),
    ("Images not supported", "模型不支持图片"),
    (
        "The selected model does not accept image input. Remove the image attachments or choose a vision-capable model.",
        "当前模型不支持图片输入，请移除图片附件或选择支持视觉的模型。",
    ),
    (
        "{} is no longer available. Remove it and try again.",
        "{} 已不存在，请移除后重试。",
    ),
    ("Work mode", "工作模式"),
    // Work-mode names, shared by user_prompt / center_pane / prompt_recipes / export.
    ("Plan", "计划"),
    ("Code", "代码"),
    ("Review", "审查"),
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
    ("Connecting...", "连接中…"),
    ("command not found", "找不到命令"),
    ("failed to launch", "启动失败"),
    ("connection failed", "连接失败"),
    ("tool listing failed", "工具列表失败"),
    ("no tools", "无工具"),
    ("timed out", "超时"),
    // ── workspace picker pseudo-entry (system_prompt.rs) ──
    ("📁 Select new...", "📁 选择新文件…"),
];
