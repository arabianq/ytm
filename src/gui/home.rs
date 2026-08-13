use super::{HomeBanner, HomeChip, HomeItem, HomeSection, HomeSnapshot};
use serde_json::Value;
use ytmapi_rs::common::Thumbnail;

pub(super) fn parse_home_snapshot(root: &Value) -> HomeSnapshot {
    let single_column = root
        .pointer("/contents/singleColumnBrowseResultsRenderer")
        .unwrap_or(&Value::Null);
    let tab = root
        .pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer")
        .unwrap_or(&Value::Null);
    let contents = tab
        .pointer("/content/sectionListRenderer/contents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let sections = contents.iter().filter_map(parse_home_section).collect();
    let banner = contents.iter().find_map(parse_home_banner);
    let chips = single_column
        .pointer("/header/chipCloudRenderer/chips")
        .and_then(Value::as_array)
        .map(|chips| chips.iter().filter_map(parse_home_chip).collect())
        .unwrap_or_default();
    HomeSnapshot {
        chips,
        sections,
        banner,
    }
}

fn parse_home_section(section: &Value) -> Option<HomeSection> {
    let carousel = section.get("musicCarouselShelfRenderer")?;
    let title = runs_text_at(
        carousel,
        "/header/musicCarouselShelfBasicHeaderRenderer/title/runs",
    )
    .or_else(|| {
        string_at(
            carousel,
            "/header/musicCarouselShelfBasicHeaderRenderer/title/text",
        )
    })
    .unwrap_or_else(|| "Recommended".to_string());

    let items = carousel
        .get("contents")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(parse_home_item).collect::<Vec<_>>())
        .unwrap_or_default();

    if items.is_empty() {
        None
    } else {
        Some(HomeSection { title, items })
    }
}

fn parse_home_banner(section: &Value) -> Option<HomeBanner> {
    let banner = section.get("musicTastebuilderShelfRenderer")?;
    let title = runs_text_at(banner, "/primaryText/runs")?;
    let subtitle = runs_text_at(banner, "/secondaryText/runs").unwrap_or_default();
    let thumbnails = thumbnails_at(
        banner,
        "/thumbnail/musicTastebuilderShelfThumbnailRenderer/thumbnail/thumbnails",
    );

    Some(HomeBanner {
        title,
        subtitle,
        thumbnails,
    })
}

fn parse_home_chip(chip: &Value) -> Option<HomeChip> {
    let chip = chip.get("chipCloudChipRenderer")?;
    let title = runs_text_at(chip, "/text/runs")?;
    let params = string_at(chip, "/navigationEndpoint/browseEndpoint/params");
    let selected = chip
        .get("isSelected")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    Some(HomeChip {
        title,
        params,
        selected,
    })
}

fn parse_home_item(item: &Value) -> Option<HomeItem> {
    let item = item.get("musicTwoRowItemRenderer")?;
    let title = runs_text_at(item, "/title/runs")?;
    let subtitle = runs_text_at(item, "/subtitle/runs").unwrap_or_default();
    let browse_id = string_at(item, "/navigationEndpoint/browseEndpoint/browseId");
    let page_type = string_at(
        item,
        "/navigationEndpoint/browseEndpoint/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType",
    );
    let thumbnails = thumbnails_at(
        item,
        "/thumbnailRenderer/musicThumbnailRenderer/thumbnail/thumbnails",
    );

    Some(HomeItem {
        title,
        subtitle,
        browse_id,
        page_type,
        thumbnails,
    })
}

fn runs_text_at(value: &Value, pointer: &str) -> Option<String> {
    let runs = value.pointer(pointer)?.as_array()?;
    let text = runs
        .iter()
        .filter_map(|run| run.get("text").and_then(Value::as_str))
        .collect::<String>();
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn string_at(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

fn thumbnails_at(value: &Value, pointer: &str) -> Vec<Thumbnail> {
    value
        .pointer(pointer)
        .cloned()
        .and_then(|thumbs| serde_json::from_value::<Vec<Thumbnail>>(thumbs).ok())
        .unwrap_or_default()
}
