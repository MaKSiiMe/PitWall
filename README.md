# PitWall

A Trackmania 2020 dedicated server controller written in Rust, built for **cups, LAN events and
team matches**.

- **Hot format switching**: go from Time Attack to Cup, Rounds, Teams, Knockout… with one command,
  without disconnecting anyone. The mode script, its settings and the playlist change in a
  single map load.
- **Fluid in-game UI**: live ranking whose rows slide to their new rank, cup banner with a ready
  button, records, admin panel. Each player hides any widget instantly (handled on the client),
  and data updates are small and sent only when something changed.
- **Cup engine**: cups are chains of phases described in TOML (qualifiers → knockout, groups →
  final, brackets…). Optional sandboxed Lua scripts handle custom rules. The engine also covers
  ready checks, disconnection policies, crash recovery and per-cup registration modes.

> Status: the code is complete and tested against an in-process fake server; it has not yet run
> against a real dedicated server. See [Known limitations](#known-limitations).

French getting-started guide: [docs/GUIDE.fr.md](docs/GUIDE.fr.md).

## Features

| Area | What you get |
| --- | --- |
| Protocol | GbxRemote 2 client: typed methods and callbacks (legacy + TM2020 script callbacks), multicall, timeouts, auto-reconnect |
| Formats | Built-in TA, Rounds, Cup, Teams, Knockout, Laps, TMWT, Royal; custom formats in `config/formats/*.toml`; `/mode <format>`, `/set S_x value`; `controller.formats` picks the ones offered (TA and Rounds by default) |
| Cups | Phases, qualification (top N / all / Lua), single elimination brackets (teams), start policy (manual / ready check / auto), disconnect policy (keep / pause / eliminate), registrations (list / open / in game / web, optional approval), state saved for crash recovery; off by default for now (`controller.cups = true`) |
| In-game UI | Live ranking, cup banner + ready button, records, collapsible role-based admin panel, instant show/hide menu, FR/EN |
| Moderation | kick, ban, blacklist, mute, force spectator/player, teams, pause, end round, points corrections, guest list, password, test bots |
| Roles | Player < Caster < Referee < Admin < SuperAdmin, fixed permissions |
| Maps | Trackmania Exchange (id, URL, search), Nadeo map uid, club campaigns and rooms, local files, uploaded with `WriteFile` (no shared volume); in-game map manager; playlist and mode saved in the server's startup MatchSettings |
| Storage | SQLite: players, Elo rating, runs with checkpoints, cup history and phase results, audit log, registrations, teams |
| Web | Dashboard (htmx, works offline), live cup page, web registration, history with CSV/JSON export, JSON API, WebSocket events (overlay-ready) |
| Integrations | Discord webhooks (registrations, start, results, podium) |

## Quick start

Prerequisites: Rust (stable, edition 2024), Docker (for the dedicated server), and a dedicated
server account from <https://players.trackmania.com/server/dedicated>.

```sh
cp .env.example .env              # fill in the server account and XML-RPC passwords
docker compose up -d trackmania   # dedicated server, XML-RPC on 127.0.0.1:5000
cp pitwall.example.toml pitwall.toml
cargo run --release               # reads .env, connects, console on stdin (type `help`)
```

Without a Trackmania server, try it against the fake server:

```sh
cargo run -p pitwall-gbx --features mock --example mock_server   # terminal 1
PITWALL_SERVER__PASSWORD=dev cargo run                            # terminal 2, then: mode cup
```

## Configuration

`pitwall.toml` (see [pitwall.example.toml](pitwall.example.toml)), overridden by environment
variables `PITWALL_<SECTION>__<KEY>`. **Secrets only go in the environment / `.env`**:

| Variable | Purpose |
| --- | --- |
| `PITWALL_SERVER__PASSWORD` | XML-RPC SuperAdmin password (required) |
| `PITWALL_API__ADMIN_TOKEN` | Web admin token (empty = admin pages disabled) |
| `PITWALL_NADEO__LOGIN` / `PITWALL_NADEO__PASSWORD` | Dedicated server account, for `/map uid` and `/map club` |
| `PITWALL_DISCORD__WEBHOOK_URL` | Discord channel webhook |

## Commands

Typed in game (`/help`), in the console (no slash needed) or in the web console.

| Command | Role | |
| --- | --- | --- |
| `/help`, `/lang <fr\|en>`, `/ui <widget\|all> [on\|off]` | Player | |
| `/join`, `/leave`, `/ready` | Player | cup registration and ready check |
| `/formats` | Player | list formats |
| `/players` | Caster | |
| `/kick`, `/mute`, `/unmute`, `/spec`, `/play`, `/free`, `/team <p> <blue\|red>` | Referee | |
| `/pause`, `/resume`, `/endround`, `/restart`, `/endwarmup`, `/say` | Referee | |
| `/addtime [minutes]` | Referee | Time Attack: adds time to the running map (5 min by default, panel button `+5 min`); the next map gets its normal limit back |
| `/points <p> <round\|map\|match> <n>`, `/teampoints <blue\|red> <…> <n>` | Referee | score corrections |
| `/cup status\|go\|pause\|resume\|dq\|reinstate` | Referee | |
| `/role <login> [role]` | Referee (set: SuperAdmin) | |
| `/mode <format>`, `/set <S_x> <value>` | Admin | hot format switch, live setting |
| `/cup list\|load\|unload\|open\|start\|advance\|winner\|retry\|add\|remove\|approve\|abort` | Admin | |
| `/maps [page]` | Admin | in-game map manager: order, next, remove, clear, TMX add and search |
| `/map list\|move <from> <to>\|next <n>\|play <n>\|remove [n]\|clear` | Admin | playlist (numbers from `/map list`) |
| `/map tmx <id\|url> [next]`, `/map search`, `/map uid`, `/map club <club> <campaign>`, `/map room <club> <room>`, `/map local` | Admin | add maps |
| `/ban`, `/blacklist`, `/unban`, `/next`, `/guest`, `/password`, `/bot [off]` | Admin | |

## Formats and cups

A format (`config/formats/*.toml`):

```toml
id = "mini-cup"
mode = "cup"                      # known mode id/alias, or a script path
name = { en = "Mini cup", fr = "Mini coupe" }
points_repartition = [10, 6, 4, 3, 2, 1]
[settings]
S_PointsLimit = 50
S_RoundsPerMap = 4
```

A cup (`config/cups/*.toml`, see the two examples there):

```toml
id = "mini-cup"
registration = ["list", "ingame"]  # list | open | ingame | web | discord
start = "ready_check"              # manual | ready_check | auto
disconnect = { policy = "pause", grace_secs = 120 }

[[phase]]
id = "quali"
format = "timeattack"
settings = { S_TimeLimit = 600 }
qualify = { top = 4 }              # "all" | { top = n } | { lua = "function" }

[[phase]]
id = "final"
format = "mini-cup"
```

Lua rules (`rules = "file.lua"`) may define `qualify` functions, `seed(phase, ids)` and
`on_phase_end(phase, standings, cup_points)`. They run in a sandbox (string/table/math only,
memory limit, instruction budget). See [config/cups/team-league.lua](config/cups/team-league.lua).

## Architecture

```
crates/gbx           GbxRemote 2 protocol only (framing, XML-RPC, typed calls/callbacks, mock server)
crates/core          event bus, command API, server state, roles, i18n, runtime (reconnect, mode switch)
crates/formats       formats (TOML) and the cup engine (pure state machine, brackets, Lua rules)
crates/ui            Manialink templates (askama) + ManiaScript, theme and layout
crates/storage       SQLite persistence (sqlx, embedded migrations)
crates/integrations  TMX, Nadeo API, Discord webhooks, exports
crates/api           web dashboard, JSON API, WebSocket (axum + htmx)
crates/app           the `pitwall` binary: config, console, wiring
plugins/*            internal plugins: logger, formats, admin, cup, ui, storage, maps, discord
```

Plugins receive events and send commands; they never touch the socket. See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Development

```sh
cargo test --workspace --all-features      # unit + integration tests (mock server)
cargo clippy --workspace --all-targets --all-features
cargo run -p pitwall-ui --example render -- ranking fr    # print a rendered widget
git config core.hooksPath .githooks     # run the CI checks before every push
```

## Deployment

A push to `main` runs the CI. Once it passes, the CI publishes `ghcr.io/maksiime/pitwall:sha-<commit>`
and writes [deploy/portainer/stack.yml](deploy/portainer/stack.yml), pinned to that image, on the
`deploy` branch. A Portainer stack linked to that branch (GitOps polling) then redeploys PitWall.
The Trackmania server image is pinned too: a daily workflow opens a PR when a newer one is
published. Commits with `[skip deploy]` in their message are not deployed. Setup:
[docs/GUIDE.fr.md](docs/GUIDE.fr.md), section 7.

## Known limitations

- Never run against a real dedicated server yet: XML-RPC calls follow the official method list,
  but the in-game ManiaScript of the widgets and a few behaviours (see GUIDE.fr.md, "À vérifier")
  need a live test.
- Not implemented yet: Discord bot (webhooks only), start.gg/Challonge sync, OBS overlay pages
  (the WebSocket is ready), WASM/external plugins, several servers per controller.

No license has been chosen yet.
