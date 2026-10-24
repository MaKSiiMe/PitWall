//! Player votes: `/skip` and `/res` call a vote to skip or restart the map, `/yes` and `/no`
//! answer it, `/vote pass|cancel` lets a referee decide. One vote at a time; it ends with the map.
//! Staff allowed to act directly skip the vote: `/skip` runs `/next` for admins, `/res` runs
//! `/restart` for referees (the admin plugin's own commands, with their message and audit).
//!
//! The vote passes as soon as more than `ratio` of the connected players said yes, fails as soon
//! as that cannot happen any more, and otherwise is decided at the deadline by the votes cast.
//! The state is published as the custom event `votes/state` for the UI:
//!
//! ```json
//! { "active": true, "kind": "skip" | "restart", "initiator": "<nickname>",
//!   "yes": ["<login>"], "no": ["<login>"], "deadline_ms": 0 }
//! ```

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use pitwall_core::{
    Command, CommandSpec, Event, Invocation, Lang, Plugin, PluginContext, PluginError,
    PluginFuture, Role, Text, now_ms,
};
use serde::Deserialize;

/// `[plugins.votes]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct VotesConfig {
    enabled: bool,
    /// Seconds to answer.
    duration_s: u32,
    /// Share of yes needed, over the connected players (early end) or the votes cast (deadline).
    ratio: f64,
    /// Seconds before the same player can call another vote.
    cooldown_s: u32,
}

impl Default for VotesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            duration_s: 30,
            ratio: 0.5,
            cooldown_s: 60,
        }
    }
}

/// What a vote decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Skip,
    Restart,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::Skip => "skip",
            Kind::Restart => "restart",
        }
    }

    fn command(self) -> Command {
        match self {
            Kind::Skip => Command::NextMap,
            Kind::Restart => Command::RestartMap,
        }
    }
}

/// A running vote.
#[derive(Debug, Clone)]
struct Vote {
    kind: Kind,
    initiator: String,
    yes: BTreeSet<String>,
    no: BTreeSet<String>,
    deadline_ms: i64,
}

impl Vote {
    /// `Some(true)` passed, `Some(false)` failed, `None` still open. Only the votes of the
    /// `electorate` (connected players) count.
    fn outcome(&self, electorate: &BTreeSet<String>, ratio: f64, now: i64) -> Option<bool> {
        let yes = self.yes.intersection(electorate).count() as f64;
        let no = self.no.intersection(electorate).count() as f64;
        let total = electorate.len() as f64;
        if yes > ratio * total {
            return Some(true);
        }
        // Even if everyone left answered yes.
        if total - no <= ratio * total {
            return Some(false);
        }
        if now >= self.deadline_ms {
            return Some(yes + no > 0.0 && yes > ratio * (yes + no));
        }
        None
    }
}

/// The votes plugin.
#[derive(Debug, Default)]
pub struct VotesPlugin;

impl Plugin for VotesPlugin {
    fn name(&self) -> &'static str {
        "votes"
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec::new("votes", "skip", Role::Player).usage("/skip"),
            CommandSpec::new("votes", "res", Role::Player)
                .alias("replay")
                .usage("/res"),
            CommandSpec::new("votes", "yes", Role::Player)
                .alias("y")
                .usage("/yes"),
            CommandSpec::new("votes", "no", Role::Player)
                .alias("n")
                .usage("/no"),
            CommandSpec::new("votes", "vote", Role::Referee).usage("/vote <pass|cancel>"),
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
            let config: VotesConfig = ctx.config()?;
            let mut votes = Votes {
                config,
                vote: None,
                last_call: HashMap::new(),
            };
            let mut tick = tokio::time::interval(Duration::from_millis(500));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    event = ctx.next_event() => match event {
                        Some(event) => votes.on_event(&ctx, event),
                        None => break,
                    },
                    _ = tick.tick() => votes.check(&ctx),
                }
            }
            Ok::<(), PluginError>(())
        })
    }
}

