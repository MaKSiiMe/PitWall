//! Live check of a club campaign import:
//! `cargo run -p pitwall-integrations --example nadeo_campaign -- <club id> <campaign id>`.
//! Reads the dedicated server account from `PITWALL_NADEO__LOGIN` / `PITWALL_NADEO__PASSWORD`.

use pitwall_integrations::{http_client, nadeo::Nadeo};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1).map(|a| a.parse::<i64>());
    let (Some(Ok(club)), Some(Ok(campaign))) = (args.next(), args.next()) else {
        return Err("usage: nadeo_campaign <club id> <campaign id>".into());
    };
    let login = std::env::var("PITWALL_NADEO__LOGIN")?;
    let password = std::env::var("PITWALL_NADEO__PASSWORD")?;
    let nadeo = Nadeo::new(http_client()?, &login, &password)?;
    let list = nadeo.club_campaign(club, campaign).await?;
    println!("{}: {} uids", list.name, list.map_uids.len());
    let maps = nadeo.maps(&list.map_uids).await?;
    println!("{} maps known to the core API", maps.len());
    for uid in &list.map_uids {
        let Some(map) = maps.iter().find(|m| &m.map_uid == uid) else {
            println!("{uid}: MISSING from the core API");
            continue;
        };
        match nadeo.download(map).await {
            Ok(data) => println!(
                "{uid} {:>9} bytes ({:>9} base64)  {:?}  {}",
                data.len(),
                data.len().div_ceil(3) * 4,
                String::from_utf8_lossy(&data[..3.min(data.len())]),
                map.name
            ),
            Err(err) => println!("{uid}: download failed: {err}  {}", map.name),
        }
    }
    Ok(())
}
