//! Liste des « grandes décisions depuis 1873 » du Conseil d'État : pages HTML
//! publiques, sans API, trois pages de liste et une fiche par décision. Les
//! pages de liste sont l'index, qui bouge quand le catalogue s'allonge, et se
//! relisent à chaque passage ; seules les fiches, une par slug, sont cachées
//! sous `state_dir`.

use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::error::{Result, SourceError};

pub const BASE_URL: &str = "https://www.conseil-etat.fr";
pub const LIST_PATH: &str = "/decisions-de-justice/jurisprudence/les-grandes-decisions-depuis-1873";
/// User-Agent de contact (scraping courtois d'un site public sans API).
pub const USER_AGENT: &str = "librejustice-ce-list/0.1 (+https://librejustice.fr)";
/// Throttle de courtoisie entre requêtes réseau, appliqué par l'appelant.
pub const THROTTLE: Duration = Duration::from_millis(500);

pub fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(SourceError::from)
}

/// Une page de liste (`?page=N`, 1-based).
pub async fn list_page(client: &reqwest::Client, page: u32) -> Result<String> {
    get(client, &format!("{BASE_URL}{LIST_PATH}?page={page}")).await
}

/// La fiche d'une entrée, depuis le chemin absolu que la liste sert.
/// `fetched` dit si le réseau a été touché, l'appelant ne throttlant que dans
/// ce cas.
pub async fn entry_page_cached(
    client: &reqwest::Client,
    cache_dir: &Path,
    path: &str,
) -> Result<(String, bool)> {
    let slug = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path);
    if slug.is_empty() || slug.contains(['/', '\\']) {
        return Err(SourceError::Invalid(format!(
            "chemin de fiche illisible: {path}"
        )));
    }
    let cache = cache_dir.join(format!("{slug}.html"));
    if cache.exists() {
        return Ok((fs::read_to_string(&cache)?, false));
    }
    let html = get(client, &format!("{BASE_URL}{path}")).await?;
    fs::create_dir_all(cache_dir)?;
    fs::write(&cache, &html)?;
    Ok((html, true))
}

async fn get(client: &reqwest::Client, url: &str) -> Result<String> {
    let resp = client.get(url).send().await?;
    let status = resp.status().as_u16();
    if status != 200 {
        return Err(SourceError::Invalid(format!("{url}: statut {status}")));
    }
    Ok(resp.text().await?)
}
