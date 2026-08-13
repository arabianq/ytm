use egui::{Context, FontData, FontDefinitions, FontFamily};
use std::{fs, path::PathBuf, sync::Arc};

struct SystemFontCandidate {
    name: &'static str,
    path: PathBuf,
    index: u32,
}

pub(super) fn configure_fonts(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    let loaded = load_cjk_fallbacks(&mut fonts);

    if loaded.is_empty() {
        log::warn!("No CJK fallback fonts found; non-Latin glyphs may render as tofu.");
    } else {
        log::info!("Loaded CJK fallback fonts: {}", loaded.join(", "));
    }

    ctx.set_fonts(fonts);
}

fn load_cjk_fallbacks(fonts: &mut FontDefinitions) -> Vec<String> {
    let mut loaded = Vec::new();

    for candidate in font_candidates() {
        let Ok(bytes) = fs::read(&candidate.path) else {
            continue;
        };

        let font_name = format!("system-cjk-{}", candidate.name);
        let mut font_data = FontData::from_owned(bytes);
        font_data.index = candidate.index;

        fonts
            .font_data
            .insert(font_name.clone(), Arc::new(font_data));

        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            if let Some(entries) = fonts.families.get_mut(&family)
                && !entries.contains(&font_name)
            {
                entries.push(font_name.clone());
            }
        }

        loaded.push(candidate.name.to_string());
    }

    loaded
}

fn font_candidates() -> Vec<SystemFontCandidate> {
    let mut candidates = Vec::new();

    if cfg!(target_os = "windows") {
        let fonts_dir = windows_fonts_dir();
        candidates.extend([
            SystemFontCandidate {
                name: "yu-gothic",
                path: fonts_dir.join("YuGothR.ttc"),
                index: 0,
            },
            SystemFontCandidate {
                name: "ms-gothic",
                path: fonts_dir.join("msgothic.ttc"),
                index: 0,
            },
            SystemFontCandidate {
                name: "microsoft-yahei",
                path: fonts_dir.join("msyh.ttc"),
                index: 0,
            },
            SystemFontCandidate {
                name: "simsun",
                path: fonts_dir.join("simsun.ttc"),
                index: 0,
            },
        ]);
    }

    if cfg!(target_os = "macos") {
        candidates.extend([
            SystemFontCandidate {
                name: "pingfang",
                path: PathBuf::from("/System/Library/Fonts/PingFang.ttc"),
                index: 0,
            },
            SystemFontCandidate {
                name: "hiragino-sans",
                path: PathBuf::from("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc"),
                index: 0,
            },
            SystemFontCandidate {
                name: "hiragino-mincho",
                path: PathBuf::from("/System/Library/Fonts/ヒラギノ明朝 ProN.ttc"),
                index: 0,
            },
        ]);
    }

    if cfg!(target_os = "linux") {
        candidates.extend([
            SystemFontCandidate {
                name: "noto-sans-cjk",
                path: PathBuf::from("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
                index: 0,
            },
            SystemFontCandidate {
                name: "noto-serif-cjk",
                path: PathBuf::from("/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc"),
                index: 0,
            },
            SystemFontCandidate {
                name: "source-han-sans",
                path: PathBuf::from(
                    "/usr/share/fonts/opentype/source-han-sans/SourceHanSans-Regular.otf",
                ),
                index: 0,
            },
        ]);
    }

    candidates
}

fn windows_fonts_dir() -> PathBuf {
    std::env::var_os("WINDIR")
        .or_else(|| std::env::var_os("SYSTEMROOT"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts")
}
