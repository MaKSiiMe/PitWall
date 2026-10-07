//! Built-in commands: `/help`, `/lang`, `/role`, `/players`.

use crate::chat::{CommandSpec, Invocation};
use crate::event::Event;
use crate::i18n::{Lang, Text};
use crate::plugin::{Plugin, PluginContext, PluginFuture};
use crate::roles::{Permission, Role};

/// Always loaded by the controller.
#[derive(Debug, Clone, Copy)]
pub struct CorePlugin;

impl Plugin for CorePlugin {
    fn name(&self) -> &'static str {
        "core"
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec::new("core", "help", Role::Player).alias("h"),
            CommandSpec::new("core", "lang", Role::Player)
                .alias("language")
                .usage("/lang <fr|en>"),
            CommandSpec::new("core", "role", Role::Referee)
                .usage("/role <login> [player|caster|referee|admin|superadmin]"),
            CommandSpec::new("core", "players", Role::Caster),
        ]
    }

    fn run(self: Box<Self>, mut ctx: PluginContext) -> PluginFuture {
        Box::pin(async move {
            while let Some(event) = ctx.next_event().await {
                if let Event::Command { invocation } = event {
                    handle(&ctx, &invocation);
                }
            }
            Ok(())
        })
    }
}

fn handle(ctx: &PluginContext, inv: &Invocation) {
    match inv.name.as_str() {
        "help" => {
            let mut lines = vec![Text::tr("core.help_header")];
            for spec in ctx.registry.available_to(inv.role) {
                let description = ctx.i18n.render(inv.lang, &Text::tr(&spec.description_key));
                let command = if spec.usage.starts_with(&format!("/{}", spec.name)) {
                    spec.usage.clone()
                } else {
                    format!("/{}", spec.name)
                };
                lines.push(
                    Text::tr("core.help_entry")
                        .arg("command", command)
                        .arg("description", description),
                );
            }
            for line in lines {
                ctx.reply(inv, line);
            }
        }
        "lang" => {
            let Some(lang) = inv.arg(0).and_then(Lang::from_code) else {
                ctx.reply(inv, Text::tr("core.usage").arg("usage", "/lang <fr|en>"));
                return;
            };
            if let Some(login) = inv.login() {
                let player = ctx.state.write(|s| {
                    let p = s.players.get_mut(login)?;
                    p.lang = lang;
                    Some(p.clone())
                });
                if let Some(player) = player {
                    ctx.bus.publish(Event::PlayerUpdated { player });
                }
            }
            let mut answer = inv.clone();
            answer.lang = lang;
            ctx.reply(&answer, Text::tr("core.lang_set"));
        }
        "role" => {
            let Some(login) = inv.arg(0) else {
                ctx.reply(
                    inv,
                    Text::tr("core.usage").arg("usage", "/role <login> [role]"),
                );
                return;
            };
            match inv.arg(1) {
                None => {
                    let role = ctx.roles.get(login);
                    let role_name = ctx
                        .i18n
                        .t(inv.lang, &format!("core.role.{}", role.as_str()));
                    ctx.reply(
                        inv,
                        Text::tr("core.role_of")
                            .arg("login", login)
                            .arg("role", role_name),
                    );
                }
                Some(role) => {
                    if !inv.role.can(Permission::ManageRoles) {
                        ctx.reply(inv, Text::tr("core.permission_denied").arg("name", "role"));
                        return;
                    }
                    let Some(role) = Role::parse(role) else {
                        ctx.reply(
                            inv,
                            Text::tr("core.usage").arg(
                                "usage",
                                "/role <login> [player|caster|referee|admin|superadmin]",
                            ),
                        );
                        return;
                    };
                    ctx.roles.set(login, role);
                    let player = ctx.state.write(|s| {
                        let p = s.players.get_mut(login)?;
                        p.role = role;
                        Some(p.clone())
                    });
                    if let Some(player) = player {
                        ctx.bus.publish(Event::PlayerUpdated { player });
                    }
                    let role_name = ctx
                        .i18n
                        .t(inv.lang, &format!("core.role.{}", role.as_str()));
                    ctx.reply(
                        inv,
                        Text::tr("core.role_set")
                            .arg("login", login)
                            .arg("role", role_name),
                    );
                }
            }
        }
        "players" => {
            let players = ctx
                .state
                .read(|s| s.players.values().cloned().collect::<Vec<_>>());
            for p in players {
                let flag = if p.spectator { " (spec)" } else { "" };
                ctx.reply(
                    inv,
                    Text::raw(format!(
                        "$fff{}$z$s $888{} {}{}",
                        p.nickname, p.login, p.lang, flag
                    )),
                );
            }
        }
        _ => {}
    }
}
