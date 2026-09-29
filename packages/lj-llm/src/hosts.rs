//! Local embedding hosts, in priority order.

/// Host used when `LIBREJUSTICE_EMBED_URL` is unset.
pub const DEFAULT_URL: &str = "http://127.0.0.1:8400/v1/embeddings";

/// One OpenAI-compatible embedding host of the ordered list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedHost {
    pub url: String,
    pub api_key: Option<String>,
    /// `host:port`, what traces and metrics name the host by.
    pub label: String,
}

/// Reads `LIBREJUSTICE_EMBED_URL` and `LIBREJUSTICE_EMBED_API_KEY` as two parallel
/// comma-separated lists, the key at the position of its URL, an empty entry for a
/// host without auth. The keys stay out of the URL list because every URL is written
/// to the HTTP spans.
pub fn parse_hosts(urls: &str, keys: Option<&str>) -> Result<Vec<EmbedHost>, String> {
    let urls: Vec<&str> = urls.split(',').map(str::trim).collect();
    let keys: Vec<Option<String>> = match keys {
        None => vec![None; urls.len()],
        Some(raw) => raw
            .split(',')
            .map(str::trim)
            .map(|k| (!k.is_empty()).then(|| k.to_string()))
            .collect(),
    };
    if keys.len() != urls.len() {
        return Err(format!(
            "LIBREJUSTICE_EMBED_API_KEY: {} keys for {} URLs, one per URL in the same order",
            keys.len(),
            urls.len()
        ));
    }
    urls.into_iter()
        .zip(keys)
        .map(|(url, api_key)| {
            let parsed = reqwest::Url::parse(url)
                .map_err(|e| format!("LIBREJUSTICE_EMBED_URL: invalid URL {url:?}: {e}"))?;
            let host = parsed
                .host_str()
                .ok_or_else(|| format!("LIBREJUSTICE_EMBED_URL: no host in {url:?}"))?;
            let port = parsed.port_or_known_default().unwrap_or_default();
            Ok(EmbedHost {
                url: url.to_string(),
                api_key,
                label: format!("{host}:{port}"),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_url_is_a_list_of_one() {
        let hosts = parse_hosts(DEFAULT_URL, Some("k")).unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].api_key.as_deref(), Some("k"));
        assert_eq!(hosts[0].label, "127.0.0.1:8400");
    }

    #[test]
    fn keys_pair_with_urls_by_position_and_an_empty_key_means_none() {
        let hosts = parse_hosts(
            "http://a:8400/v1/embeddings, http://b:8401/v1/embeddings",
            Some(",kb"),
        )
        .unwrap();
        assert_eq!(hosts[0].label, "a:8400");
        assert_eq!(hosts[0].api_key, None);
        assert_eq!(hosts[1].label, "b:8401");
        assert_eq!(hosts[1].api_key.as_deref(), Some("kb"));
    }

    #[test]
    fn a_key_count_that_differs_from_the_url_count_is_refused() {
        assert!(parse_hosts("http://a:8400,http://b:8400", Some("k")).is_err());
        assert!(parse_hosts("http://a:8400,,http://b:8400", None).is_err());
    }
}
