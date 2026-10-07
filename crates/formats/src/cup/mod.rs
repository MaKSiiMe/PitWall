//! Cups: chains of phases (each one a format) driven by a pure state machine.

pub mod bracket;
pub mod def;
pub mod engine;
pub mod library;
pub mod rules;
pub mod standings;
#[cfg(test)]
mod tests;

pub use def::{CupDef, CupDefError, CupKind, RegistrationMode, StartPolicy};
pub use engine::{
    CupAction, CupEngine, CupError, CupInput, CupSnapshot, CupState, CupStatus, Participant,
    PhaseInfo, PhaseResult,
};
pub use library::{CupLibrary, LibraryError, LoadedCup};
pub use rules::{CupRules, RulesError};
pub use standings::Standing;
