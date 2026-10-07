//! Colors and positions, configurable in `[plugins.ui]`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::widget::WidgetId;

/// Dark, sober esport theme.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    /// Accent color, hex without `#` (3 or 6 digits).
    pub accent: String,
    /// Background color of panels.
    pub background: String,
    pub background_opacity: f32,
    pub text: String,
    pub muted: String,
    /// Color of time gains / losses in the checkpoint widget.
    pub good: String,
    pub bad: String,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            accent: "0af".to_owned(),
            background: "111".to_owned(),
            background_opacity: 0.75,
            text: "fff".to_owned(),
            muted: "999".to_owned(),
            good: "39f".to_owned(),
            bad: "f44".to_owned(),
        }
    }
}

impl Theme {
    /// Keeps only valid hex colors, falling back to the defaults.
    pub fn sanitized(mut self) -> Theme {
        let defaults = Theme::default();
        let fix = |value: &mut String, default: &str| {
            let ok = matches!(value.len(), 3 | 6) && value.chars().all(|c| c.is_ascii_hexdigit());
            if !ok {
                *value = default.to_owned();
            }
        };
        fix(&mut self.accent, &defaults.accent);
        fix(&mut self.background, &defaults.background);
        fix(&mut self.text, &defaults.text);
        fix(&mut self.muted, &defaults.muted);
        fix(&mut self.good, &defaults.good);
        fix(&mut self.bad, &defaults.bad);
        self.background_opacity = self.background_opacity.clamp(0.0, 1.0);
        self
    }

    /// ManiaScript `Vec3` literal of a hex color.
    pub fn vec3(hex: &str) -> String {
        let digits: Vec<u32> = hex.chars().filter_map(|c| c.to_digit(16)).collect();
        let (r, g, b) = match digits.as_slice() {
            [r, g, b] => (r * 17, g * 17, b * 17),
            [r1, r2, g1, g2, b1, b2] => (r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2),
            _ => (255, 255, 255),
        };
        let f = |v: u32| format!("{:.3}", f64::from(v) / 255.0);
        format!("<{}, {}, {}>", f(r), f(g), f(b))
    }
}

/// Top-left corner of a widget, in Manialink units (x ∈ [-160, 160], y ∈ [-90, 90]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub x: f32,
    pub y: f32,
}

/// Widget positions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub positions: BTreeMap<WidgetId, Position>,
    /// Rows of the ranking and records widgets.
    pub max_rows: usize,
}

impl Default for Layout {
    fn default() -> Self {
        let positions = [
            (WidgetId::Ranking, Position { x: 116.0, y: 62.0 }),
            (WidgetId::CpDiff, Position { x: 0.0, y: 52.0 }),
            (WidgetId::CupInfo, Position { x: 0.0, y: 88.0 }),
            (WidgetId::Records, Position { x: -160.0, y: 62.0 }),
            (
                WidgetId::AdminPanel,
                Position {
                    x: -160.0,
                    y: -30.0,
                },
            ),
            (
                WidgetId::Menu,
                Position {
                    x: -160.0,
                    y: -82.0,
                },
            ),
        ]
        .into_iter()
        .collect();
        Self {
            positions,
            max_rows: 12,
        }
    }
}

impl Layout {
    pub fn position(&self, widget: WidgetId) -> Position {
        self.positions
            .get(&widget)
            .copied()
            .or_else(|| Layout::default().positions.get(&widget).copied())
            .unwrap_or(Position { x: 0.0, y: 0.0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(Theme::vec3("0af"), "<0.000, 0.667, 1.000>");
        assert_eq!(Theme::vec3("ff0000"), "<1.000, 0.000, 0.000>");
        let theme = Theme {
            accent: "zzz".into(),
            background_opacity: 3.0,
            ..Theme::default()
        }
        .sanitized();
        assert_eq!(theme.accent, "0af");
        assert!((theme.background_opacity - 1.0).abs() < f32::EPSILON);
    }
}
