//! JSON payloads of the data layers. Field names must match the ManiaScript `#Struct`s.

use serde::Serialize;

/// One line of the live ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RankingRow {
    pub login: String,
    pub name: String,
    pub rank: i32,
    /// Best time in ms (time modes), `0` if none.
    pub time: i32,
    pub points: i32,
    /// Difference to the first, in ms (time modes).
    pub gap: i32,
    /// Finished the current round (points modes).
    pub finished: bool,
}

/// Live ranking (localized header).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RankingData {
    /// `time` or `points`.
    pub mode: String,
    /// Header: cup phase or mode name; empty keeps the view's default title.
    pub title: String,
    /// Right of the header, e.g. "MAP 3/20".
    pub subtitle: String,
    /// Number of qualified players (a line is drawn below), 0 = none.
    pub cutline: i32,
    pub rows: Vec<RankingRow>,
}

/// Cup banner (texts are already localized).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CupInfoData {
    /// Empty when no cup is loaded (the banner hides).
    pub cup: String,
    pub phase: String,
    pub status: String,
    pub ready_check: bool,
    pub ready_logins: Vec<String>,
    pub ready_done: Vec<String>,
    /// Remaining ms of the ready check when sent.
    pub remaining_ms: i32,
}

/// One best time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordRow {
    pub login: String,
    pub name: String,
    pub time: i32,
}

/// Best times on the map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordsData {
    pub rows: Vec<RecordRow>,
}

/// One line of the scores table. Values are preformatted (times, "+12").
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScoreboardRow {
    pub login: String,
    pub name: String,
    /// 0 = not ranked yet.
    pub rank: i32,
    pub info: String,
    pub delta: String,
    pub total: String,
}

/// Scores table (Tab): localized header and columns, every player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScoreboardData {
    pub title: String,
    pub subtitle: String,
    pub col_info: String,
    pub col_delta: String,
    pub col_total: String,
    pub rows: Vec<ScoreboardRow>,
}
