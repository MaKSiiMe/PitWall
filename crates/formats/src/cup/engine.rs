//! The cup state machine.
//!
//! Pure logic, no I/O: [`CupEngine::handle`] takes a [`CupInput`] (admin command, game event,
//! clock tick) and returns [`CupAction`]s for the cup plugin to execute.
//!
//! ```text
//! Draft ─open→ Registration ─start→ WaitingStart(i) ─go/ready/auto→ Loading(i) ─mode loaded→
//! Running(i) ─end of match→ (next phase: WaitingStart(i+1)) | Finished
//!                ↕ pause/resume (admin or disconnection)
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use pitwall_core::gbx::callbacks::Scores;
use pitwall_core::{Lang, Text};
use serde::{Deserialize, Serialize};

use crate::cup::bracket::Bracket;
use crate::cup::def::{
    CupDef, CupKind, DisconnectAction, PhaseDef, PhaseKind, PhaseParticipants, Qualify,
    RegistrationMode, StartPolicy,
};
use crate::cup::rules::{CupRules, RulesError};
use crate::cup::standings::{Standing, rank_players, rank_teams};

/// Registration status of a participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipantStatus {
    /// Waiting for an admin approval.
    Pending,
    Registered,
    Disqualified,
}

/// A player (players cups) or a team (teams cups).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    /// Login or team id.
    pub id: String,
    pub name: String,
    /// Logins that race for this participant (the login itself for players cups).
    pub players: Vec<String>,
    pub status: ParticipantStatus,
    pub via: RegistrationMode,
}

/// Why a running phase is paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    Admin,
    Disconnect,
}

/// Where the cup is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CupStatus {
    Draft,
    Registration,
    /// Phase prepared, waiting for an admin, the ready check or the auto delay.
    WaitingStart {
        phase: usize,
    },
    /// The phase's format is loading on the server.
    Loading {
        phase: usize,
    },
    Running {
        phase: usize,
    },
    Paused {
        phase: usize,
        reason: PauseReason,
    },
    Finished,
    Aborted,
}

impl CupStatus {
    pub fn phase(&self) -> Option<usize> {
        match *self {
            CupStatus::WaitingStart { phase }
            | CupStatus::Loading { phase }
            | CupStatus::Running { phase }
            | CupStatus::Paused { phase, .. } => Some(phase),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            CupStatus::Draft => "draft",
            CupStatus::Registration => "registration",
            CupStatus::WaitingStart { .. } => "waiting_start",
            CupStatus::Loading { .. } => "loading",
            CupStatus::Running { .. } => "running",
            CupStatus::Paused { .. } => "paused",
            CupStatus::Finished => "finished",
            CupStatus::Aborted => "aborted",
        }
    }

    fn is_over(&self) -> bool {
        matches!(self, CupStatus::Finished | CupStatus::Aborted)
    }
}

/// Result of a completed phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseResult {
    pub phase_index: usize,
    pub phase_id: String,
    pub standings: Vec<Standing>,
    pub qualified: Vec<String>,
    pub finished_at: i64,
}

/// Inputs of the state machine.
#[derive(Debug, Clone, PartialEq)]
pub enum CupInput {
    OpenRegistration,
    Register {
        id: String,
        name: String,
        /// Logins racing for this participant (empty = `[id]` for players cups).
        players: Vec<String>,
        via: RegistrationMode,
    },
    Approve {
        id: String,
    },
    Unregister {
        id: String,
    },
    /// Starts the cup. `connected` = `(login, nickname)` of connected players (for open cups).
    Start {
        connected: Vec<(String, String)>,
    },
    /// Admin starts the waiting phase now.
    Go,
    Ready {
        login: String,
    },
    Unready {
        login: String,
    },
    /// The phase's mode is running on the server.
    ModeLoaded,
    /// `Trackmania.Scores`; `section == "EndMatch"` ends the running match.
    Scores(Box<Scores>),
    /// Admin ends the current match/phase now with the last known scores.
    Advance,
    /// Admin sets the winner of the current bracket match.
    SetWinner {
        id: String,
    },
    Pause,
    Resume,
    PlayerLeft {
        login: String,
    },
    PlayerJoined {
        login: String,
    },
    Disqualify {
        id: String,
    },
    Reinstate {
        id: String,
    },
    /// Clock: handles timeouts.
    Tick,
    Abort,
}

