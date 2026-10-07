//! Scenario tests of the cup engine.

use std::sync::Arc;

use pitwall_core::Text;
use pitwall_core::gbx::callbacks::{PlayerScore, Scores, TeamScore};

use super::def::CupDef;
use super::engine::{CupAction, CupEngine, CupInput, CupStatus, ParticipantStatus, PauseReason};
use super::{CupError, RegistrationMode};

fn def(source: &str) -> CupDef {
    CupDef::from_toml(source, "test.toml").unwrap()
}

fn has_key(actions: &[CupAction], wanted: &str) -> bool {
    actions.iter().any(|a| match a {
        CupAction::Announce(Text::Key { key, .. })
        | CupAction::Notify {
            text: Text::Key { key, .. },
            ..
        } => key == wanted,
        _ => false,
    })
}

fn applied_format(actions: &[CupAction]) -> Option<&str> {
    actions.iter().find_map(|a| match a {
        CupAction::ApplyFormat { format, .. } => Some(format.as_str()),
        _ => None,
    })
}

fn racing(actions: &[CupAction]) -> Option<Vec<String>> {
    actions.iter().rev().find_map(|a| match a {
        CupAction::Place { racing } => Some(racing.clone()),
        _ => None,
    })
}

fn end_match(players: &[(&str, i32)]) -> CupInput {
    CupInput::Scores(Box::new(Scores {
        section: "EndMatch".into(),
        players: players
            .iter()
            .map(|(login, rank)| PlayerScore {
                login: (*login).into(),
                rank: *rank,
                bestracetime: 50_000 + rank * 100,
                ..PlayerScore::default()
            })
            .collect(),
        ..Scores::default()
    }))
}

const TWO_PHASES: &str = r#"
id = "mini"
name = { en = "Mini cup", fr = "Mini coupe" }
participants = ["a", "b", "c", "d"]

[[phase]]
id = "quali"
format = "timeattack"
settings = { S_TimeLimit = 600 }
qualify = { top = 2 }

[[phase]]
id = "final"
format = "cup"
"#;

