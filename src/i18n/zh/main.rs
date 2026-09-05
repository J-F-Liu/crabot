//! Translations for app-level chrome: modals, update banner, window chrome.
//! See `crate::i18n::zh` for conventions. Add new pairs here as you wrap
//! strings with `lang.tr("...")`.

pub static TABLE: &[(&str, &str)] = &[
    // ── modal.rs ──
    ("Empty Workspace", "空工作区"),
    (
        "Workspace path is empty.\n\nContinue with the default workspace?\n{}",
        "工作区路径为空。\n\n是否使用默认工作区继续？\n{}",
    ),
    ("Yes", "是"),
    ("No", "否"),
    ("Revert All Files", "还原所有文件"),
    (
        "Revert all files modified by this session?\n\n\
         Modified files are restored to their original content and files \
         created by the session are deleted. This cannot be undone.",
        "还原此会话修改的所有文件？\n\n\
         修改过的文件将恢复为原始内容，会话创建的文件将被删除。此操作无法撤销。",
    ),
    ("Revert All", "全部还原"),
    ("Cancel", "取消"),
    // ── update.rs ──
    ("Install New Version", "安装新版本"),
    ("Download Failed", "下载失败"),
    ("⏳ Downloading…", "⏳ 下载中…"),
    ("Restart to Update", "重启以更新"),
    (
        "🆕  Crabot v{latest} is available! (current: v{CURRENT_VERSION})",
        "🆕 Crabot v{latest} 新版本可用！（当前：v{CURRENT_VERSION}）",
    ),
    ("View Release Notes", "查看发布说明"),
    // ── app/conversation.rs export dialogs ──
    ("Export saved", "导出已保存"),
    ("Export session failed", "导出会话失败"),
    ("Export task failed", "导出任务失败"),
    ("Failed to write {}: {e}", "写入 {} 失败：{e}"),
    (
        "Wrote {} but could not open it in a browser: {e}",
        "已写入 {}，但无法在浏览器中打开：{e}",
    ),
];
