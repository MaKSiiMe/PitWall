//! Cups: chains of phases (each one a format) driven by a pure state machine.

pub mod bracket;
pub mod def;
pub mod engine;
pub mod rules;
pub mod standings;
#[cfg(test)]
mod tests;

pub use def::{CupDef, CupDefError, CupKind, RegistrationMode, StartPolicy};
pub use engine::{
    CupAction, CupEngine, CupError, CupInput, CupState, CupStatus, Participant, PhaseResult,
};
pub use rules::{CupRules, RulesError};
pub use standings::Standing;
