//! Format switching from chat/console: `/mode <format>`, `/formats`, `/set S_Setting value`.

use std::sync::{Arc, RwLock};

use pitwall_core::gbx::types::{ScriptSetting, ScriptSettings};
use pitwall_core::{
    Command, CommandSpec, Event, Invocation, Lang, Plugin, PluginContext, PluginFuture, Recipients,
    Role, Text,
};
use pitwall_formats::format::parse_setting;
use pitwall_formats::{FormatRegistry, mode};

/// Shared format registry (the cup plugin and the web API use the same one).
pub type SharedFormats = Arc<RwLock<FormatRegistry>>;

/// The formats plugin.
#[derive(Debug, Clone)]
pub struct FormatsPlugin {
    formats: SharedFormats,
}

impl FormatsPlugin {
    pub fn new(formats: SharedFormats) -> FormatsPlugin {
        FormatsPlugin { formats }
    }
}

impl Plugin for FormatsPlugin {
    fn name(&self) -> &'static str {
        "formats"
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec::new("formats", "mode", Role::Admin)
                .alias("format")
                .usage("/mode <format>"),
            CommandSpec::new("formats", "formats", Role::Player),
            CommandSpec::new("formats", "set", Role::Admin).usage("/set <S_Setting> <value>"),
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
            while let Some(event) = ctx.next_event().await {
                match event {
                    Event::Command { invocation } => self.on_command(&ctx, invocation),
                    Event::ModeChanged { script } => {
                        let name = mode::KNOWN_MODES
                            .iter()
                            .find(|m| {
                                pitwall_core::matchsettings::normalize_script_name(m.script)
                                    == pitwall_core::matchsettings::normalize_script_name(&script)
                            })
                            .map_or_else(|| script.clone(), |m| m.id.to_owned());
                        let display = self
                            .formats
                            .read()
                            .ok()
                            .and_then(|f| f.get(&name).map(|d| d.name.clone()));
                        // Localized names are rendered per language through raw keys.
                        for lang in Lang::ALL {
                            let logins: Vec<String> = ctx.state.read(|s| {
                                s.players
                                    .values()
                                    .filter(|p| p.lang == lang)
                                    .map(|p| p.login.clone())
                                    .collect()
                            });
                            if logins.is_empty() {
                                continue;
                            }
                            let shown = display
                                .as_ref()
                                .map(|d| d.get(lang).to_owned())
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| name.clone());
                            ctx.chat(
                                Recipients::Logins(logins),
                                Text::tr("formats.switched").arg("name", shown),
                            );
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        })
    }
}

impl FormatsPlugin {
    fn on_command(&self, ctx: &PluginContext, inv: Invocation) {
        match inv.name.as_str() {
            "formats" => {
                ctx.reply(&inv, Text::tr("formats.list_header"));
                let Ok(formats) = self.formats.read() else {
                    return;
                };
                for def in formats.all() {
                    ctx.reply(
                        &inv,
                        Text::tr("formats.list_entry")
                            .arg("id", &def.id)
                            .arg("name", def.display_name(inv.lang))
                            .arg("description", def.description.get(inv.lang)),
                    );
                }
            }
            "mode" => {
                let Some(id) = inv.arg(0) else {
                    ctx.reply(&inv, Text::tr("core.usage").arg("usage", "/mode <format>"));
                    return;
                };
                let switch = self.formats.read().ok().and_then(|f| {
                    let def = f.get(id)?;
                    Some((
                        def.display_name(inv.lang).to_owned(),
                        def.to_mode_switch(&ScriptSettings::new(), None),
                    ))
                });
                match switch {
                    None => ctx.reply(&inv, Text::tr("formats.unknown").arg("name", id)),
                    Some((_, Err(err))) => {
                        ctx.reply(&inv, Text::tr("formats.failed").arg("message", err))
                    }
                    Some((name, Ok(switch))) => {
                        ctx.commands
                            .chat_all(Text::tr("formats.switching").arg("name", &name));
                        let commands = ctx.commands_for(&inv);
                        let reply_ctx = ctx.replier(&inv);
                        tokio::spawn(async move {
                            if let Err(err) = commands.call(Command::SwitchMode(switch)).await {
                                reply_ctx.say(Text::tr("formats.failed").arg("message", err));
                            }
                        });
                    }
                }
            }
            "set" => {
                let (Some(name), Some(raw)) = (inv.arg(0), inv.arg(1)) else {
                    ctx.reply(
                        &inv,
                        Text::tr("core.usage").arg("usage", "/set <S_Setting> <value>"),
                    );
                    return;
                };
                let value = parse_setting(&inv.rest(1));
                let shown = match &value {
                    ScriptSetting::Text(t) => t.clone(),
                    other => format!("{other:?}"),
                };
                let mut settings = ScriptSettings::new();
                settings.insert(name.to_owned(), value);
                let commands = ctx.commands_for(&inv);
                let reply_ctx = ctx.replier(&inv);
                let (name, _raw) = (name.to_owned(), raw.to_owned());
                tokio::spawn(async move {
                    match commands.call(Command::SetModeSettings { settings }).await {
                        Ok(_) => reply_ctx.say(
                            Text::tr("formats.setting_set")
                                .arg("name", name)
                                .arg("value", shown),
                        ),
                        Err(err) => reply_ctx.say(Text::tr("core.error").arg("message", err)),
                    }
                });
            }
            _ => {}
        }
    }
}
