//! Loads every theme file shipped in `assets/themes` the way the application does, and checks
//! what the loader would otherwise swallow in silence: a file that does not parse, a key the
//! theme schema does not know (a typo), a colour string that is not a colour.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;
use theme::{Appearance, CornerStyle, ThemeColors, try_parse_color};

use crate::{ThemeFamilyContent, refine_theme};

/// Keys of a theme's `style` object that hold something other than a colour.
const NON_COLOR_STYLE_KEYS: &[&str] = &[
    "background.appearance",
    "corners",
    "accents",
    "players",
    "syntax",
];

fn themes_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/themes")
}

fn bundled_theme_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    let families = std::fs::read_dir(themes_dir()).expect("assets/themes must be readable");
    for family in families {
        let family = family.expect("directory entry").path();
        if !family.is_dir() {
            continue;
        }
        for file in std::fs::read_dir(&family).expect("theme directory must be readable") {
            let file = file.expect("directory entry").path();
            if file
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                files.push(file);
            }
        }
    }
    files.sort();
    files
}

/// Every key a theme may write in its `style` object, taken from the schema of
/// [`settings::ThemeStyleContent`].
fn known_style_keys() -> BTreeSet<String> {
    let schema =
        schemars::SchemaGenerator::default().into_root_schema_for::<settings::ThemeStyleContent>();
    let schema = serde_json::to_value(schema).expect("schema serializes");
    let keys: BTreeSet<String> = schema
        .get("properties")
        .and_then(Value::as_object)
        .expect("the theme style schema has properties")
        .keys()
        .cloned()
        .collect();
    for expected in [
        "border",
        "corners",
        "players",
        "syntax",
        "error",
        "accent.background",
        "accent.hover",
        "accent.foreground",
        "border.strong",
        "text.faint",
    ] {
        assert!(
            keys.contains(expected),
            "the theme style schema lost the key {expected:?}: the unknown-key check would be wrong"
        );
    }
    keys
}

fn check_color(problems: &mut Vec<String>, location: &str, value: &Value) {
    match value {
        // `null` means "not set" in a theme file.
        Value::Null => {}
        Value::String(color) => {
            if let Err(error) = try_parse_color(color) {
                problems.push(format!("{location}: {color:?} is not a colour ({error})"));
            }
        }
        other => problems.push(format!(
            "{location}: expected a colour string, found {other}"
        )),
    }
}

fn check_style(
    problems: &mut Vec<String>,
    location: &str,
    style: &Value,
    known: &BTreeSet<String>,
) {
    let Some(style) = style.as_object() else {
        problems.push(format!("{location}: `style` is not an object"));
        return;
    };

    for (key, value) in style {
        let location = format!("{location}: {key:?}");
        if !known.contains(key) {
            problems.push(format!("{location} is not a key of the theme schema"));
            continue;
        }
        if !NON_COLOR_STYLE_KEYS.contains(&key.as_str()) {
            check_color(problems, &location, value);
        }
    }

    for (index, accent) in style
        .get("accents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        check_color(problems, &format!("{location}: accents[{index}]"), accent);
    }

    for (index, player) in style
        .get("players")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        for (key, value) in player.as_object().into_iter().flatten() {
            let location = format!("{location}: players[{index}].{key}");
            if matches!(key.as_str(), "cursor" | "background" | "selection") {
                check_color(problems, &location, value);
            } else {
                problems.push(format!("{location} is not a key of a player colour"));
            }
        }
    }

    for (name, highlight) in style
        .get("syntax")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        for key in ["color", "background_color"] {
            if let Some(value) = highlight.get(key) {
                check_color(problems, &format!("{location}: syntax.{name}.{key}"), value);
            }
        }
    }
}