/// Actions for the plugin to perform.
#[derive(Debug, Clone, PartialEq)]
pub enum CupAction {
    /// Chat message to everyone.
    Announce(Text),
    /// Chat message to some players.
    Notify {
        logins: Vec<String>,
        text: Text,
    },
    /// Switch to a format (by id) with phase overrides.
    ApplyFormat {
        format: String,
        settings: BTreeMap<String, toml::Value>,
        maps: Vec<String>,
    },
    /// These logins race; every other connected player spectates (if the cup locks the server).
    Place {
        racing: Vec<String>,
    },
    /// Everybody may race or spectate again.
    ReleasePlacement,
    AssignTeams {
        blue: Vec<String>,
        red: Vec<String>,
    },
    ReadyCheck {
        logins: Vec<String>,
        deadline_ms: i64,
    },
    EndReadyCheck,
    PauseMatch(bool),
    PhaseCompleted(PhaseResult),
    Finished {
        standings: Vec<Standing>,
    },
}

/// Errors of the state machine.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CupError {
    #[error("not possible while the cup is {0}")]
    InvalidState(&'static str),
    #[error("registrations via {0:?} are not allowed for this cup")]
    RegistrationNotAllowed(RegistrationMode),
    #[error("the cup is full")]
    Full,
    #[error("{0} is already registered")]
    AlreadyRegistered(String),
    #[error("unknown participant {0}")]
    UnknownParticipant(String),
    #[error("{0} is not in the current match")]
    NotInMatch(String),
    #[error("nobody is registered")]
    NoParticipants,
    #[error("format of phase {0} not found")]
    UnknownPhase(usize),
    #[error(transparent)]
    Rules(#[from] RulesError),
}

/// Serializable state (saved after every change, restored after a restart).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CupState {
    pub def: CupDef,
    pub status: CupStatus,
    /// In registration order.
    pub participants: Vec<Participant>,
    pub results: Vec<PhaseResult>,
    /// Participant ids playing the current phase.
    pub phase_ids: Vec<String>,
    pub bracket: Option<Bracket>,
    pub current_match: Option<(usize, usize)>,
    pub ready: BTreeSet<String>,
    pub deadline_ms: Option<i64>,
    /// Login → disconnection time, for participants of the running phase.
    pub disconnected: BTreeMap<String, i64>,
    pub cup_points: BTreeMap<String, i64>,
    pub last_scores: Option<Scores>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

/// The cup engine.
#[derive(Debug, Clone)]
pub struct CupEngine {
    state: CupState,
    rules: Option<Arc<dyn CupRules>>,
}

type Out = Vec<CupAction>;

impl CupEngine {
    /// A new cup in `Draft`, with the participants and teams of its definition.
    pub fn new(def: CupDef, rules: Option<Arc<dyn CupRules>>) -> CupEngine {
        let mut participants = Vec::new();
        match def.kind {
            CupKind::Players => {
                for login in &def.participants {
                    participants.push(Participant {
                        id: login.clone(),
                        name: login.clone(),
                        players: vec![login.clone()],
                        status: ParticipantStatus::Registered,
                        via: RegistrationMode::List,
                    });
                }
            }
            CupKind::Teams => {
                for team in &def.teams {
                    participants.push(Participant {
                        id: team.id.clone(),
                        name: if team.name.is_empty() {
                            team.id.clone()
                        } else {
                            team.name.clone()
                        },
                        players: team.players.clone(),
                        status: ParticipantStatus::Registered,
                        via: RegistrationMode::List,
                    });
                }
            }
        }
        CupEngine {
            state: CupState {
                def,
                status: CupStatus::Draft,
                participants,
                results: Vec::new(),
                phase_ids: Vec::new(),
                bracket: None,
                current_match: None,
                ready: BTreeSet::new(),
                deadline_ms: None,
                disconnected: BTreeMap::new(),
                cup_points: BTreeMap::new(),
                last_scores: None,
                started_at: None,
                finished_at: None,
            },
            rules,
        }
    }

    /// Restores a saved cup.
    pub fn restore(state: CupState, rules: Option<Arc<dyn CupRules>>) -> CupEngine {
        CupEngine { state, rules }
    }

    pub fn state(&self) -> &CupState {
        &self.state
    }

    pub fn def(&self) -> &CupDef {
        &self.state.def
    }

    /// The phase being played or prepared.
    pub fn current_phase(&self) -> Option<&PhaseDef> {
        self.state
            .status
            .phase()
            .and_then(|i| self.state.def.phases.get(i))
    }

    fn participant(&self, id: &str) -> Option<&Participant> {
        self.state.participants.iter().find(|p| p.id == id)
    }

    fn participant_mut(&mut self, id: &str) -> Option<&mut Participant> {
        self.state.participants.iter_mut().find(|p| p.id == id)
    }

    fn name_of(&self, id: &str) -> String {
        self.participant(id)
            .map_or_else(|| id.to_owned(), |p| p.name.clone())
    }

    /// Participant id a login races for.
    pub fn participant_of_login(&self, login: &str) -> Option<&Participant> {
        self.state
            .participants
            .iter()
            .find(|p| p.players.iter().any(|l| l == login))
    }

    fn active_ids(&self) -> Vec<String> {
        self.state
            .participants
            .iter()
            .filter(|p| p.status == ParticipantStatus::Registered)
            .map(|p| p.id.clone())
            .collect()
    }

    /// Logins that race right now (current phase, or current bracket match).
    pub fn racing_logins(&self) -> Vec<String> {
        let ids: Vec<String> = match (&self.state.bracket, self.state.current_match) {
            (Some(bracket), Some((r, i))) => bracket
                .get(r, i)
                .map(|m| m.a.iter().chain(m.b.iter()).cloned().collect())
                .unwrap_or_default(),
            _ => self.state.phase_ids.clone(),
        };
        ids.iter()
            .filter_map(|id| self.participant(id))
            .filter(|p| p.status == ParticipantStatus::Registered)
            .flat_map(|p| p.players.clone())
            .collect()
    }

    fn phase(&self, index: usize) -> Result<&PhaseDef, CupError> {
        self.state
            .def
            .phases
            .get(index)
            .ok_or(CupError::UnknownPhase(index))
    }

    fn start_policy(&self, index: usize) -> StartPolicy {
        self.state
            .def
            .phases
            .get(index)
            .and_then(|p| p.start)
            .unwrap_or(self.state.def.start)
    }

    /// Localized phase name (falls back to its id).
    fn phase_text(&self, index: usize) -> Text {
        match self.state.def.phases.get(index) {
            Some(p) => Text::localized(p.display_name(Lang::En), p.display_name(Lang::Fr)),
            None => Text::raw(""),
        }
    }

    /// Localized cup name.
    fn cup_text(&self) -> Text {
        let def = &self.state.def;
        Text::localized(def.display_name(Lang::En), def.display_name(Lang::Fr))
    }

    /// Handles one input at time `now` (Unix ms).
    pub fn handle(&mut self, input: CupInput, now: i64) -> Result<Out, CupError> {
        let status = self.state.status;
        let mut out = Vec::new();
        match input {
            CupInput::OpenRegistration => {
                if status != CupStatus::Draft {
                    return Err(CupError::InvalidState(status.name()));
                }
                self.state.status = CupStatus::Registration;
                out.push(CupAction::Announce(
                    Text::tr("cup.registration_open").arg_text("cup", self.cup_text()),
                ));
            }
            CupInput::Register {
                id,
                name,
                players,
                via,
            } => self.register(id, name, players, via, &mut out)?,
            CupInput::Approve { id } => {
                let participant = self
                    .participant_mut(&id)
                    .ok_or(CupError::UnknownParticipant(id.clone()))?;
                if participant.status == ParticipantStatus::Pending {
                    participant.status = ParticipantStatus::Registered;
                    let name = participant.name.clone();
                    out.push(CupAction::Announce(
                        Text::tr("cup.registered").arg("name", name),
                    ));
                }
            }
            CupInput::Unregister { id } => {
                if !matches!(status, CupStatus::Draft | CupStatus::Registration) {
                    return Err(CupError::InvalidState(status.name()));
                }
                let before = self.state.participants.len();
                self.state.participants.retain(|p| p.id != id);
                if self.state.participants.len() == before {
                    return Err(CupError::UnknownParticipant(id));
                }
            }
            CupInput::Start { connected } => {
                if !matches!(status, CupStatus::Draft | CupStatus::Registration) {
                    return Err(CupError::InvalidState(status.name()));
                }
                if self.state.def.allows(RegistrationMode::Open)
                    && self.state.def.kind == CupKind::Players
                {
                    for (login, nick) in connected {
                        if self.participant(&login).is_none() {
                            self.state.participants.push(Participant {
                                id: login.clone(),
                                name: nick,
                                players: vec![login],
                                status: ParticipantStatus::Registered,
                                via: RegistrationMode::Open,
                            });
                        }
                    }
                }
                if self.active_ids().is_empty() {
                    return Err(CupError::NoParticipants);
                }
                self.state.started_at = Some(now);
                out.push(CupAction::Announce(
                    Text::tr("cup.started")
                        .arg_text("cup", self.cup_text())
                        .arg("count", self.active_ids().len()),
                ));
                self.prepare_phase(0, now, &mut out)?;
            }
            CupInput::Go => match status {
                CupStatus::WaitingStart { phase } => self.begin(phase, &mut out)?,
                _ => return Err(CupError::InvalidState(status.name())),
            },
            CupInput::Ready { login } => {
                let CupStatus::WaitingStart { phase } = status else {
                    return Err(CupError::InvalidState(status.name()));
                };
                if self.start_policy(phase) != StartPolicy::ReadyCheck {
                    return Err(CupError::InvalidState(status.name()));
                }
                let racing = self.racing_logins();
                if !racing.contains(&login) {
                    return Err(CupError::NotInMatch(login));
                }
                self.state.ready.insert(login);
                let ready = racing
                    .iter()
                    .filter(|l| self.state.ready.contains(*l))
                    .count();
                out.push(CupAction::Notify {
                    logins: racing.clone(),
                    text: Text::tr("cup.ready_progress")
                        .arg("ready", ready)
                        .arg("total", racing.len()),
                });
                if ready == racing.len() {
                    self.begin(phase, &mut out)?;
                }
            }
            CupInput::Unready { login } => {
                self.state.ready.remove(&login);
            }
            CupInput::ModeLoaded => {
                if let CupStatus::Loading { phase } = status {
                    self.state.status = CupStatus::Running { phase };
                    self.state.disconnected.clear();
                    self.place(&mut out);
                }
            }
            CupInput::Scores(scores) => {
                let ends_match = scores.section == "EndMatch";
                self.state.last_scores = Some(*scores);
                if ends_match
                    && matches!(status, CupStatus::Running { .. } | CupStatus::Paused { .. })
                {
                    self.end_match(now, &mut out)?;
                }
            }
            CupInput::Advance => {
                if !matches!(
                    status,
                    CupStatus::Running { .. }
                        | CupStatus::Paused { .. }
                        | CupStatus::Loading { .. }
                ) {
                    return Err(CupError::InvalidState(status.name()));
                }
                self.end_match(now, &mut out)?;
            }
            CupInput::SetWinner { id } => {
                let (Some(bracket), Some((r, i))) =
                    (&mut self.state.bracket, self.state.current_match)
                else {
                    return Err(CupError::InvalidState(status.name()));
                };
                if !bracket.record(r, i, &id) {
                    return Err(CupError::NotInMatch(id));
                }
                self.after_bracket_match(&id, now, &mut out)?;
            }
            CupInput::Pause => match status {
                CupStatus::Running { phase } => {
                    self.state.status = CupStatus::Paused {
                        phase,
                        reason: PauseReason::Admin,
                    };
                    out.push(CupAction::PauseMatch(true));
                    out.push(CupAction::Announce(Text::tr("cup.paused")));
                }
                _ => return Err(CupError::InvalidState(status.name())),
            },
            CupInput::Resume => match status {
                CupStatus::Paused { phase, .. } => {
                    self.state.status = CupStatus::Running { phase };
                    out.push(CupAction::PauseMatch(false));
                    out.push(CupAction::Announce(Text::tr("cup.resumed")));
                }
                _ => return Err(CupError::InvalidState(status.name())),
            },
            CupInput::PlayerLeft { login } => self.player_left(login, now, &mut out),
            CupInput::PlayerJoined { login } => self.player_joined(&login, &mut out),
            CupInput::Disqualify { id } => {
                let participant = self
                    .participant_mut(&id)
                    .ok_or(CupError::UnknownParticipant(id.clone()))?;
                participant.status = ParticipantStatus::Disqualified;
                let name = participant.name.clone();
                self.state.phase_ids.retain(|p| *p != id);
                out.push(CupAction::Announce(
                    Text::tr("cup.disqualified").arg("name", name),
                ));
                if matches!(
                    status,
                    CupStatus::Running { .. }
                        | CupStatus::Paused { .. }
                        | CupStatus::Loading { .. }
                ) {
                    self.place(&mut out);
                }
            }
            CupInput::Reinstate { id } => {
                let participant = self
                    .participant_mut(&id)
                    .ok_or(CupError::UnknownParticipant(id.clone()))?;
                participant.status = ParticipantStatus::Registered;
                if !self.state.phase_ids.contains(&id) && status.phase().is_some() {
                    self.state.phase_ids.push(id);
                    self.place(&mut out);
                }
            }
            CupInput::Tick => self.tick(now, &mut out)?,
            CupInput::Abort => {
                if status.is_over() {
                    return Err(CupError::InvalidState(status.name()));
                }
                self.state.status = CupStatus::Aborted;
                self.state.finished_at = Some(now);
                out.push(CupAction::EndReadyCheck);
                out.push(CupAction::ReleasePlacement);
                out.push(CupAction::Announce(Text::tr("cup.aborted")));
            }
        }
        Ok(out)
    }

    fn register(
        &mut self,
        id: String,
        name: String,
        players: Vec<String>,
        via: RegistrationMode,
        out: &mut Out,
    ) -> Result<(), CupError> {
        let status = self.state.status;
        if !matches!(status, CupStatus::Draft | CupStatus::Registration) {
            return Err(CupError::InvalidState(status.name()));
        }
        // Admins add participants whatever the modes (`List`); self-registrations need the mode.
        if via != RegistrationMode::List && !self.state.def.allows(via) {
            return Err(CupError::RegistrationNotAllowed(via));
        }
        if self.participant(&id).is_some() {
            return Err(CupError::AlreadyRegistered(id));
        }
        if let Some(max) = self.state.def.max_participants
            && self.state.participants.len() >= max
        {
            return Err(CupError::Full);
        }
        let players = if players.is_empty() {
            vec![id.clone()]
        } else {
            players
        };
        for login in &players {
            if let Some(other) = self.participant_of_login(login) {
                return Err(CupError::AlreadyRegistered(format!(
                    "{login} ({})",
                    other.name
                )));
            }
        }
        let status = if self.state.def.approval && via != RegistrationMode::List {
            ParticipantStatus::Pending
        } else {
            ParticipantStatus::Registered
        };
        let key = if status == ParticipantStatus::Pending {
            "cup.pending"
        } else {
            "cup.registered"
        };
        out.push(CupAction::Announce(Text::tr(key).arg("name", &name)));
        self.state.participants.push(Participant {
            id,
            name,
            players,
            status,
            via,
        });
        Ok(())
    }

    /// Participants of phase `index`, seeded.
    fn phase_participants(&self, index: usize) -> Result<Vec<String>, CupError> {
        let phase = self.phase(index)?;
        let active = self.active_ids();
        let mut ids: Vec<String> = match (index, phase.participants, self.state.results.last()) {
            (0, _, _) | (_, PhaseParticipants::All, _) | (_, _, None) => active.clone(),
            (_, PhaseParticipants::Previous, Some(previous)) => previous
                .qualified
                .iter()
                .filter(|id| active.contains(id))
                .cloned()
                .collect(),
        };
        if let Some(rules) = &self.rules
            && let Some(seeded) = rules.seed(&phase.id, &ids)?
        {
            ids = seeded
                .into_iter()
                .filter(|id| active.contains(id))
                .collect();
        }
        Ok(ids)
    }

    fn prepare_phase(&mut self, index: usize, now: i64, out: &mut Out) -> Result<(), CupError> {
        let ids = self.phase_participants(index)?;
        let phase = self.phase(index)?.clone();
        self.state.phase_ids = ids.clone();
        self.state.bracket = None;
        self.state.current_match = None;
        if phase.kind == PhaseKind::Bracket {
            let bracket = Bracket::new(&ids);
            self.state.current_match = bracket.next_match();
            self.state.bracket = Some(bracket);
        }
        out.push(CupAction::Announce(
            Text::tr("cup.phase_next")
                .arg_text("phase", self.phase_text(index))
                .arg("count", ids.len()),
        ));
        self.wait_start(index, now, out)
    }

    /// Enters `WaitingStart` for the phase (or the next bracket match).
    fn wait_start(&mut self, index: usize, now: i64, out: &mut Out) -> Result<(), CupError> {
        self.state.status = CupStatus::WaitingStart { phase: index };
        self.state.ready.clear();
        self.state.deadline_ms = None;
        if let (Some(bracket), Some((r, i))) = (&self.state.bracket, self.state.current_match)
            && let Some(m) = bracket.get(r, i)
        {
            let a =
                m.a.as_deref()
                    .map(|id| self.name_of(id))
                    .unwrap_or_default();
            let b =
                m.b.as_deref()
                    .map(|id| self.name_of(id))
                    .unwrap_or_default();
            out.push(CupAction::Announce(
                Text::tr("cup.match_next").arg("a", a).arg("b", b),
            ));
        }
        self.place(out);
        let delay_before_secs = self.phase(index)?.delay_before_secs;
        match self.start_policy(index) {
            StartPolicy::Manual => out.push(CupAction::Announce(Text::tr("cup.waiting_admin"))),
            StartPolicy::ReadyCheck => {
                let deadline = now + secs_to_ms(self.state.def.ready_timeout_secs);
                self.state.deadline_ms = Some(deadline);
                out.push(CupAction::ReadyCheck {
                    logins: self.racing_logins(),
                    deadline_ms: deadline,
                });
                out.push(CupAction::Announce(
                    Text::tr("cup.ready_check").arg("seconds", self.state.def.ready_timeout_secs),
                ));
            }
            StartPolicy::Auto => {
                if delay_before_secs == 0 {
                    return self.begin(index, out);
                }
                self.state.deadline_ms = Some(now + secs_to_ms(delay_before_secs));
                out.push(CupAction::Announce(
                    Text::tr("cup.starting_in").arg("seconds", delay_before_secs),
                ));
            }
        }
        Ok(())
    }

    /// Loads the phase's format.
    fn begin(&mut self, index: usize, out: &mut Out) -> Result<(), CupError> {
        let phase = self.phase(index)?.clone();
        self.state.status = CupStatus::Loading { phase: index };
        self.state.deadline_ms = None;
        self.state.ready.clear();
        out.push(CupAction::EndReadyCheck);
        out.push(CupAction::Announce(
            Text::tr("cup.phase_start").arg_text("phase", self.phase_text(index)),
        ));
        self.place(out);
        out.push(CupAction::ApplyFormat {
            format: phase.format.clone(),
            settings: phase.settings.clone(),
            maps: phase.maps.clone(),
        });
        Ok(())
    }

    /// Placement (and teams for blue/red matches).
    fn place(&self, out: &mut Out) {
        out.push(CupAction::Place {
            racing: self.racing_logins(),
        });
        if self.state.def.kind != CupKind::Teams {
            return;
        }
        let ids: Vec<String> = match (&self.state.bracket, self.state.current_match) {
            (Some(bracket), Some((r, i))) => bracket
                .get(r, i)
                .map(|m| m.a.iter().chain(m.b.iter()).cloned().collect())
                .unwrap_or_default(),
            _ => self.state.phase_ids.clone(),
        };
        if let [blue, red] = ids.as_slice() {
            let players = |id: &String| {
                self.participant(id)
                    .map(|p| p.players.clone())
                    .unwrap_or_default()
            };
            out.push(CupAction::AssignTeams {
                blue: players(blue),
                red: players(red),
            });
        }
    }

    fn end_match(&mut self, now: i64, out: &mut Out) -> Result<(), CupError> {
        let Some(index) = self.state.status.phase() else {
            return Err(CupError::InvalidState(self.state.status.name()));
        };
        let scores = self.state.last_scores.clone().unwrap_or_default();

        // Bracket: one match done.
        if let (Some(bracket), Some((r, i))) = (&self.state.bracket, self.state.current_match) {
            let Some(m) = bracket.get(r, i).cloned() else {
                return Err(CupError::InvalidState("bracket"));
            };
            let (Some(a), Some(b)) = (m.a, m.b) else {
                return Err(CupError::InvalidState("bracket"));
            };
            let ranking = rank_teams(&scores, &a, &b, |id| self.name_of(id));
            let winner = ranking[0].id.clone();
            if let Some(bracket) = &mut self.state.bracket {
                bracket.record(r, i, &winner);
            }
            return self.after_bracket_match(&winner, now, out);
        }

        let standings = match self.state.def.kind {
            CupKind::Players => {
                let logins = self.racing_logins();
                rank_players(&scores, &logins, |login| self.name_of(login))
            }
            CupKind::Teams => {
                let ids = self.state.phase_ids.clone();
                match ids.as_slice() {
                    [blue, red] => rank_teams(&scores, blue, red, |id| self.name_of(id)),
                    _ => ids
                        .iter()
                        .enumerate()
                        .map(|(i, id)| Standing {
                            id: id.clone(),
                            name: self.name_of(id),
                            rank: u32::try_from(i + 1).unwrap_or(u32::MAX),
                            points: 0,
                            best_time: None,
                        })
                        .collect(),
                }
            }
        };
        self.complete_phase(index, standings, now, out)
    }

    fn after_bracket_match(
        &mut self,
        winner: &str,
        now: i64,
        out: &mut Out,
    ) -> Result<(), CupError> {
        let Some(index) = self.state.status.phase() else {
            return Err(CupError::InvalidState(self.state.status.name()));
        };
        out.push(CupAction::Announce(
            Text::tr("cup.match_won").arg("winner", self.name_of(winner)),
        ));
        let next = self.state.bracket.as_ref().and_then(Bracket::next_match);
        self.state.current_match = next;
        if next.is_some() {
            return self.wait_start(index, now, out);
        }
        let standings = self
            .state
            .bracket
            .as_ref()
            .map(|b| b.standings(|id| self.name_of(id)))
            .unwrap_or_default();
        self.state.bracket = None;
        self.complete_phase(index, standings, now, out)
    }

    fn complete_phase(
        &mut self,
        index: usize,
        standings: Vec<Standing>,
        now: i64,
        out: &mut Out,
    ) -> Result<(), CupError> {
        let phase = self.phase(index)?.clone();
        let mut qualified: Vec<String> = match &phase.qualify {
            Qualify::All => standings.iter().map(|s| s.id.clone()).collect(),
            Qualify::Top(n) => standings.iter().take(*n).map(|s| s.id.clone()).collect(),
            Qualify::Lua(function) => match &self.rules {
                Some(rules) => rules.qualify(function, &phase.id, &standings)?,
                None => return Err(RulesError::MissingFunction(function.clone()).into()),
            },
        };
        qualified.retain(|id| standings.iter().any(|s| &s.id == id));

        let mut messages = Vec::new();
        if let Some(rules) = &self.rules {
            let outcome = rules.on_phase_end(&phase.id, &standings, &self.state.cup_points)?;
            if let Some(points) = outcome.cup_points {
                self.state.cup_points = points;
            }
            messages = outcome.messages;
        }

        out.push(CupAction::Announce(
            Text::tr("cup.phase_results").arg_text("phase", self.phase_text(index)),
        ));
        for s in standings.iter().take(3) {
            out.push(CupAction::Announce(
                Text::tr("cup.result_line")
                    .arg("rank", s.rank)
                    .arg("name", &s.name)
                    .arg("points", s.points),
            ));
        }
        for message in messages {
            out.push(CupAction::Announce(
                Text::tr("cup.rules_message").arg("text", message),
            ));
        }

        let result = PhaseResult {
            phase_index: index,
            phase_id: phase.id.clone(),
            standings: standings.clone(),
            qualified: qualified.clone(),
            finished_at: now,
        };
        self.state.results.push(result.clone());
        out.push(CupAction::PhaseCompleted(result));

        let last = index + 1 >= self.state.def.phases.len();
        if last {
            self.state.status = CupStatus::Finished;
            self.state.finished_at = Some(now);
            out.push(CupAction::ReleasePlacement);
            out.push(CupAction::Announce(
                Text::tr("cup.finished").arg_text("cup", self.cup_text()),
            ));
            for s in standings.iter().take(3) {
                out.push(CupAction::Announce(
                    Text::tr("cup.podium")
                        .arg("rank", s.rank)
                        .arg("name", &s.name),
                ));
            }
            out.push(CupAction::Finished { standings });
            return Ok(());
        }

        // Tell the eliminated ones.
        let eliminated: Vec<String> = standings
            .iter()
            .filter(|s| !qualified.contains(&s.id))
            .filter_map(|s| self.participant(&s.id))
            .flat_map(|p| p.players.clone())
            .collect();
        if !eliminated.is_empty() {
            out.push(CupAction::Notify {
                logins: eliminated,
                text: Text::tr("cup.eliminated"),
            });
        }
        out.push(CupAction::Announce(
            Text::tr("cup.qualified").arg("count", qualified.len()),
        ));
        self.prepare_phase(index + 1, now, out)
    }

    fn player_left(&mut self, login: String, now: i64, out: &mut Out) {
        let CupStatus::Running { phase } = self.state.status else {
            return;
        };
        if !self.racing_logins().contains(&login) {
            return;
        }
        self.state.disconnected.insert(login.clone(), now);
        let name = self
            .participant_of_login(&login)
            .map_or_else(|| login.clone(), |p| p.name.clone());
        match self.state.def.disconnect.policy {
            DisconnectAction::Keep => {}
            DisconnectAction::Pause => {
                self.state.status = CupStatus::Paused {
                    phase,
                    reason: PauseReason::Disconnect,
                };
                out.push(CupAction::PauseMatch(true));
                out.push(CupAction::Announce(
                    Text::tr("cup.paused_disconnect")
                        .arg("name", name)
                        .arg("seconds", self.state.def.disconnect.grace_secs),
                ));
            }
            DisconnectAction::Eliminate => out.push(CupAction::Announce(
                Text::tr("cup.disconnected_grace")
                    .arg("name", name)
                    .arg("seconds", self.state.def.disconnect.grace_secs),
            )),
        }
    }

    fn player_joined(&mut self, login: &str, out: &mut Out) {
        let was_away = self.state.disconnected.remove(login).is_some();
        if self.state.status.phase().is_some() {
            // Re-apply the placement for the newcomer (racing or spectating).
            self.place(out);
        }
        if was_away
            && self.state.disconnected.is_empty()
            && let CupStatus::Paused {
                phase,
                reason: PauseReason::Disconnect,
            } = self.state.status
        {
            self.state.status = CupStatus::Running { phase };
            out.push(CupAction::PauseMatch(false));
            out.push(CupAction::Announce(Text::tr("cup.resumed")));
        }
    }

    fn tick(&mut self, now: i64, out: &mut Out) -> Result<(), CupError> {
        match self.state.status {
            CupStatus::WaitingStart { phase } => {
                if self.state.deadline_ms.is_some_and(|d| now >= d) {
                    if self.start_policy(phase) == StartPolicy::ReadyCheck {
                        out.push(CupAction::Announce(Text::tr("cup.ready_timeout")));
                    }
                    self.begin(phase, out)?;
                }
            }
            CupStatus::Running { phase } | CupStatus::Paused { phase, .. } => {
                let grace = secs_to_ms(self.state.def.disconnect.grace_secs);
                let expired: Vec<String> = self
                    .state
                    .disconnected
                    .iter()
                    .filter(|(_, since)| now - **since >= grace)
                    .map(|(login, _)| login.clone())
                    .collect();
                if expired.is_empty() {
                    return Ok(());
                }
                for login in &expired {
                    self.state.disconnected.remove(login);
                }
                match self.state.def.disconnect.policy {
                    DisconnectAction::Keep => {}
                    DisconnectAction::Pause => {
                        if self.state.disconnected.is_empty()
                            && matches!(
                                self.state.status,
                                CupStatus::Paused {
                                    reason: PauseReason::Disconnect,
                                    ..
                                }
                            )
                        {
                            self.state.status = CupStatus::Running { phase };
                            out.push(CupAction::PauseMatch(false));
                            out.push(CupAction::Announce(Text::tr("cup.resumed")));
                        }
                    }
                    DisconnectAction::Eliminate => {
                        for login in expired {
                            if let Some(id) =
                                self.participant_of_login(&login).map(|p| p.id.clone())
                            {
                                let mut more = self.handle(CupInput::Disqualify { id }, now)?;
                                out.append(&mut more);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn secs_to_ms(secs: u64) -> i64 {
    i64::try_from(secs.saturating_mul(1000)).unwrap_or(i64::MAX)
}
