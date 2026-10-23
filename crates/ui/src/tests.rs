//! Rendering tests: every page must be well-formed XML with a sane script.

use pitwall_core::{Lang, Role, Translator};
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::model::{RankingData, RankingRow};
use crate::render::{AdminContext, MapEntry, MapsWindow, MenuContext, TmxEntry, ms_string};
use crate::{Renderer, WidgetId, catalogs};

fn translator() -> Translator {
    let t = Translator::with_core_catalogs().unwrap();
    for (lang, catalog) in catalogs() {
        t.add_catalog(lang, catalog).unwrap();
    }
    t
}

/// Parses the document and returns `(root attributes ok, script text)`.
fn check_xml(xml: &str) -> String {
    let mut reader = Reader::from_str(xml);
    let mut depth = 0i32;
    let mut script = String::new();
    let mut in_script = false;
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|e| panic!("invalid XML: {e}\n{xml}"))
        {
            Event::Start(e) => {
                depth += 1;
                in_script = e.local_name().as_ref() == "script";
            }
            Event::End(_) => {
                depth -= 1;
                in_script = false;
            }
            Event::Comment(c) if in_script => script.push_str(&c.into_inner()),
            Event::Eof => break,
            _ => {}
        }
    }
    assert_eq!(depth, 0, "unbalanced document");
    assert!(
        !xml.contains("{{") && !xml.contains("{%"),
        "template syntax left in output"
    );
    script
}

fn check_script(script: &str) {
    let open = script.matches('{').count();
    let close = script.matches('}').count();
    assert_eq!(open, close, "unbalanced braces in ManiaScript:\n{script}");
    assert!(
        !script.contains("&lt;") && !script.contains("&#"),
        "HTML entities inside the script:\n{script}"
    );
    let parens = script.matches('(').count() == script.matches(')').count();
    assert!(parens, "unbalanced parentheses in ManiaScript");
}

#[test]
fn every_view_renders_in_both_languages() {
    let renderer = Renderer::default();
    let i18n = translator();
    for lang in Lang::ALL {
        for widget in WidgetId::ALL {
            let xml = renderer.view(widget, lang, &i18n).unwrap();
            assert!(xml.contains(&format!("id=\"{}\"", widget.view_id())));
            let script = check_xml(&xml);
            check_script(&script);
            if widget.has_data() {
                assert!(script.contains(&format!("{}_Serial for LocalUser", widget.var_prefix())));
            }
            if widget.hideable() {
                // Every widget hides itself from the menu's LocalUser flags.
                assert!(script.contains(&format!(
                    "declare Boolean {} for LocalUser",
                    widget.hidden_var()
                )));
                assert!(script.contains("PitWall_HideAll"));
            }
        }
    }
}

