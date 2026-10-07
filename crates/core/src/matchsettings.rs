//! MatchSettings files (playlist + mode script + settings), as documented on the Trackmania wiki.
//!
//! A hot mode switch writes such a file with `WriteFile`, loads it with `LoadMatchSettings` and
//! restarts the map: mode, settings and playlist change in a single map load.

use pitwall_gbx::types::{ScriptSetting, ScriptSettings};
use quick_escape::escape;

/// Normalizes a mode script name for comparisons:
/// `Modes/TrackMania/TM_Cup_Online.Script.txt` == `trackmania/tm_cup_online`.
pub fn normalize_script_name(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase().replace('\\', "/");
    let without_ext = lower.strip_suffix(".script.txt").unwrap_or(&lower);
    without_ext
        .strip_prefix("modes/")
        .unwrap_or(without_ext)
        .to_owned()
}

/// Name to write in `<script_name>`: the wiki documents official modes without `.Script.txt`.
fn script_name_for_file(name: &str) -> &str {
    name.strip_suffix(".Script.txt")
        .or_else(|| name.strip_suffix(".script.txt"))
        .unwrap_or(name)
}

/// Renders a MatchSettings document.
pub fn render(
    script: &str,
    settings: &ScriptSettings,
    maps: &[String],
    start_index: usize,
) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\" ?>\n<playlist>\n");
    out.push_str("\t<gameinfos>\n\t\t<game_mode>0</game_mode>\n");
    out.push_str(&format!(
        "\t\t<script_name>{}</script_name>\n",
        escape(script_name_for_file(script))
    ));
    out.push_str("\t</gameinfos>\n");
    out.push_str("\t<filter>\n\t\t<random_map_order>0</random_map_order>\n\t</filter>\n");
    out.push_str("\t<mode_script_settings>\n");
    for (name, value) in settings {
        let (kind, value) = match value {
            ScriptSetting::Bool(b) => ("boolean", if *b { "1".to_owned() } else { "0".to_owned() }),
            ScriptSetting::Int(i) => ("integer", i.to_string()),
            ScriptSetting::Real(r) => ("real", r.to_string()),
            ScriptSetting::Text(t) => ("text", t.clone()),
        };
        out.push_str(&format!(
            "\t\t<setting name=\"{}\" type=\"{kind}\" value=\"{}\"/>\n",
            escape(name),
            escape(&value)
        ));
    }
    out.push_str("\t</mode_script_settings>\n");
    out.push_str(&format!("\t<startindex>{start_index}</startindex>\n"));
    for map in maps {
        out.push_str(&format!("\t<map><file>{}</file></map>\n", escape(map)));
    }
    out.push_str("</playlist>\n");
    out
}

/// Makes a label safe to use as a file name.
pub fn file_safe(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "format".to_owned()
    } else {
        cleaned
    }
}

/// Minimal XML escaping (no external dependency needed for this file).
mod quick_escape {
    pub fn escape(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&apos;"),
                c => out.push(c),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_playlist() {
        let mut settings = ScriptSettings::new();
        settings.insert("S_PointsLimit".into(), ScriptSetting::Int(100));
        settings.insert("S_WarmUpNb".into(), ScriptSetting::Int(1));
        settings.insert("S_UseTieBreak".into(), ScriptSetting::Bool(false));
        settings.insert(
            "S_PointsRepartition".into(),
            ScriptSetting::Text("10,6,4,3,2,1".into()),
        );
        let xml = render(
            "Trackmania/TM_Cup_Online.Script.txt",
            &settings,
            &["Campaigns/A & B.Map.Gbx".to_owned()],
            0,
        );
        assert!(xml.contains("<script_name>Trackmania/TM_Cup_Online</script_name>"));
        assert!(xml.contains(r#"<setting name="S_PointsLimit" type="integer" value="100"/>"#));
        assert!(xml.contains(r#"<setting name="S_UseTieBreak" type="boolean" value="0"/>"#));
        assert!(
            xml.contains(
                r#"<setting name="S_PointsRepartition" type="text" value="10,6,4,3,2,1"/>"#
            )
        );
        assert!(xml.contains("<map><file>Campaigns/A &amp; B.Map.Gbx</file></map>"));
        assert!(xml.contains("<startindex>0</startindex>"));
    }

    #[test]
    fn script_names_compare_loosely() {
        assert_eq!(
            normalize_script_name("Modes/TrackMania/TM_Cup_Online.Script.txt"),
            normalize_script_name("Trackmania/TM_Cup_Online")
        );
        assert_ne!(
            normalize_script_name("Trackmania/TM_Cup_Online"),
            normalize_script_name("Trackmania/TM_Rounds_Online")
        );
    }

    #[test]
    fn file_names_are_sanitized() {
        assert_eq!(file_safe("Mini Cup #1/final"), "Mini_Cup__1_final");
        assert_eq!(file_safe(""), "format");
    }
}
