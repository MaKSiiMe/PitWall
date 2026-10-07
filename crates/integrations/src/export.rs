//! CSV / JSON exports of results.

use serde::Serialize;

/// One line of an exported ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportRow {
    pub phase: String,
    pub rank: u32,
    pub id: String,
    pub name: String,
    pub points: i32,
    pub best_time: Option<i32>,
    pub qualified: bool,
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

/// CSV with a header line (RFC 4180 quoting).
pub fn to_csv(rows: &[ExportRow]) -> String {
    let mut out = String::from("phase,rank,id,name,points,best_time_ms,qualified\n");
    for r in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{},{}\n",
            csv_field(&r.phase),
            r.rank,
            csv_field(&r.id),
            csv_field(&r.name),
            r.points,
            r.best_time.map(|t| t.to_string()).unwrap_or_default(),
            r.qualified
        ));
    }
    out
}

/// Formats milliseconds as `m:ss.mmm`.
pub fn format_time(ms: i32) -> String {
    let ms = ms.max(0);
    format!("{}:{:02}.{:03}", ms / 60_000, (ms / 1000) % 60, ms % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quoting() {
        let rows = [ExportRow {
            phase: "quali".into(),
            rank: 1,
            id: "a".into(),
            name: "Max, \"the\" best".into(),
            points: 0,
            best_time: Some(51_234),
            qualified: true,
        }];
        assert_eq!(
            to_csv(&rows),
            "phase,rank,id,name,points,best_time_ms,qualified\nquali,1,a,\"Max, \"\"the\"\" best\",0,51234,true\n"
        );
        assert_eq!(format_time(61_234), "1:01.234");
    }
}
