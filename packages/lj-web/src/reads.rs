//! Tells the server what a page shows: a page served from the CDN cache never
//! reaches it, so only the browser can count the reading.

#[cfg(feature = "hydrate")]
static DOCUMENT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
#[cfg(feature = "hydrate")]
static LANDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The address the browser loaded, taken before the router can move it.
#[cfg(feature = "hydrate")]
pub fn note_document() {
    if let Some(href) = web_sys::window().and_then(|w| w.location().href().ok()) {
        let _ = DOCUMENT.set(href);
    }
}

/// A landing is the loaded document itself, reported once: a search page
/// followed by a click on a hit is a navigation, and so is a return to it.
#[cfg(feature = "hydrate")]
pub fn report(kind: lj_dtos::ReadKind) {
    use std::sync::atomic::Ordering;

    let Some(window) = web_sys::window() else {
        return;
    };
    let href = window.location().href().ok();
    let landing =
        href.is_some() && DOCUMENT.get() == href.as_ref() && !LANDED.swap(true, Ordering::Relaxed);
    let (referrer, utm_source) = match landing {
        true => (
            window
                .document()
                .map(|d| d.referrer())
                .filter(|r| !r.is_empty()),
            window
                .location()
                .search()
                .ok()
                .as_deref()
                .and_then(utm_source),
        ),
        false => (None, None),
    };
    let report = lj_dtos::ReadReport {
        kind,
        reference: window.location().pathname().unwrap_or_default(),
        landing,
        referrer,
        utm_source,
    };
    leptos::task::spawn_local(async move {
        let _ = crate::api::ApiClient::from_context()
            .report_read(&report)
            .await;
    });
}

/// An annuaire search, reported for the same reason as a reading.
#[cfg(feature = "hydrate")]
pub fn report_entity_search(query: String) {
    let report = lj_dtos::EntitySearchReport { query };
    leptos::task::spawn_local(async move {
        let _ = crate::api::ApiClient::from_context()
            .report_entity_search(&report)
            .await;
    });
}

#[cfg(feature = "hydrate")]
fn utm_source(search: &str) -> Option<String> {
    let value = search
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| pair.strip_prefix("utm_source="))?;
    js_sys::decode_uri_component(&value.replace('+', " "))
        .ok()
        .map(String::from)
        .filter(|v| !v.is_empty())
}
