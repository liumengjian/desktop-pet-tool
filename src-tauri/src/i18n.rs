// 主进程侧的语言（菜单、托盘、系统对话框标题、文档任务提示）。
// 前端文案由 ui/i18n.js 负责；两边都以配置项 lang（"zh" | "en"）为准。
use std::sync::atomic::{AtomicU8, Ordering};

const ZH: u8 = 0;
const EN: u8 = 1;

static LANG: AtomicU8 = AtomicU8::new(ZH);

pub fn set_from_code(code: &str) {
    let v = if code.eq_ignore_ascii_case("en") { EN } else { ZH };
    LANG.store(v, Ordering::Relaxed);
}

pub fn is_en() -> bool {
    LANG.load(Ordering::Relaxed) == EN
}

/// 选中英两套文案里的一套
pub fn t(zh: &str, en: &str) -> String {
    if is_en() {
        en.to_string()
    } else {
        zh.to_string()
    }
}
