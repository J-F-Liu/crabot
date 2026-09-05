//! Translations for the center pane: conversation, tool messages, tabs.
//! See `crate::i18n::zh` for conventions. Add new pairs here as you wrap
//! strings with `lang.tr("...")`.

pub static TABLE: &[(&str, &str)] = &[
    // ── dialog headers & turn badges (center_pane.rs) ──
    ("{} turn", "{} 轮"),
    ("{} turns", "{} 轮"),
    ("Dialog {}", "对话 {}"),
    ("New session", "新会话"),
    ("User", "用户"),
    ("Assistant", "助手"),
    ("System", "系统"),
    // ── session header menu & info (center_pane.rs) ──
    ("Copy title", "复制标题"),
    ("Resend session", "重新发送"),
    ("Fork session", "分身会话"),
    ("Compact session", "压缩会话"),
    ("Export as HTML", "导出为 HTML"),
    ("Model: {model_id}", "模型：{model_id}"),
    ("Spawned from {}", "由 {} 派生"),
    ("session {}", "会话 {}"),
    ("Created: {}", "创建时间：{}"),
    // ── status line (center_pane.rs) ──
    ("⏹ Stop", "⏹ 停止"),
    ("Session {} running…", "会话 {} 运行中…"),
    ("Sessions {} running…", "会话 {} 运行中…"),
    ("⏳ Loading LLM…", "⏳ 正在加载 LLM…"),
    ("💭 LLM thinking…", "💭 LLM 思考中…"),
    ("🛠️ Tool executing…", "🛠️ 工具执行中…"),
    ("✅ Ready", "✅ 就绪"),
    (
        "Send user prompt to start dialog with LLM",
        "发送提示词即可开始与 LLM 对话",
    ),
    // ── ask tool view (tool_message.rs) ──
    ("⏳ {seconds_left}s left", "⏳ 剩余 {seconds_left} 秒"),
    ("Extend +{} min", "延长 +{} 分钟"),
    ("🤖 LLM asks:", "🤖 LLM 提问："),
    ("Enter my answer", "输入我的回答"),
    ("You decide", "你来决定"),
    ("Ok", "确定"),
    ("Type your answer…", "输入你的回答…"),
    ("Answer", "回答"),
    ("Error", "错误"),
    ("Options:", "选项："),
    ("Result", "结果"),
    // ── tool result & argument tables (tool_message.rs) ──
    ("{} edit(s)", "{} 处修改"),
    ("Edit #{}", "编辑 #{}"),
    ("Text", "内容"),
    ("Status", "状态"),
    ("pending", "待处理"),
    ("in progress", "进行中"),
    ("completed", "已完成"),
    ("⚠ invalid", "⚠ 无效"),
    ("Running…", "运行中…"),
    (
        "… {skipped} bytes of earlier output hidden …",
        "… 省略了此前 {skipped} 字节的输出 …",
    ),
    // ── session tabs (session_tabs.rs) ──
    ("Session {}", "会话 {}"),
    // ── search bar (search_bar.rs) ──
    ("Search…", "搜索…"),
    // ── status line retry countdown (app/session_state.rs) ──
    (
        "Retry in {seconds} second{s} ({attempt}/{max})",
        "将在 {seconds} 秒后重试（{attempt}/{max}）",
    ),
    // ── static HTML export chrome (views/export.rs) ──
    ("Tool - {name}", "工具 - {name}"),
];