#[test]
fn qualifiers_then_final() {
    let mut cup = CupEngine::new(def(TWO_PHASES), None);
    assert_eq!(cup.state().status, CupStatus::Draft);

    let out = cup
        .handle(CupInput::Start { connected: vec![] }, 0)
        .unwrap();
    assert!(has_key(&out, "cup.started"));
    assert!(
        has_key(&out, "cup.waiting_admin"),
        "manual start by default"
    );
    assert_eq!(cup.state().status, CupStatus::WaitingStart { phase: 0 });
    assert_eq!(racing(&out).unwrap(), ["a", "b", "c", "d"]);

    let out = cup.handle(CupInput::Go, 1).unwrap();
    assert_eq!(applied_format(&out), Some("timeattack"));
    assert_eq!(cup.state().status, CupStatus::Loading { phase: 0 });

    cup.handle(CupInput::ModeLoaded, 2).unwrap();
    assert_eq!(cup.state().status, CupStatus::Running { phase: 0 });

    // Scores that do not end the match are only stored.
    let live = Scores {
        section: "EndRound".into(),
        ..Scores::default()
    };
    assert!(
        cup.handle(CupInput::Scores(Box::new(live)), 3)
            .unwrap()
            .is_empty()
    );

    let out = cup
        .handle(
            end_match(&[("c", 1), ("a", 2), ("d", 3), ("b", 4), ("spectator", 5)]),
            4,
        )
        .unwrap();
    let result = out
        .iter()
        .find_map(|a| match a {
            CupAction::PhaseCompleted(r) => Some(r.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(result.qualified, ["c", "a"]);
    assert_eq!(result.standings.len(), 4, "only participants are ranked");
    assert!(has_key(&out, "cup.eliminated"));
    assert_eq!(cup.state().status, CupStatus::WaitingStart { phase: 1 });
    assert_eq!(racing(&out).unwrap(), ["c", "a"]);

    let out = cup.handle(CupInput::Go, 5).unwrap();
    assert_eq!(applied_format(&out), Some("cup"));
    cup.handle(CupInput::ModeLoaded, 6).unwrap();
    let out = cup.handle(end_match(&[("a", 1), ("c", 2)]), 7).unwrap();
    assert!(has_key(&out, "cup.finished"));
    assert!(out.iter().any(|a| matches!(a, CupAction::ReleasePlacement)));
    let CupAction::Finished { standings } = out.last().unwrap() else {
        panic!("expected Finished last, got {out:?}");
    };
    assert_eq!(standings[0].id, "a");
    assert_eq!(cup.state().status, CupStatus::Finished);
    assert_eq!(cup.state().results.len(), 2);
}

#[test]
fn ready_check_and_timeout() {
    let source = TWO_PHASES.replace(
        "participants = [",
        "start = \"ready_check\"\nready_timeout_secs = 60\nparticipants = [",
    );
    let mut cup = CupEngine::new(def(&source), None);
    let out = cup
        .handle(CupInput::Start { connected: vec![] }, 0)
        .unwrap();
    assert!(out.iter().any(|a| matches!(
        a,
        CupAction::ReadyCheck {
            deadline_ms: 60_000,
            ..
        }
    )));

    assert!(matches!(
        cup.handle(
            CupInput::Ready {
                login: "stranger".into()
            },
            1
        ),
        Err(CupError::NotInMatch(_))
    ));
    for login in ["a", "b", "c"] {
        let out = cup
            .handle(
                CupInput::Ready {
                    login: login.into(),
                },
                1,
            )
            .unwrap();
        assert!(applied_format(&out).is_none());
    }
    let out = cup
        .handle(CupInput::Ready { login: "d".into() }, 2)
        .unwrap();
    assert_eq!(
        applied_format(&out),
        Some("timeattack"),
        "everyone ready → go"
    );

    // Next phase: nobody clicks, the timeout starts it.
    cup.handle(CupInput::ModeLoaded, 3).unwrap();
    cup.handle(end_match(&[("a", 1), ("b", 2)]), 10_000)
        .unwrap();
    assert!(cup.handle(CupInput::Tick, 20_000).unwrap().is_empty());
    let out = cup.handle(CupInput::Tick, 70_000).unwrap();
    assert!(has_key(&out, "cup.ready_timeout"));
    assert_eq!(applied_format(&out), Some("cup"));
}

#[test]
fn auto_start_with_delay() {
    let source = TWO_PHASES.replace(
        "format = \"cup\"",
        "format = \"cup\"\nstart = \"auto\"\ndelay_before_secs = 30",
    );
    let mut cup = CupEngine::new(def(&source), None);
    cup.handle(CupInput::Start { connected: vec![] }, 0)
        .unwrap();
    cup.handle(CupInput::Go, 0).unwrap();
    cup.handle(CupInput::ModeLoaded, 0).unwrap();
    let out = cup.handle(end_match(&[("a", 1), ("b", 2)]), 1_000).unwrap();
    assert!(has_key(&out, "cup.starting_in"));
    assert!(cup.handle(CupInput::Tick, 10_000).unwrap().is_empty());
    let out = cup.handle(CupInput::Tick, 31_000).unwrap();
    assert_eq!(applied_format(&out), Some("cup"));
}

#[test]
fn registration_modes_and_approval() {
    let source = TWO_PHASES.replace(
        "participants = [\"a\", \"b\", \"c\", \"d\"]",
        "participants = [\"a\"]\nregistration = [\"list\", \"ingame\"]\napproval = true\nmax_participants = 3",
    );
    let mut cup = CupEngine::new(def(&source), None);
    cup.handle(CupInput::OpenRegistration, 0).unwrap();
    let web = CupInput::Register {
        id: "w".into(),
        name: "W".into(),
        players: vec![],
        via: RegistrationMode::Web,
    };
    assert!(matches!(
        cup.handle(web, 0),
        Err(CupError::RegistrationNotAllowed(RegistrationMode::Web))
    ));

    let out = cup
        .handle(
            CupInput::Register {
                id: "b".into(),
                name: "B".into(),
                players: vec![],
                via: RegistrationMode::InGame,
            },
            0,
        )
        .unwrap();
    assert!(has_key(&out, "cup.pending"));
    assert_eq!(
        cup.state().participants[1].status,
        ParticipantStatus::Pending
    );
    // Admin additions skip approval.
    cup.handle(
        CupInput::Register {
            id: "c".into(),
            name: "C".into(),
            players: vec![],
            via: RegistrationMode::List,
        },
        0,
    )
    .unwrap();
    assert!(matches!(
        cup.handle(
            CupInput::Register {
                id: "d".into(),
                name: "D".into(),
                players: vec![],
                via: RegistrationMode::List
            },
            0
        ),
        Err(CupError::Full)
    ));
    assert!(matches!(
        cup.handle(
            CupInput::Register {
                id: "a".into(),
                name: "A".into(),
                players: vec![],
                via: RegistrationMode::List
            },
            0
        ),
        Err(CupError::AlreadyRegistered(_))
    ));

    // Pending players do not race until approved.
    let out = cup
        .handle(CupInput::Start { connected: vec![] }, 0)
        .unwrap();
    assert_eq!(racing(&out).unwrap(), ["a", "c"]);
}

#[test]
fn open_cups_take_connected_players() {
    let source = TWO_PHASES.replace(
        "participants = [\"a\", \"b\", \"c\", \"d\"]",
        "registration = [\"open\"]",
    );
    let mut cup = CupEngine::new(def(&source), None);
    assert!(matches!(
        cup.handle(CupInput::Start { connected: vec![] }, 0),
        Err(CupError::NoParticipants)
    ));
    let out = cup
        .handle(
            CupInput::Start {
                connected: vec![("x".into(), "$f00X".into()), ("y".into(), "Y".into())],
            },
            0,
        )
        .unwrap();
    assert_eq!(racing(&out).unwrap(), ["x", "y"]);
    assert_eq!(cup.state().participants[0].name, "$f00X");
}

#[test]
fn disconnection_pause_then_resume() {
    let source = TWO_PHASES.replace(
        "participants = [",
        "disconnect = { policy = \"pause\", grace_secs = 60 }\nparticipants = [",
    );
    let mut cup = CupEngine::new(def(&source), None);
    cup.handle(CupInput::Start { connected: vec![] }, 0)
        .unwrap();
    cup.handle(CupInput::Go, 0).unwrap();
    cup.handle(CupInput::ModeLoaded, 0).unwrap();

    // Spectators leaving do not matter.
    assert!(
        cup.handle(
            CupInput::PlayerLeft {
                login: "viewer".into()
            },
            5
        )
        .unwrap()
        .is_empty()
    );

    let out = cup
        .handle(CupInput::PlayerLeft { login: "b".into() }, 10)
        .unwrap();
    assert!(out.contains(&CupAction::PauseMatch(true)));
    assert_eq!(
        cup.state().status,
        CupStatus::Paused {
            phase: 0,
            reason: PauseReason::Disconnect
        }
    );
    let out = cup
        .handle(CupInput::PlayerJoined { login: "b".into() }, 20)
        .unwrap();
    assert!(out.contains(&CupAction::PauseMatch(false)));
    assert_eq!(cup.state().status, CupStatus::Running { phase: 0 });

    // Not back in time: the match resumes without them.
    cup.handle(CupInput::PlayerLeft { login: "c".into() }, 100)
        .unwrap();
    assert!(cup.handle(CupInput::Tick, 120).unwrap().is_empty());
    let out = cup.handle(CupInput::Tick, 60_200).unwrap();
    assert!(out.contains(&CupAction::PauseMatch(false)));
}

#[test]
fn disconnection_eliminates_after_grace() {
    let source = TWO_PHASES.replace(
        "participants = [",
        "disconnect = { policy = \"eliminate\", grace_secs = 30 }\nparticipants = [",
    );
    let mut cup = CupEngine::new(def(&source), None);
    cup.handle(CupInput::Start { connected: vec![] }, 0)
        .unwrap();
    cup.handle(CupInput::Go, 0).unwrap();
    cup.handle(CupInput::ModeLoaded, 0).unwrap();
    let out = cup
        .handle(CupInput::PlayerLeft { login: "d".into() }, 0)
        .unwrap();
    assert!(has_key(&out, "cup.disconnected_grace"));
    let out = cup.handle(CupInput::Tick, 30_000).unwrap();
    assert!(has_key(&out, "cup.disqualified"));
    assert_eq!(racing(&out).unwrap(), ["a", "b", "c"]);
}

#[test]
fn admin_pause_advance_and_abort() {
    let mut cup = CupEngine::new(def(TWO_PHASES), None);
    assert!(matches!(
        cup.handle(CupInput::Pause, 0),
        Err(CupError::InvalidState("draft"))
    ));
    cup.handle(CupInput::Start { connected: vec![] }, 0)
        .unwrap();
    cup.handle(CupInput::Go, 0).unwrap();
    cup.handle(CupInput::ModeLoaded, 0).unwrap();
    assert!(
        cup.handle(CupInput::Pause, 1)
            .unwrap()
            .contains(&CupAction::PauseMatch(true))
    );
    assert!(
        cup.handle(CupInput::Resume, 2)
            .unwrap()
            .contains(&CupAction::PauseMatch(false))
    );

    // Advance with no scores ranks everyone in order and moves on.
    let out = cup.handle(CupInput::Advance, 3).unwrap();
    assert!(
        out.iter()
            .any(|a| matches!(a, CupAction::PhaseCompleted(_)))
    );
    assert_eq!(cup.state().status, CupStatus::WaitingStart { phase: 1 });

    let out = cup.handle(CupInput::Abort, 4).unwrap();
    assert!(out.contains(&CupAction::ReleasePlacement));
    assert_eq!(cup.state().status, CupStatus::Aborted);
    assert!(cup.handle(CupInput::Abort, 5).is_err());
}

const TEAMS: &str = r#"
id = "teams"
kind = "teams"

[[teams]]
id = "t1"
name = "Team One"
players = ["p1", "p2"]
[[teams]]
id = "t2"
players = ["p3", "p4"]
[[teams]]
id = "t3"
players = ["p5", "p6"]
[[teams]]
id = "t4"
players = ["p7", "p8"]

[[phase]]
id = "playoffs"
format = "teams"
kind = "bracket"
start = "auto"
"#;

fn team_win(blue_wins: bool) -> CupInput {
    CupInput::Scores(Box::new(Scores {
        section: "EndMatch".into(),
        winnerteam: if blue_wins { 0 } else { 1 },
        teams: vec![
            TeamScore {
                id: 0,
                matchpoints: i32::from(blue_wins),
                ..TeamScore::default()
            },
            TeamScore {
                id: 1,
                matchpoints: i32::from(!blue_wins),
                ..TeamScore::default()
            },
        ],
        ..Scores::default()
    }))
}

#[test]
fn teams_bracket() {
    let mut cup = CupEngine::new(def(TEAMS), None);
    let out = cup
        .handle(CupInput::Start { connected: vec![] }, 0)
        .unwrap();
    // Auto start: first match (t1 vs t4) loads right away with teams assigned.
    assert_eq!(applied_format(&out), Some("teams"));
    assert!(out.contains(&CupAction::AssignTeams {
        blue: vec!["p1".into(), "p2".into()],
        red: vec!["p7".into(), "p8".into()],
    }));
    assert_eq!(racing(&out).unwrap(), ["p1", "p2", "p7", "p8"]);

    cup.handle(CupInput::ModeLoaded, 1).unwrap();
    let out = cup.handle(team_win(false), 2).unwrap(); // t4 wins
    assert!(has_key(&out, "cup.match_won"));
    assert_eq!(applied_format(&out), Some("teams"), "next match (t2 vs t3)");
    assert_eq!(racing(&out).unwrap(), ["p3", "p4", "p5", "p6"]);

    cup.handle(CupInput::ModeLoaded, 3).unwrap();
    cup.handle(team_win(true), 4).unwrap(); // t2 wins
    cup.handle(CupInput::ModeLoaded, 5).unwrap();
    // Admin decides the final manually.
    assert!(matches!(
        cup.handle(CupInput::SetWinner { id: "t1".into() }, 6),
        Err(CupError::NotInMatch(_))
    ));
    let out = cup
        .handle(CupInput::SetWinner { id: "t4".into() }, 6)
        .unwrap();
    let CupAction::Finished { standings } = out.last().unwrap() else {
        panic!("{out:?}");
    };
    let ranks: Vec<_> = standings.iter().map(|s| (s.id.as_str(), s.rank)).collect();
    assert_eq!(ranks, [("t4", 1), ("t2", 2), ("t1", 3), ("t3", 3)]);
}

#[cfg(feature = "lua")]
#[test]
fn lua_qualification_and_points() {
    use super::rules::LuaRules;
    let rules = LuaRules::load(
        r#"
function odd_ranks(phase, standings)
  local out = {}
  for _, s in ipairs(standings) do
    if s.rank % 2 == 1 then out[#out + 1] = s.id end
  end
  return out
end
function on_phase_end(phase, standings, points)
  for _, s in ipairs(standings) do points[s.id] = (points[s.id] or 0) + 10 - s.rank end
  return { cup_points = points, messages = { "gg " .. phase } }
end
"#,
        "rules.lua",
    )
    .unwrap();
    let source = TWO_PHASES
        .replace("qualify = { top = 2 }", "qualify = { lua = \"odd_ranks\" }")
        .replace(
            "participants = [",
            "rules = \"rules.lua\"\nparticipants = [",
        );
    let mut cup = CupEngine::new(def(&source), Some(Arc::new(rules)));
    cup.handle(CupInput::Start { connected: vec![] }, 0)
        .unwrap();
    cup.handle(CupInput::Go, 0).unwrap();
    cup.handle(CupInput::ModeLoaded, 0).unwrap();
    let out = cup
        .handle(end_match(&[("a", 1), ("b", 2), ("c", 3), ("d", 4)]), 1)
        .unwrap();
    assert!(has_key(&out, "cup.rules_message"));
    assert_eq!(cup.state().results[0].qualified, ["a", "c"]);
    assert_eq!(cup.state().cup_points["a"], 9);
    assert_eq!(cup.state().cup_points["d"], 6);
}

#[test]
fn state_survives_a_restart() {
    let mut cup = CupEngine::new(def(TWO_PHASES), None);
    cup.handle(CupInput::Start { connected: vec![] }, 0)
        .unwrap();
    cup.handle(CupInput::Go, 0).unwrap();
    cup.handle(CupInput::ModeLoaded, 0).unwrap();
    let json = serde_json::to_string(cup.state()).unwrap();
    let restored = serde_json::from_str(&json).unwrap();
    let mut cup2 = CupEngine::restore(restored, None);
    assert_eq!(cup2.state().status, CupStatus::Running { phase: 0 });
    let out = cup2.handle(end_match(&[("a", 1), ("b", 2)]), 1).unwrap();
    assert!(
        out.iter()
            .any(|a| matches!(a, CupAction::PhaseCompleted(_)))
    );
}