#[test]
fn ranking_has_a_header_podium_and_pinned_own_line() {
    let renderer = Renderer::default();
    let xml = renderer
        .view(WidgetId::Ranking, Lang::Fr, &translator())
        .unwrap();
    let script = check_xml(&xml);
    // Header filled from the data, default title before it arrives.
    assert!(xml.contains(r#"id="pw-title""#) && xml.contains(r#"id="pw-subtitle""#));
    assert!(script.contains("Title.Value = _Data.title;"));
    // Podium colors from the theme.
    assert!(script.contains(&crate::Theme::vec3(&crate::Theme::default().gold)));
    // The local player's line is pinned below the visible rows.
    assert!(xml.contains(r#"id="own""#) && script.contains("Pinned = Own >= C_MaxRows"));
}

#[test]
fn scoreboard_takes_the_scores_table_layer() {
    let renderer = Renderer::default();
    let xml = renderer
        .view(WidgetId::Scoreboard, Lang::En, &translator())
        .unwrap();
    check_script(&check_xml(&xml));
    assert!(xml.contains(r#"layer="ScoresTable""#));
    // 16 visible lines and the pinned slot of the local player.
    assert!(xml.contains(r#"id="sb-row-15""#) && xml.contains(r#"id="sb-row-16""#));
    assert!(!xml.contains(r#"id="sb-row-17""#));
    assert!(xml.contains("Own >= C_MaxRows"));
}

#[test]
fn scoreboard_lines_offer_spectate_and_profile() {
    let renderer = Renderer::default();
    let i18n = translator();
    let player = renderer.scoreboard(Lang::Fr, &i18n, false).unwrap();
    let script = check_xml(&player);
    check_script(&script);
    assert!(player.contains(r#"id="sb-row-0-hit""#) && player.contains(r#"id="sb-row-16-spec""#));
    assert!(player.contains(r#"text="SPECTATER""#) && player.contains(r#"id="sb-row-3-profile""#));
    assert!(script.contains("SetSpectateTarget(_Login)"));
    assert!(script.contains("TMGame_ScoresTable_OpenProfileUserId for ClientUI"));
    assert!(
        !player.contains("-mute\""),
        "players get no moderation buttons"
    );

    // Country flag and club tag come from the game's user data.
    assert!(player.contains(r#"id="sb-row-16-flag""#));
    assert!(script.contains("Score.User.CountryFlagUrl") && script.contains("Score.User.ClubTag"));

    let referee = renderer.scoreboard(Lang::En, &i18n, true).unwrap();
    check_script(&check_xml(&referee));
    assert!(
        referee.contains(r#"id="sb-row-2-mute""#) && referee.contains(r#"id="sb-row-2-unmute""#)
    );
    assert!(referee.contains(r#""pw:cmd:"^_Action^" "^_Login"#));
}

#[test]
fn world_records_sit_above_the_best_times() {
    let renderer = Renderer::default();
    let xml = renderer
        .view(WidgetId::WorldRecords, Lang::Fr, &translator())
        .unwrap();
    let script = check_xml(&xml);
    check_script(&script);
    assert!(xml.contains("RECORDS DU MONDE"));
    assert!(xml.contains(r#"id="wr-4""#) && !xml.contains(r#"id="wr-5""#));
    // The player's own line, hidden while they are in the top; nothing shown without data.
    assert!(xml.contains(r#"id="wr-own""#) && script.contains("InTop"));
    assert!(script.contains("Root.Visible = HasData &&"));
    let layout = renderer.layout();
    assert!(layout.position(WidgetId::Records).y < layout.position(WidgetId::WorldRecords).y);
    // The best times keep at most 8 lines below.
    let records = renderer
        .view(WidgetId::Records, Lang::En, &translator())
        .unwrap();
    assert!(records.contains("#Const C_MaxRows 8"));
}

#[test]
fn vote_widget_answers_through_commands() {
    let xml = Renderer::default()
        .view(WidgetId::Vote, Lang::Fr, &translator())
        .unwrap();
    let script = check_xml(&xml);
    check_script(&script);
    assert!(xml.contains("PASSER LA MAP ?") && xml.contains("RECOMMENCER LA MAP ?"));
    assert!(script.contains(r#"TriggerPageAction("pw:cmd:yes")"#));
    assert!(script.contains(r#"TriggerPageAction("pw:cmd:no")"#));
    assert!(!WidgetId::Vote.hideable());
}

#[test]
fn views_are_localized() {
    let renderer = Renderer::default();
    let i18n = translator();
    let fr = renderer.view(WidgetId::Ranking, Lang::Fr, &i18n).unwrap();
    assert!(fr.contains("CLASSEMENT LIVE"));
    let en = renderer.view(WidgetId::CupInfo, Lang::En, &i18n).unwrap();
    assert!(en.contains("\"READY ("), "{en}");
}

#[test]
fn data_layer_escapes_json() {
    let renderer = Renderer::default();
    let data = RankingData {
        mode: "time".into(),
        title: "SPEED MAPPING".into(),
        subtitle: "MAP 1/20".into(),
        cutline: 4,
        rows: vec![RankingRow {
            login: "abc".into(),
            name: "$f00<Evil> \"Nick\" & co".into(),
            rank: 1,
            time: 51_234,
            points: 0,
            gap: 0,
            finished: false,
        }],
    };
    let xml = renderer.data(WidgetId::Ranking, &data).unwrap();
    check_script(&check_xml(&xml));
    // The attribute decodes back to the exact JSON.
    let mut reader = Reader::from_str(&xml);
    let mut json = None;
    loop {
        match reader.read_event().unwrap() {
            Event::Empty(e) | Event::Start(e) => {
                for attr in e.attributes().flatten() {
                    if attr.key.as_ref() == "data-json" {
                        json = Some(
                            attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .unwrap()
                                .into_owned(),
                        );
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let json = json.expect("data-json attribute");
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["rows"][0]["name"], "$f00<Evil> \"Nick\" & co");
    assert_eq!(parsed["cutline"], 4);
}

#[test]
fn admin_panel_depends_on_role() {
    let renderer = Renderer::default();
    let i18n = translator();
    let referee = AdminContext {
        role: Role::Referee,
        formats: vec![("cup".into(), "Cup".into())],
        cup_loaded: true,
        time_attack: false,
    };
    let xml = renderer.admin_panel(Lang::En, &i18n, &referee).unwrap();
    check_xml(&xml);
    assert!(xml.contains("pw:cmd:pause"));
    assert!(xml.contains("pw:cmd:cup go"));
    assert!(
        !xml.contains("pw:cmd:addtime"),
        "no time limit outside Time Attack"
    );
    let time_attack = AdminContext {
        time_attack: true,
        ..referee.clone()
    };
    let xml = renderer.admin_panel(Lang::En, &i18n, &time_attack).unwrap();
    assert!(xml.contains("pw:cmd:addtime 5"));
    assert!(
        !xml.contains("pw:cmd:mode cup"),
        "referees cannot switch formats"
    );
    let admin = AdminContext {
        role: Role::Admin,
        ..referee
    };
    let xml = renderer.admin_panel(Lang::En, &i18n, &admin).unwrap();
    assert!(xml.contains("pw:cmd:mode cup"));
    assert!(xml.contains("pw:cmd:next"));
}

#[test]
fn menu_reflects_preferences() {
    let renderer = Renderer::default();
    let i18n = translator();
    let menu = MenuContext {
        widgets: vec![(WidgetId::Ranking, true), (WidgetId::Records, false)],
        all_hidden: false,
    };
    let xml = renderer.menu(Lang::Fr, &i18n, &menu).unwrap();
    check_script(&check_xml(&xml));
    // Initial labels (before the script runs) and saved states pushed to the client.
    assert!(xml.contains("● Classement"));
    assert!(xml.contains("○ Records"));
    assert!(xml.contains("PitWall_Ranking_Hidden = False;"));
    assert!(xml.contains("PitWall_Records_Hidden = True;"));
    assert!(xml.contains("PitWall_HideAll = False;"));
    // Clicks are handled by the script, which tells the server the new state.
    assert!(xml.contains("pw:ui:set:records:1") && xml.contains("pw:ui:set:records:0"));
    assert!(!xml.contains("action="), "no server round trip on click");

    let hidden = renderer
        .menu(
            Lang::Fr,
            &i18n,
            &MenuContext {
                widgets: vec![],
                all_hidden: true,
            },
        )
        .unwrap();
    assert!(hidden.contains("PitWall_HideAll = True;"));
    assert!(hidden.contains("pw:ui:all:0"));
}

fn maps_window(count: usize) -> MapsWindow {
    MapsWindow {
        maps: (1..=count)
            .map(|i| MapEntry {
                name: format!("$f00Map <{i}> & \"co\""),
                author: format!("author{i}"),
            })
            .collect(),
        current: Some(1),
        page: 0,
        query: String::new(),
        results: vec![],
    }
}

#[test]
fn maps_window_renders_the_playlist() {
    let renderer = Renderer::default();
    let i18n = translator();
    for lang in Lang::ALL {
        let xml = renderer.maps_window(lang, &i18n, &maps_window(3)).unwrap();
        check_script(&check_xml(&xml));
        assert!(xml.contains(r#"id="PitWall_Maps""#));
    }
    let xml = renderer
        .maps_window(Lang::En, &i18n, &maps_window(3))
        .unwrap();
    // Map names are escaped.
    assert!(xml.contains("Map &#60;1&#62; &#38; &#34;co&#34;"), "{xml}");
    assert!(xml.contains("pw:cmd:map move 2 1") && xml.contains("pw:cmd:map move 2 3"));
    assert!(
        !xml.contains("pw:cmd:map move 1 0"),
        "first map cannot go up"
    );
    assert!(
        !xml.contains("pw:cmd:map move 3 4"),
        "last map cannot go down"
    );
    // Map 2 is being played: no "play now" / "next" for it.
    assert!(!xml.contains("pw:cmd:map play 2") && xml.contains("pw:cmd:map play 3"));
    assert!(xml.contains("pw:cmd:map remove 2"));
    assert!(xml.contains("pw:cmd:map clear"));
    assert!(xml.contains("pw:cmd:map club ") && xml.contains("pw:cmd:map room "));
    assert!(!xml.contains("pw:maps:page:"), "a single page");
}

#[test]
fn maps_window_pages_and_search() {
    let renderer = Renderer::default();
    let i18n = translator();
    let mut window = maps_window(40);
    window.page = 1;
    window.query = "winter".into();
    window.results = vec![TmxEntry {
        id: 361_306,
        name: "Snow".into(),
        author: "someone".into(),
    }];
    let xml = renderer.maps_window(Lang::Fr, &i18n, &window).unwrap();
    check_script(&check_xml(&xml));
    assert!(xml.contains("Page 2/3"));
    assert!(xml.contains("pw:maps:page:0") && xml.contains("pw:maps:page:2"));
    // Second page: maps 16 to 30.
    assert!(xml.contains("pw:cmd:map remove 16") && xml.contains("pw:cmd:map remove 30"));
    assert!(!xml.contains("pw:cmd:map remove 15") && !xml.contains("pw:cmd:map remove 31"));
    assert!(xml.contains("pw:cmd:map tmx 361306\"") && xml.contains("pw:cmd:map tmx 361306 next"));
    assert!(xml.contains(r#"default="winter""#));

    window.page = 99;
    window.results.clear();
    let xml = renderer.maps_window(Lang::Fr, &i18n, &window).unwrap();
    assert!(xml.contains("Page 3/3"), "page is clamped");
    assert!(xml.contains("Aucune map trouvée"));
}

/// Characters seen rendering in game (2026-10-19): ASCII, Latin letters and a few symbols.
/// ✕ ▶ ▾ ▸ showed up as empty boxes, so anything else must be tried in game first.
fn assert_known_glyphs(xml: &str) {
    let ok = |c: char| {
        c.is_ascii() || ('\u{00C0}'..='\u{017F}').contains(&c) || "●○▲▼«»·…—’\u{00A0}".contains(c)
    };
    let bad: String = xml.chars().filter(|c| !ok(*c)).collect();
    assert!(bad.is_empty(), "glyphs not known to render in game: {bad}");
}

#[test]
fn pages_only_use_glyphs_the_game_renders() {
    let renderer = Renderer::default();
    let i18n = translator();
    for lang in Lang::ALL {
        for widget in WidgetId::ALL {
            assert_known_glyphs(&renderer.view(widget, lang, &i18n).unwrap());
        }
        let admin = AdminContext {
            role: Role::SuperAdmin,
            formats: vec![],
            cup_loaded: true,
            time_attack: true,
        };
        assert_known_glyphs(&renderer.admin_panel(lang, &i18n, &admin).unwrap());
        let menu = MenuContext {
            widgets: vec![(WidgetId::Ranking, true), (WidgetId::Records, false)],
            all_hidden: true,
        };
        assert_known_glyphs(&renderer.menu(lang, &i18n, &menu).unwrap());
        assert_known_glyphs(&renderer.maps_window(lang, &i18n, &maps_window(40)).unwrap());
    }
}

#[test]
fn hide_page_is_empty() {
    let xml = Renderer::default().hide(WidgetId::Records).unwrap();
    assert_eq!(
        xml.trim(),
        r#"<manialink id="PitWall_Records" version="3"></manialink>"#
    );
}

#[test]
fn maniascript_strings() {
    assert_eq!(
        ms_string(r#"l'admin "x" \ -- y"#),
        r#"l'admin \"x\" \\ - - y"#
    );
}

#[test]
fn catalogs_have_the_same_keys() {
    let [(_, en), (_, fr)] = catalogs();
    let en: toml::Table = toml::from_str(en).unwrap();
    let fr: toml::Table = toml::from_str(fr).unwrap();
    fn keys(prefix: &str, t: &toml::Table, out: &mut Vec<String>) {
        for (k, v) in t {
            let key = format!("{prefix}{k}");
            match v {
                toml::Value::Table(t) => keys(&format!("{key}."), t, out),
                _ => out.push(key),
            }
        }
    }
    let (mut a, mut b) = (Vec::new(), Vec::new());
    keys("", &en, &mut a);
    keys("", &fr, &mut b);
    a.sort();
    b.sort();
    assert_eq!(a, b);
}
