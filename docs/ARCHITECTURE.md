# Architecture

## Layers

```
            ┌───────────── crates/app (pitwall binary) ─────────────┐
            │ config (figment) · console (stdin) · signals · wiring │
            └──────┬───────────────────────┬────────────────────────┘
                   │                       │
        plugins/* (internal)          crates/api (axum)
   logger formats admin cup ui         dashboard · JSON · WS
   storage maps discord                     │
                   │ events ▲  ▼ commands   │ Handles (same API)
            ┌──────┴─────────────────────────┴───────┐
            │ crates/core                            │
            │ EventBus (broadcast) · CommandHandle   │
            │ StateHandle · RoleBook · Translator    │
            │ Controller runtime (connect, reconnect,│
            │ state tracking, command executor)      │
            └──────────────────┬─────────────────────┘
                               │ GbxClient (only the runtime holds it)
            ┌──────────────────┴─────────────────────┐
            │ crates/gbx: framing, XML-RPC, callbacks│
            └──────────────────┬─────────────────────┘
                               │ TCP 5000 (GbxRemote 2)
                     Trackmania dedicated server
```

Pure libraries (no I/O): `crates/formats` (formats, cup engine), `crates/ui` (rendering).
I/O libraries: `crates/storage` (SQLite), `crates/integrations` (HTTP).

## Event flow

1. The `gbx` reader task decodes frames; handles `>= 0x8000_0000` complete pending calls, the
   others are callbacks, typed into `Callback` / `ScriptCallback`.
2. The runtime updates `ServerState`, publishes the raw `Event::Callback` and derived events
   (`PlayerJoined`, `Finish`, `MapStart`, `Scores`, …). Chat lines starting with `/` are parsed,
   role-checked through the `CommandRegistry` and published as `Event::Command`.
3. Plugins react and send `Command`s through their `CommandHandle`. A single executor task runs
   them in order and publishes `Event::Audit` for admin actions.

## Hot mode switch

`Command::SwitchMode(ModeSwitch)`:

1. builds a MatchSettings document (script, `S_*` settings, playlist: the current one, or the
   format's maps);
2. `WriteFile("MatchSettings/PitWall/<label>.txt")`, `LoadMatchSettings`, then `RestartMap` (or
   `ChooseNextMap` + `NextMap` for a new playlist): one map load, players stay connected;
3. on the next `Maniaplanet.StartMap_Start`, the runtime checks `GetScriptName`, re-applies any
   setting that did not stick, sets the points repartition and publishes `Event::ModeChanged`.

## Cup engine

`CupEngine::handle(CupInput, now) -> Vec<CupAction>` is pure and fully tested. The cup plugin
feeds it (admin commands, `ModeChanged`, `Scores` with `section = "EndMatch"`, joins/leaves, ready
clicks, a 1 s tick) and executes the actions (chat, `SwitchMode`, `ForceSpectator`,
`ForcePlayerTeam`, pause). The state is saved to `data/cups/active.json` after every change.

## UI updates

Each widget has a view page (sent once per player) and a data page (JSON in a `data-json`
attribute). The data page's script stores the JSON in `declare Text … for LocalUser` and bumps a
serial number; the view's loop parses it with `fromjson` and animates the changes with `AnimMgr`.
The UI plugin batches changes every 250 ms, skips identical payloads and only targets players
displaying the widget.

## Extending

- New format: a TOML file in `config/formats`.
- New cup: a TOML file in `config/cups` (+ optional Lua rules).
- New internal plugin: implement `pitwall_core::Plugin` (commands, translations, `run` loop) and
  add it in `crates/app/src/main.rs`. External plugins (WASM) are planned on top of this API.
