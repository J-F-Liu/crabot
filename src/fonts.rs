use fontdb::Family;
use iced::advanced::graphics::text;
use std::sync::OnceLock;

/// After loading system fonts, this searches fontdb for a known CJK family name
/// and sets it as the sans-serif generic family so all text benefits.
static CJK_FAMILY: OnceLock<&'static str> = OnceLock::new();

pub fn load_system_fonts() {
    let mut fs = text::font_system().write().expect("Write font system");

    fs.raw().db_mut().load_system_fonts();

    // Known CJK family names, ordered by likelihood across platforms.
    // After `load_system_fonts()` these should be discoverable via fontdb
    // on their respective platforms.
    const CJK_NAMES: &[&str] = &[
        "Microsoft YaHei UI",
        "Microsoft YaHei",
        "PingFang SC",
        "PingFang TC",
        "STHeiti",
        "STXihei",
        "Noto Sans CJK SC",
        "Noto Sans SC",
        "WenQuanYi Micro Hei",
        "WenQuanYi Zen Hei",
        "Droid Sans Fallback",
    ];

    let cjk_name: Option<&str> = {
        // `db_mut()` takes `&mut self` but `query` only needs `&Database`
        let db = fs.raw().db_mut();
        let mut found = None;
        for &name in CJK_NAMES {
            let q = fontdb::Query {
                families: &[Family::Name(name)],
                ..Default::default()
            };
            if db.query(&q).is_some() {
                found = Some(name);
                break;
            }
        }
        found
    };

    if let Some(name) = cjk_name {
        let _ = CJK_FAMILY.set(name);
        fs.raw().db_mut().set_sans_serif_family(name);
    }
}