struct Votes {
    config: VotesConfig,
    vote: Option<Vote>,
    /// Login → when they last called a vote (Unix ms).
    last_call: HashMap<String, i64>,
}

/// Runs the admin command `line` for the invoker, with the same role checks.
fn forward(ctx: &PluginContext, inv: &Invocation, line: &str) {
    ctx.invoke(
        inv.actor.clone(),
        inv.role,
        inv.lang,
        line,
        inv.reply.clone(),
    );
}

fn electorate(ctx: &PluginContext) -> BTreeSet<String> {
    ctx.state.read(|s| s.players.keys().cloned().collect())
}

impl Votes {
    fn on_event(&mut self, ctx: &PluginContext, event: Event) {
        match event {
            Event::Command { invocation } => match invocation.name.as_str() {
                "skip" if invocation.role >= Role::Admin => forward(ctx, &invocation, "next"),
                "res" if invocation.role >= Role::Referee => forward(ctx, &invocation, "restart"),
                "skip" => self.call(ctx, &invocation, Kind::Skip),
                "res" => self.call(ctx, &invocation, Kind::Restart),
                "yes" => self.answer(ctx, &invocation, true),
                "no" => self.answer(ctx, &invocation, false),
                "vote" => self.decide(ctx, &invocation),
                _ => {}
            },
            // The map it was about is over.
            Event::MapEnd { .. } | Event::MapStart { .. } => {
                if self.vote.take().is_some() {
                    self.publish(ctx);
                }
            }
            Event::PlayerLeft { .. } => self.check(ctx),
            // The UI may have restarted: give it the current state.
            Event::Connected { .. } => self.publish(ctx),
            _ => {}
        }
    }

    fn call(&mut self, ctx: &PluginContext, inv: &Invocation, kind: Kind) {
        let Some(login) = inv.login() else {
            return;
        };
        if !self.config.enabled {
            return ctx.reply(inv, Text::tr("votes.disabled"));
        }
        if self.vote.is_some() {
            return ctx.reply(inv, Text::tr("votes.running"));
        }
        let now = now_ms();
        let cooldown = i64::from(self.config.cooldown_s) * 1000;
        if let Some(last) = self.last_call.get(login)
            && now - last < cooldown
        {
            let seconds = (cooldown - (now - last) + 999) / 1000;
            return ctx.reply(inv, Text::tr("votes.cooldown").arg("seconds", seconds));
        }
        self.last_call.insert(login.to_owned(), now);
        let nickname = ctx
            .state
            .player(login)
            .map(|p| p.nickname)
            .unwrap_or_else(|| login.to_owned());
        self.vote = Some(Vote {
            kind,
            initiator: nickname.clone(),
            yes: BTreeSet::from([login.to_owned()]),
            no: BTreeSet::new(),
            deadline_ms: now + i64::from(self.config.duration_s) * 1000,
        });
        tracing::info!(login, kind = kind.key(), "vote called");
        ctx.commands.chat_all(
            Text::tr(match kind {
                Kind::Skip => "votes.started_skip",
                Kind::Restart => "votes.started_restart",
            })
            .arg("player", nickname)
            .arg("seconds", self.config.duration_s),
        );
        self.publish(ctx);
        // Alone on the server: decided at once.
        self.check(ctx);
    }

    fn answer(&mut self, ctx: &PluginContext, inv: &Invocation, yes: bool) {
        let Some(login) = inv.login() else {
            return;
        };
        let Some(vote) = self.vote.as_mut() else {
            return ctx.reply(inv, Text::tr("votes.none"));
        };
        if yes {
            vote.no.remove(login);
            vote.yes.insert(login.to_owned());
        } else {
            vote.yes.remove(login);
            vote.no.insert(login.to_owned());
        }
        self.publish(ctx);
        self.check(ctx);
    }

