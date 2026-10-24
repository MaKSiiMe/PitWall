//! Moderation and match control commands for referees and admins.

use pitwall_core::gbx::types::{ScriptSetting, ScriptSettings};
use pitwall_core::state::FindPlayerError;
use pitwall_core::{
    Command, CommandSpec, Event, Invocation, Lang, Plugin, PluginContext, PluginFuture, Role,
    SpectatorModeDto, Text,
};

/// The admin plugin.
#[derive(Debug, Default, Clone, Copy)]
pub struct AdminPlugin;

fn spec(name: &str, role: Role, usage: &str) -> CommandSpec {
    CommandSpec::new("admin", name, role).usage(usage)
}

impl Plugin for AdminPlugin {
    fn name(&self) -> &'static str {
        "admin"
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            spec("kick", Role::Referee, "/kick <player> [reason]"),
            spec("ban", Role::Admin, "/ban <player> [reason]"),
            spec("blacklist", Role::Admin, "/blacklist <player> [reason]"),
            spec("unban", Role::Admin, "/unban <login>"),
            spec("mute", Role::Referee, "/mute <player>"),
            spec("unmute", Role::Referee, "/unmute <player>"),
            spec("spec", Role::Referee, "/spec <player>"),
            spec("play", Role::Referee, "/play <player>"),
            spec("free", Role::Referee, "/free <player>"),
            spec("team", Role::Referee, "/team <player> <blue|red>"),
            spec("restart", Role::Referee, "/restart"),
            spec("addtime", Role::Referee, "/addtime [minutes]"),
            spec("next", Role::Admin, "/next"),
            spec("endround", Role::Referee, "/endround").alias("er"),
            spec("pause", Role::Referee, "/pause"),
            spec("resume", Role::Referee, "/resume").alias("unpause"),
            spec("endwarmup", Role::Referee, "/endwarmup").alias("endwu"),
            spec(
                "points",
                Role::Referee,
                "/points <player> <round|map|match> <value>",
            ),
            spec(
                "teampoints",
                Role::Referee,
                "/teampoints <blue|red> <round|map|match> <value>",
            ),
            spec(
                "guest",
                Role::Admin,
                "/guest <add|remove> <login> | /guest clear",
            ),
            spec("password", Role::Admin, "/password <password|off>"),
            spec("bot", Role::Admin, "/bot [off]"),
            spec("say", Role::Referee, "/say <text>"),
        ]
    }

    fn translations(&self) -> Vec<(Lang, &'static str)> {
        vec![
            (Lang::En, include_str!("../locales/en.toml")),
            (Lang::Fr, include_str!("../locales/fr.toml")),
        ]
    }

    fn run(self: Box<Self>, mut ctx: PluginContext) -> PluginFuture {
        Box::pin(async move {
            let mut extra: Option<ExtraTime> = None;
            while let Some(event) = ctx.next_event().await {
                match event {
                    Event::Command { invocation } if invocation.name == "addtime" => {
                        add_time(&ctx, &invocation, &mut extra);
                    }
                    Event::Command { invocation } => handle(&ctx, &invocation),
                    Event::MapEnd { .. } => {
                        if let Some(extra) = extra.take() {
                            restore_time_limit(&ctx, extra.base);
                        }
                    }
                    // The new mode came with its own settings.
                    Event::ModeChanged { .. } => extra = None,
                    _ => {}
                }
            }
            Ok(())
        })
    }
}

/// Resolves a target player (connected), answering the invoker on failure.
fn target(ctx: &PluginContext, inv: &Invocation, index: usize) -> Option<(String, String)> {
    let Some(query) = inv.arg(index) else {
        usage(ctx, inv);
        return None;
    };
    match ctx.state.read(|s| {
        s.find_player(query)
            .map(|p| (p.login.clone(), p.nickname.clone()))
    }) {
        Ok(found) => Some(found),
        Err(FindPlayerError::NotFound(_)) => {
            ctx.reply(inv, Text::tr("admin.player_not_found").arg("query", query));
            None
        }
        Err(FindPlayerError::Ambiguous(logins)) => {
            ctx.reply(
                inv,
                Text::tr("admin.player_ambiguous").arg("logins", logins.join(", ")),
            );
            None
        }
    }
}

fn usage(ctx: &PluginContext, inv: &Invocation) {
    let usage = ctx.registry.resolve(&inv.name, Role::SuperAdmin);
    let usage = match usage {
        pitwall_core::chat::Resolution::Allowed(spec) => spec.usage,
        _ => format!("/{}", inv.name),
    };
    ctx.reply(inv, Text::tr("core.usage").arg("usage", usage));
}

/// Name of the invoker for public messages.
fn invoker_name(ctx: &PluginContext, inv: &Invocation) -> String {
    match inv.login() {
        Some(login) => ctx
            .state
            .player(login)
            .map_or_else(|| login.to_owned(), |p| p.nickname),
        None => "Admin".to_owned(),
    }
}

