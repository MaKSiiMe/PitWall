//! Live check of the TMX client: `cargo run -p pitwall-integrations --example tmx -- winter`.

use pitwall_integrations::{http_client, tmx::Tmx};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let query = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "winter".to_owned());
    let tmx = Tmx::new(http_client()?);
    let found = tmx.search(&query, 3).await?;
    for map in &found {
        println!(
            "{} {} by {} ({} ms)",
            map.map_id, map.name, map.uploader.name, map.medals.author
        );
    }
    if let Some(first) = found.first() {
        let by_id = tmx.map(first.map_id).await?;
        let file = tmx.download(first.map_id).await?;
        println!(
            "lookup ok: {} / download: {} bytes, starts with {:?}",
            by_id.map_uid,
            file.len(),
            &file[..3]
        );
    }
    Ok(())
}
