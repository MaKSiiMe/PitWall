//! Prints a rendered widget: `cargo run -p pitwall-ui --example render -- ranking fr`.

use pitwall_core::{Lang, Translator};
use pitwall_ui::{Renderer, WidgetId, catalogs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let widget = args
        .next()
        .and_then(|w| WidgetId::parse(&w))
        .unwrap_or(WidgetId::Ranking);
    let lang = args
        .next()
        .and_then(|l| Lang::from_code(&l))
        .unwrap_or(Lang::En);
    let i18n = Translator::with_core_catalogs()?;
    for (lang, catalog) in catalogs() {
        i18n.add_catalog(lang, catalog)?;
    }
    println!("{}", Renderer::default().view(widget, lang, &i18n)?);
    Ok(())
}