/// Runs a command for the invoker; on success optionally announces `success`, else reports the error.
fn run(ctx: &PluginContext, inv: &Invocation, command: Command, success: Option<(bool, Text)>) {
    let commands = ctx.commands_for(inv);
    let replier = ctx.replier(inv);
    let public = ctx.commands.clone();
    tokio::spawn(async move {
        match commands.call(command).await {
            Ok(_) => match success {
                Some((true, text)) => public.chat_all(text),
                Some((false, text)) => replier.say(text),
                None => replier.say(Text::tr("core.done")),
            },
            Err(err) => replier.say(Text::tr("core.error").arg("message", err)),
        }
    });
}

/// Time added to the current Time Attack map, given back when the map ends.
#[derive(Debug, Clone, Copy)]
struct ExtraTime {
    /// `S_TimeLimit` before the first `/addtime` on this map, in seconds.
    base: i32,
    added: i32,
}

fn time_limit(seconds: i32) -> ScriptSettings {
    ScriptSettings::from([("S_TimeLimit".to_owned(), ScriptSetting::Int(seconds))])
}

/// `/addtime [minutes]`: extends the running Time Attack map (5 minutes by default). The mode
/// script picks up a new `S_TimeLimit` during the map, counted from the map start.
fn add_time(ctx: &PluginContext, inv: &Invocation, extra: &mut Option<ExtraTime>) {
    let minutes = inv.arg(0).unwrap_or("5");
    let seconds = minutes
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|m| m.is_finite() && *m > 0.0 && *m <= 600.0)
        .map(|m| (m * 60.0).round() as i32);
    let Some(seconds) = seconds.filter(|s| *s > 0) else {
        return usage(ctx, inv);
    };
    let (time_attack, current) = ctx.state.read(|s| {
        let current = match s.script_settings.get("S_TimeLimit") {
            Some(ScriptSetting::Int(limit)) => *limit,
            _ => 0,
        };
        (s.time_attack(), current)
    });
    if !time_attack {
        ctx.reply(inv, Text::tr("admin.addtime_not_ta"));
        return;
    }
    let state = extra.get_or_insert(ExtraTime {
        base: current,
        added: 0,
    });
    if state.base <= 0 {
        *extra = None;
        ctx.reply(inv, Text::tr("admin.addtime_no_limit"));
        return;
    }
    state.added += seconds;
    let limit = state.base + state.added;
    tracing::info!(limit, added = state.added, "time limit extended");
    let text = Text::tr("admin.time_added")
        .arg("admin", invoker_name(ctx, inv))
        .arg("minutes", minutes);
    run(
        ctx,
        inv,
        Command::SetModeSettings {
            settings: time_limit(limit),
        },
        Some((true, text)),
    );
}

/// Gives the next map the time limit it had before `/addtime`.
fn restore_time_limit(ctx: &PluginContext, base: i32) {
    let commands = ctx.commands.clone();
    tokio::spawn(async move {
        let settings = time_limit(base);
        match commands.call(Command::SetModeSettings { settings }).await {
            Ok(_) => tracing::info!(limit = base, "time limit restored"),
            Err(err) => tracing::warn!(%err, "cannot restore the time limit"),
        }
    });
}

fn points_kind(kind: &str, value: i32) -> Option<(Option<i32>, Option<i32>, Option<i32>)> {
    Some(match kind {
        "round" => (Some(value), None, None),
        "map" => (None, Some(value), None),
        "match" => (None, None, Some(value)),
        _ => return None,
    })
}

fn team_id(raw: &str) -> Option<i32> {
    match raw.to_ascii_lowercase().as_str() {
        "blue" | "bleu" | "0" => Some(0),
        "red" | "rouge" | "1" => Some(1),
        _ => None,
    }
}