    fn decide(&mut self, ctx: &PluginContext, inv: &Invocation) {
        let Some(vote) = self.vote.take() else {
            return ctx.reply(inv, Text::tr("votes.none"));
        };
        match inv.arg(0) {
            Some("pass") => {
                ctx.commands.chat_all(Text::tr("votes.forced"));
                ctx.commands_for(inv).send(vote.kind.command());
            }
            Some("cancel") => ctx.commands.chat_all(Text::tr("votes.cancelled")),
            _ => {
                self.vote = Some(vote);
                return ctx.reply(
                    inv,
                    Text::tr("core.usage").arg("usage", "/vote <pass|cancel>"),
                );
            }
        }
        self.publish(ctx);
    }

    /// Ends the vote once it is decided.
    fn check(&mut self, ctx: &PluginContext) {
        let Some(vote) = &self.vote else {
            return;
        };
        let electorate = electorate(ctx);
        let Some(passed) = vote.outcome(&electorate, self.config.ratio, now_ms()) else {
            return;
        };
        let (yes, no) = (
            vote.yes.intersection(&electorate).count(),
            vote.no.intersection(&electorate).count(),
        );
        let kind = vote.kind;
        self.vote = None;
        tracing::info!(kind = kind.key(), passed, yes, no, "vote ended");
        let key = if passed {
            "votes.passed"
        } else {
            "votes.failed"
        };
        ctx.commands
            .chat_all(Text::tr(key).arg("yes", yes).arg("no", no));
        if passed {
            ctx.commands.send(kind.command());
        }
        self.publish(ctx);
    }

    fn publish(&self, ctx: &PluginContext) {
        let payload = match &self.vote {
            Some(v) => serde_json::json!({
                "active": true,
                "kind": v.kind.key(),
                "initiator": v.initiator,
                "yes": v.yes,
                "no": v.no,
                "deadline_ms": v.deadline_ms,
            }),
            None => serde_json::json!({ "active": false }),
        };
        ctx.emit("state", payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn players(logins: &[&str]) -> BTreeSet<String> {
        logins.iter().map(|l| (*l).to_owned()).collect()
    }

    fn vote(yes: &[&str], no: &[&str]) -> Vote {
        Vote {
            kind: Kind::Skip,
            initiator: "a".into(),
            yes: players(yes),
            no: players(no),
            deadline_ms: 1_000,
        }
    }

    #[test]
    fn alone_the_caller_decides() {
        assert_eq!(
            vote(&["a"], &[]).outcome(&players(&["a"]), 0.5, 0),
            Some(true)
        );
    }

    #[test]
    fn a_majority_of_players_ends_it_early() {
        let all = players(&["a", "b", "c", "d"]);
        // 2 of 4 is not more than half.
        assert_eq!(vote(&["a", "b"], &[]).outcome(&all, 0.5, 0), None);
        assert_eq!(
            vote(&["a", "b", "c"], &[]).outcome(&all, 0.5, 0),
            Some(true)
        );
        // Two no out of four: yes can no longer pass half.
        assert_eq!(vote(&["a"], &["b", "c"]).outcome(&all, 0.5, 0), Some(false));
    }

    #[test]
    fn the_deadline_counts_the_votes_cast() {
        let all = players(&["a", "b", "c", "d", "e"]);
        assert_eq!(
            vote(&["a", "b"], &["c"]).outcome(&all, 0.5, 1_000),
            Some(true)
        );
        assert_eq!(vote(&["a"], &["c"]).outcome(&all, 0.5, 1_000), Some(false));
    }

    #[test]
    fn players_who_left_no_longer_count() {
        let all = players(&["a", "b", "c"]);
        // "x" voted, then left.
        assert_eq!(vote(&["a", "x"], &["b"]).outcome(&all, 0.5, 0), None);
        assert_eq!(
            vote(&["a", "x"], &["b"]).outcome(&all, 0.5, 1_000),
            Some(false)
        );
    }
}
