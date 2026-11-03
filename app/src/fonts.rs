use std::sync::Arc;

use eframe::egui;

/// Adobe Source Han Sans, language-specific Traditional Chinese — Taiwan.
/// Not the smaller region subset, and not Google's Noto Sans CJK build.
const SOURCE_HAN_SANS_TC: &[u8] = include_bytes!("../assets/SourceHanSansTC-Regular.otf");

const JHENGHEI_PATHS: &[&str] = &[
    r"C:\Windows\Fonts\msjh.ttc",
    r"C:\Windows\Fonts\msjh.ttf",
];

const PMINGLIU_PATHS: &[&str] = &[
    r"C:\Windows\Fonts\mingliu.ttc",
    r"C:\Windows\Fonts\pmingliu.ttf",
    r"C:\Windows\Fonts\mingliub.ttc",
];

#[inline(never)]
pub fn source_han() -> &'static [u8] {
    SOURCE_HAN_SANS_TC
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "Source Han Sans TC".to_owned(),
        Arc::new(egui::FontData::from_static(SOURCE_HAN_SANS_TC)),
    );
    let mut chain = vec!["Source Han Sans TC".to_owned()];
    crate::report::note("介面字形：思源黑體 TC（Traditional Chinese—Taiwan）");

    if let Some(data) = load_system_face(
        JHENGHEI_PATHS,
        &["Microsoft JhengHei", "微軟正黑體"],
        &["UI", "Bold", "Light", "ExtB"],
    ) {
        fonts
            .font_data
            .insert("Microsoft JhengHei".to_owned(), Arc::new(data));
        chain.push("Microsoft JhengHei".to_owned());
        crate::report::note("字形遞補：微軟正黑體");
    } else {
        crate::report::note("找不到微軟正黑體，略過這層遞補");
    }

    if let Some(data) = load_system_face(
        PMINGLIU_PATHS,
        &["PMingLiU", "新細明體"],
        &["ExtB", "Ext-B", "Bold"],
    ) {
        fonts
            .font_data
            .insert("PMingLiU".to_owned(), Arc::new(data));
        chain.push("PMingLiU".to_owned());
        crate::report::note("字形遞補：新細明體");
    } else {
        crate::report::note("找不到新細明體，略過這層遞補");
    }

    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        for (offset, name) in chain.iter().enumerate() {
            list.insert(offset, name.clone());
        }
    }
    ctx.set_fonts(fonts);
}

fn load_system_face(paths: &[&str], want: &[&str], reject: &[&str]) -> Option<egui::FontData> {
    for path in paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Some(index) = pick_face(&bytes, want, reject) else {
            continue;
        };
        if ab_glyph::FontRef::try_from_slice_and_index(&bytes, index).is_err() {
            crate::report::note(&format!("無法解析系統字型 {path}（face {index}）"));
            continue;
        }
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        return Some(data);
    }
    None
}

fn family_names(face: &ttf_parser::Face<'_>) -> Vec<String> {
    face.names()
        .into_iter()
        .filter(|name| name.name_id == 1 || name.name_id == 16)
        .filter_map(|name| name.to_string())
        .collect()
}

fn wanted(names: &[String], want: &[&str]) -> bool {
    names.iter().any(|name| want.iter().any(|token| name.contains(token)))
}

fn rejected(names: &[String], reject: &[&str]) -> bool {
    names.iter().any(|name| reject.iter().any(|token| name.contains(token)))
}

fn pick_face(bytes: &[u8], want: &[&str], reject: &[&str]) -> Option<u32> {
    let count = ttf_parser::fonts_in_collection(bytes).unwrap_or(1);
    let mut loose = None;
    for index in 0..count {
        let Ok(face) = ttf_parser::Face::parse(bytes, index) else {
            continue;
        };
        let names = family_names(&face);
        if !wanted(&names, want) || rejected(&names, reject) {
            continue;
        }
        if names.iter().any(|name| want.iter().any(|token| name == token)) {
            return Some(index);
        }
        loose.get_or_insert(index);
    }
    loose
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_glyph::Font as _;

    #[test]
    fn source_han_is_traditional_chinese_taiwan() {
        let index = pick_face(
            SOURCE_HAN_SANS_TC,
            &["Source Han Sans TC", "思源黑體"],
            &["HK", "HW", "Noto"],
        )
        .expect("face");
        assert_eq!(index, 0);
        let face = ttf_parser::Face::parse(SOURCE_HAN_SANS_TC, 0).unwrap();
        let names = family_names(&face);
        assert!(
            names.iter().any(|name| name == "Source Han Sans TC"),
            "{names:?}"
        );
        assert!(
            names.iter().any(|name| name.contains("思源黑體")),
            "{names:?}"
        );
    }

    #[test]
    fn source_han_draws_traditional_chinese() {
        let font = ab_glyph::FontRef::try_from_slice(SOURCE_HAN_SANS_TC).unwrap();
        let scale = font.pt_to_px_scale(32.0).unwrap();
        let ascent = font.ascent_unscaled();
        for ch in ['聲', '譜', '圖', '臺', '灣', '檔', '案', '體', '啟', '窗'] {
            let id = font.glyph_id(ch);
            assert_ne!(id.0, 0, "{ch} missing");
            let outlined = font
                .outline_glyph(ab_glyph::Glyph {
                    id,
                    scale,
                    position: ab_glyph::point(0.0, 0.0),
                })
                .unwrap_or_else(|| panic!("{ch} has no outline"));
            let bounds = outlined.px_bounds();
            let mut covered = 0u32;
            outlined.draw(|_, _, coverage| {
                if coverage > 0.2 {
                    covered += 1;
                }
            });
            assert!(covered > 30, "{ch} only covered {covered} px");
            let top = -bounds.min.y / 32.0 * 1000.0;
            assert!(
                top <= ascent + 1.0,
                "{ch} top {top} exceeds ascent {ascent}"
            );
        }
    }
}