fn handle(ctx: &PluginContext, inv: &Invocation) {
    let name = inv.name.as_str();
    match name {
        "kick" | "ban" | "blacklist" => {
            let Some((login, nick)) = target(ctx, inv, 0) else {
                return;
            };
            let reason = inv.rest(1);
            let announce_key = if name == "kick" {
                "admin.kicked"
            } else {
                "admin.banned"
            };
            let text = Text::tr(announce_key)
                .arg("admin", invoker_name(ctx, inv))
                .arg("player", nick)
                .arg("reason", &reason);
            let command = match name {
                "kick" => Command::Kick { login, reason },
                _ => Command::Ban {
                    login,
                    reason,
                    blacklist: name == "blacklist",
                },
            };
            run(ctx, inv, command, Some((true, text)));
        }
        "unban" => {
            let Some(login) = inv.arg(0) else {
                return usage(ctx, inv);
            };
            run(
                ctx,
                inv,
                Command::Unban {
                    login: login.to_owned(),
                },
                None,
            );
        }
        "mute" | "unmute" => {
            let Some((login, nick)) = target(ctx, inv, 0) else {
                return;
            };
            let (command, key) = if name == "mute" {
                (Command::Mute { login }, "admin.muted")
            } else {
                (Command::Unmute { login }, "admin.unmuted")
            };
            run(
                ctx,
                inv,
                command,
                Some((true, Text::tr(key).arg("player", nick))),
            );
        }
        "spec" | "play" | "free" => {
            let Some((login, _)) = target(ctx, inv, 0) else {
                return;
            };
            let mode = match name {
                "spec" => SpectatorModeDto::Spectator,
                "play" => SpectatorModeDto::Player,
                _ => SpectatorModeDto::UserSelectable,
            };
            run(ctx, inv, Command::ForceSpectator { login, mode }, None);
        }
        "team" => {
            let Some((login, _)) = target(ctx, inv, 0) else {
                return;
            };
            let Some(team) = inv.arg(1).and_then(team_id) else {
                return usage(ctx, inv);
            };
            run(ctx, inv, Command::ForceTeam { login, team }, None);
        }
        "restart" => run(
            ctx,
            inv,
            Command::RestartMap,
            Some((true, Text::tr("admin.restarting"))),
        ),
        "next" => run(
            ctx,
            inv,
            Command::NextMap,
            Some((true, Text::tr("admin.skipping"))),
        ),
        "endround" => run(
            ctx,
            inv,
            Command::ForceEndRound,
            Some((true, Text::tr("admin.round_ended"))),
        ),
        "pause" => run(
            ctx,
            inv,
            Command::Pause { active: true },
            Some((true, Text::tr("admin.paused"))),
        ),
        "resume" => run(
            ctx,
            inv,
            Command::Pause { active: false },
            Some((true, Text::tr("admin.resumed"))),
        ),
        "endwarmup" => run(
            ctx,
            inv,
            Command::StopWarmUp,
            Some((true, Text::tr("admin.warmup_stopped"))),
        ),
        "points" => {
            let Some((login, nick)) = target(ctx, inv, 0) else {
                return;
            };
            let parsed = inv
                .arg(2)
                .and_then(|v| v.parse().ok())
                .and_then(|v| points_kind(inv.arg(1).unwrap_or_default(), v));
            let Some((round, map, r#match)) = parsed else {
                return usage(ctx, inv);
            };
            run(
                ctx,
                inv,
                Command::SetPlayerPoints {
                    login,
                    round,
                    map,
                    r#match,
                },
                Some((false, Text::tr("admin.points_set").arg("target", nick))),
            );
        }
        "teampoints" => {
            let team = inv.arg(0).and_then(team_id);
            let parsed = inv
                .arg(2)
                .and_then(|v| v.parse().ok())
                .and_then(|v| points_kind(inv.arg(1).unwrap_or_default(), v));
            let (Some(team), Some((round, map, r#match))) = (team, parsed) else {
                return usage(ctx, inv);
            };
            run(
                ctx,
                inv,
                Command::SetTeamPoints {
                    team,
                    round,
                    map,
                    r#match,
                },
                Some((
                    false,
                    Text::tr("admin.points_set").arg("target", inv.arg(0).unwrap_or_default()),
                )),
            );
        }
        "guest" => match (inv.arg(0), inv.arg(1)) {
            (Some("add"), Some(login)) => run(
                ctx,
                inv,
                Command::AddGuest {
                    login: login.to_owned(),
                },
                None,
            ),
            (Some("remove"), Some(login)) => run(
                ctx,
                inv,
                Command::RemoveGuest {
                    login: login.to_owned(),
                },
                None,
            ),
            (Some("clear"), _) => run(ctx, inv, Command::CleanGuests, None),
            _ => usage(ctx, inv),
        },
        "password" => {
            let Some(raw) = inv.arg(0) else {
                return usage(ctx, inv);
            };
            let password = if raw == "off" {
                String::new()
            } else {
                raw.to_owned()
            };
            run(ctx, inv, Command::SetPassword { password }, None);
        }
        "bot" => {
            if inv.arg(0) == Some("off") {
                run(ctx, inv, Command::DisconnectBots, None);
                return;
            }
            let commands = ctx.commands_for(inv);
            let replier = ctx.replier(inv);
            tokio::spawn(async move {
                match commands.call(Command::ConnectBot).await {
                    Ok(login) => replier.say(
                        Text::tr("admin.bot_connected")
                            .arg("login", login.as_str().unwrap_or_default()),
                    ),
                    Err(err) => replier.say(Text::tr("core.error").arg("message", err)),
                }
            });
        }
        "say" => {
            if inv.raw_args.is_empty() {
                return usage(ctx, inv);
            }
            ctx.commands
                .chat_all(Text::tr("admin.announce").arg("text", &inv.raw_args));
        }
        _ => {}
    }
}
