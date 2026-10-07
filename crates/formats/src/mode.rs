//! Official TM2020 mode scripts and their short names.

/// A mode script shipped with the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownMode {
    /// Short name used in commands and formats.
    pub id: &'static str,
    pub aliases: &'static [&'static str],
    /// Value for `SetScriptName` / MatchSettings.
    pub script: &'static str,
    /// Uses blue/red teams.
    pub teams: bool,
    /// Round-based (points repartition, `ForceEndRound` apply).
    pub rounds: bool,
}

/// Script names as used by existing controllers (EvoSC#, PyPlanet, GbxRemoteGo).
/// TMWC/TMWT/Royal/Stunt/Platform names come from community sources only.
pub const KNOWN_MODES: &[KnownMode] = &[
    KnownMode {
        id: "timeattack",
        aliases: &["ta"],
        script: "Trackmania/TM_TimeAttack_Online.Script.txt",
        teams: false,
        rounds: false,
    },
    KnownMode {
        id: "rounds",
        aliases: &[],
        script: "Trackmania/TM_Rounds_Online.Script.txt",
        teams: false,
        rounds: true,
    },
    KnownMode {
        id: "cup",
        aliases: &[],
        script: "Trackmania/TM_Cup_Online.Script.txt",
        teams: false,
        rounds: true,
    },
    KnownMode {
        id: "teams",
        aliases: &["team"],
        script: "Trackmania/TM_Teams_Online.Script.txt",
        teams: true,
        rounds: true,
    },
    KnownMode {
        id: "knockout",
        aliases: &["ko"],
        script: "Trackmania/TM_Knockout_Online.Script.txt",
        teams: false,
        rounds: true,
    },
    KnownMode {
        id: "laps",
        aliases: &[],
        script: "Trackmania/TM_Laps_Online.Script.txt",
        teams: false,
        rounds: false,
    },
    KnownMode {
        id: "champion",
        aliases: &[],
        script: "Trackmania/Deprecated/TM_Champion_Online.Script.txt",
        teams: false,
        rounds: true,
    },
    KnownMode {
        id: "royal",
        aliases: &["royaltimeattack"],
        script: "Trackmania/TM_RoyalTimeAttack_Online.Script.txt",
        teams: false,
        rounds: false,
    },
    KnownMode {
        id: "stunt",
        aliases: &[],
        script: "Trackmania/TM_StuntMulti_Online.Script.txt",
        teams: false,
        rounds: false,
    },
    KnownMode {
        id: "platform",
        aliases: &[],
        script: "Trackmania/TM_Platform_Online.Script.txt",
        teams: false,
        rounds: false,
    },
    KnownMode {
        id: "tmwt",
        aliases: &["tmwtteams"],
        script: "TrackMania/TM_TMWTTeams_Online.Script.txt",
        teams: true,
        rounds: true,
    },
    KnownMode {
        id: "tmwc",
        aliases: &[],
        script: "TrackMania/TM_TMWC2023_Online.Script.txt",
        teams: false,
        rounds: true,
    },
];

/// Finds a known mode by id or alias (case-insensitive).
pub fn find_mode(name: &str) -> Option<&'static KnownMode> {
    let name = name.to_ascii_lowercase();
    KNOWN_MODES
        .iter()
        .find(|m| m.id == name || m.aliases.contains(&name.as_str()))
}

/// Finds a known mode from a script name, whatever its spelling.
pub fn mode_of_script(script: &str) -> Option<&'static KnownMode> {
    let wanted = pitwall_core::matchsettings::normalize_script_name(script);
    KNOWN_MODES
        .iter()
        .find(|m| pitwall_core::matchsettings::normalize_script_name(m.script) == wanted)
}

/// Resolves a mode name to a script: known id/alias, or a custom script path (contains `/`).
pub fn resolve_script(name: &str) -> Option<String> {
    if let Some(mode) = find_mode(name) {
        return Some(mode.script.to_owned());
    }
    if name.contains('/') {
        return Some(name.to_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_aliases_and_custom_scripts() {
        assert_eq!(
            resolve_script("TA").as_deref(),
            Some("Trackmania/TM_TimeAttack_Online.Script.txt")
        );
        assert_eq!(
            resolve_script("ko").as_deref(),
            Some("Trackmania/TM_Knockout_Online.Script.txt")
        );
        assert_eq!(
            resolve_script("Modes/Custom/My.Script.txt").as_deref(),
            Some("Modes/Custom/My.Script.txt")
        );
        assert_eq!(resolve_script("nope"), None);
        assert_eq!(
            mode_of_script("Trackmania/TM_Cup_Online").map(|m| m.id),
            Some("cup")
        );
    }
}
