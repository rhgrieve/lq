//! `schema-settings.ts`.

/// The LiquidDoc-syntax type a schema setting produces, or `None` when unmapped.
pub fn schema_setting_liquid_type(setting_type: &str) -> Option<&'static str> {
    Some(match setting_type {
        "checkbox" => "boolean",
        "number" | "range" => "number",
        "color_background" | "color_palette" | "html" | "inline_richtext" | "liquid" | "radio"
        | "richtext" | "select" | "text" | "text_alignment" | "textarea" | "url" | "video_url" => "string",
        "article" => "article",
        "blog" => "blog",
        "collection" => "collection",
        "color" => "color",
        "color_scheme" => "color_scheme",
        "page" => "page",
        "product" => "product",
        "video" => "video",
        "font_picker" => "font",
        "image_picker" => "image",
        "link_list" => "linklist",
        "collection_list" => "collection[]",
        "product_list" => "product[]",
        _ => return None,
    })
}
