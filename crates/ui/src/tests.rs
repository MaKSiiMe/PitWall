//! Rendering tests: every page must be well-formed XML with a sane script.

use pitwall_core::{Lang, Role, Translator};
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::model::{RankingData, RankingRow};
use crate::render::{AdminContext, MenuContext, ms_string};
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
        }
    }
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
    };
    let xml = renderer.admin_panel(Lang::En, &i18n, &referee).unwrap();
    check_xml(&xml);
    assert!(xml.contains("pw:cmd:pause"));
    assert!(xml.contains("pw:cmd:cup go"));
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
    check_xml(&xml);
    assert!(xml.contains("● Classement"));
    assert!(xml.contains("○ Records"));
    assert!(xml.contains("pw:ui:toggle:records"));
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
    assert!(hidden.contains("pw:ui:showall"));
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