#[test]
fn every_bundled_theme_file_loads() {
    let files = bundled_theme_files();
    assert!(
        files
            .iter()
            .any(|file| file.ends_with("unaligned/unaligned.json")),
        "the default theme file was not found under {:?}",
        themes_dir()
    );

    let known_style_keys = known_style_keys();
    let mut problems = Vec::new();
    let mut theme_names = BTreeSet::new();

    for file in &files {
        let file_name = file
            .strip_prefix(themes_dir())
            .unwrap_or(file)
            .display()
            .to_string();
        let bytes = std::fs::read(file).expect("theme file must be readable");

        // Strict JSON, exactly like `load_bundled_themes`: no comments, no trailing commas, and
        // a value of the wrong type for a key is an error.
        let family: ThemeFamilyContent = match serde_json::from_slice(&bytes) {
            Ok(family) => family,
            Err(error) => {
                problems.push(format!("{file_name}: does not parse: {error}"));
                continue;
            }
        };
        let raw: Value = serde_json::from_slice(&bytes).expect("parsed once already");

        if family.themes.is_empty() {
            problems.push(format!("{file_name}: the family has no theme"));
        }

        for (index, theme_content) in family.themes.iter().enumerate() {
            let location = format!("{file_name}: {:?}", theme_content.name);
            if !theme_names.insert(theme_content.name.clone()) {
                problems.push(format!("{location}: this theme name is used twice"));
            }

            check_style(
                &mut problems,
                &location,
                &raw["themes"][index]["style"],
                &known_style_keys,
            );

            let theme = refine_theme(theme_content);
            assert_eq!(theme.name.as_ref(), theme_content.name);

            // The derived tokens are plain values after refinement, whatever the file states.
            let colors = theme.colors();
            let style = &theme_content.style.colors;
            if style.accent_background.is_none() {
                assert_eq!(
                    colors.accent_background,
                    theme.players().local().cursor,
                    "{location}: accent fill falls back to the local player's cursor"
                );
            }
            if style.accent_hover.is_none() {
                assert_eq!(
                    colors.accent_hover,
                    ThemeColors::accent_hover_for(colors.accent_background, theme.appearance),
                    "{location}"
                );
            }
            if style.accent_foreground.is_none() {
                assert_eq!(
                    colors.accent_foreground,
                    ThemeColors::accent_foreground_for(colors.accent_background),
                    "{location}"
                );
            }
            if style.border_strong.is_none() {
                assert_eq!(colors.border_strong, colors.border, "{location}");
            }
            if style.text_faint.is_none() {
                assert_eq!(colors.text_faint, colors.text_muted, "{location}");
            }
            if theme_content.style.corners.is_none() {
                assert_eq!(theme.corners(), CornerStyle::Round, "{location}");
            }
        }
    }

    assert!(
        problems.is_empty(),
        "bundled theme files have problems:\n{}",
        problems.join("\n")
    );
}

#[test]
fn third_party_themes_get_a_readable_accent_foreground() {
    // One, Ayu and Gruvbox do not state the text colour of their primary button: it is computed,
    // and must be at least as readable as the white that used to be hard-coded.
    for file in bundled_theme_files() {
        let bytes = std::fs::read(&file).expect("theme file must be readable");
        let Ok(family) = serde_json::from_slice::<ThemeFamilyContent>(&bytes) else {
            // Reported by `every_bundled_theme_file_loads`.
            continue;
        };
        for theme_content in &family.themes {
            if theme_content.style.colors.accent_foreground.is_some() {
                continue;
            }
            let theme = refine_theme(theme_content);
            let colors = theme.colors();
            let computed =
                theme::contrast_ratio(colors.accent_foreground, colors.accent_background);
            let white = theme::contrast_ratio(gpui::white(), colors.accent_background);
            assert!(
                computed >= white,
                "{}: computed accent text {computed:.2}:1 is worse than white {white:.2}:1",
                theme_content.name
            );
            assert!(
                computed >= 4.5,
                "{}: accent text contrast is {computed:.2}:1",
                theme_content.name
            );
        }
    }
}

#[test]
fn appearance_decides_the_direction_of_the_accent_hover() {
    let accent = try_parse_color("#e5322d").expect("valid colour");
    let on_dark = ThemeColors::accent_hover_for(accent, Appearance::Dark);
    let on_light = ThemeColors::accent_hover_for(accent, Appearance::Light);
    assert!(on_dark.l > accent.l);
    assert!(on_light.l < accent.l);
    assert!((on_dark.l - accent.l - 0.08).abs() < 1e-4);
    assert!((accent.l - on_light.l - 0.08).abs() < 1e-4);
}

#[test]
fn the_style_check_reports_typos_and_bad_colors() {
    let known = known_style_keys();
    let style = serde_json::json!({
        "border": "#112233",
        "border.strong": null,
        "corners": "square",
        "accent.backgroud": "#e5322d",
        "text.faint": "827d75",
        "players": [{ "cursor": "#e5322d", "selection": "red" }],
        "accents": ["#e5322d", "nope"],
        "syntax": { "comment": { "color": "#12345" } }
    });

    let mut problems = Vec::new();
    check_style(&mut problems, "test", &style, &known);

    let expected = [
        "\"accent.backgroud\" is not a key",
        "\"text.faint\": \"827d75\" is not a colour",
        "players[0].selection: \"red\" is not a colour",
        "accents[1]: \"nope\" is not a colour",
        "syntax.comment.color: \"#12345\" is not a colour",
    ];
    for expected in expected {
        assert!(
            problems.iter().any(|problem| problem.contains(expected)),
            "missing {expected:?} in {problems:#?}"
        );
    }
    assert_eq!(problems.len(), expected.len(), "{problems:#?}");

    // A value of the wrong type for a known key stops the whole file from loading.
    assert!(
        serde_json::from_str::<settings::ThemeStyleContent>(r#"{ "corners": "squre" }"#).is_err()
    );
}
