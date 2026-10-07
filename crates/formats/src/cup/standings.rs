//! Phase rankings computed from `Trackmania.Scores`.

use pitwall_core::gbx::callbacks::Scores;
use serde::{Deserialize, Serialize};

/// One line of a ranking. `id` is a login (players cups) or a team id (teams cups).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    pub id: String,
    pub name: String,
    /// 1-based; equal ranks are allowed for brackets.
    pub rank: u32,
    pub points: i32,
    /// Best race time in ms, if any.
    pub best_time: Option<i32>,
}

/// Ranks `participants` (logins) from the mode's scores.
///
/// The mode's own `rank` is authoritative (time for TimeAttack, points for Rounds/Cup, ...).
/// Participants absent from the scores are ranked last, in their given order.
pub fn rank_players(
    scores: &Scores,
    participants: &[String],
    name_of: impl Fn(&str) -> String,
) -> Vec<Standing> {
    let mut ranked: Vec<_> = scores
        .players
        .iter()
        .filter(|p| participants.contains(&p.login))
        .collect();
    // rank 0 means "not ranked" for the game: put them after ranked ones.
    ranked.sort_by_key(|p| (p.rank <= 0, p.rank));
    let mut out: Vec<Standing> = ranked
        .iter()
        .map(|p| Standing {
            id: p.login.clone(),
            name: if p.name.is_empty() {
                name_of(&p.login)
            } else {
                p.name.clone()
            },
            rank: 0,
            points: if p.matchpoints != 0 {
                p.matchpoints
            } else {
                p.mappoints
            },
            best_time: (p.bestracetime > 0).then_some(p.bestracetime),
        })
        .collect();
    for login in participants {
        if !out.iter().any(|s| &s.id == login) {
            out.push(Standing {
                id: login.clone(),
                name: name_of(login),
                rank: 0,
                points: 0,
                best_time: None,
            });
        }
    }
    for (i, standing) in out.iter_mut().enumerate() {
        standing.rank = u32::try_from(i + 1).unwrap_or(u32::MAX);
    }
    out
}

/// Ranks teams from the team scores of a blue/red match. `blue`/`red` are cup team ids.
pub fn rank_teams(
    scores: &Scores,
    blue: &str,
    red: &str,
    name_of: impl Fn(&str) -> String,
) -> Vec<Standing> {
    let points = |id: i32| {
        scores.teams.iter().find(|t| t.id == id).map_or(0, |t| {
            if t.matchpoints != 0 {
                t.matchpoints
            } else {
                t.mappoints
            }
        })
    };
    let (blue_points, red_points) = (points(0), points(1));
    let blue_wins = match scores.winnerteam {
        0 => true,
        1 => false,
        _ => blue_points >= red_points,
    };
    let make = |id: &str, pts: i32, rank: u32| Standing {
        id: id.to_owned(),
        name: name_of(id),
        rank,
        points: pts,
        best_time: None,
    };
    if blue_wins {
        vec![make(blue, blue_points, 1), make(red, red_points, 2)]
    } else {
        vec![make(red, red_points, 1), make(blue, blue_points, 2)]
    }
}

#[cfg(test)]
mod tests {
    use pitwall_core::gbx::callbacks::{PlayerScore, TeamScore};

    use super::*;

    fn player(login: &str, rank: i32, time: i32) -> PlayerScore {
        PlayerScore {
            login: login.into(),
            name: login.to_uppercase(),
            rank,
            bestracetime: time,
            ..PlayerScore::default()
        }
    }

    #[test]
    fn ranks_by_game_rank_and_appends_absent_players() {
        let scores = Scores {
            players: vec![
                player("b", 2, 51_000),
                player("a", 1, 50_000),
                player("x", 0, -1),
                player("spect", 3, 0),
            ],
            ..Scores::default()
        };
        let participants = vec!["a".to_owned(), "b".into(), "x".into(), "absent".into()];
        let standings = rank_players(&scores, &participants, |l| format!("<{l}>"));
        let ids: Vec<_> = standings.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "x", "absent"]);
        assert_eq!(standings[0].best_time, Some(50_000));
        assert_eq!(standings[2].best_time, None);
        assert_eq!(standings[3].name, "<absent>");
        assert_eq!(standings[3].rank, 4);
    }

    #[test]
    fn team_match_winner() {
        let scores = Scores {
            winnerteam: 1,
            teams: vec![
                TeamScore {
                    id: 0,
                    matchpoints: 1,
                    ..TeamScore::default()
                },
                TeamScore {
                    id: 1,
                    matchpoints: 2,
                    ..TeamScore::default()
                },
            ],
            ..Scores::default()
        };
        let standings = rank_teams(&scores, "kc", "vit", str::to_owned);
        assert_eq!(standings[0].id, "vit");
        assert_eq!(standings[0].points, 2);
        assert_eq!(standings[1].id, "kc");
    }
}
