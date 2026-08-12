//! Page `/confirmer` : echange le `token_hash` d'un lien de mail (inscription,
//! lien magique, invitation, changement d'adresse) contre une session, puis rend
//! la main a `redirect_to`.
//!
//! `type=recovery` atterrit directement sur `/reinitialiser-mot-de-passe` : son
//! formulaire s'ouvre sur `PASSWORD_RECOVERY`, que seule la page deja montee
//! entend.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};
use leptos_router::components::A;

#[component]
pub fn ConfirmPage() -> impl IntoView {
    let error = RwSignal::new(None::<String>);

    #[cfg(feature = "hydrate")]
    {
        use super::login_page::browser;
        let query = leptos_router::hooks::use_query_map();
        let navigate = leptos_router::hooks::use_navigate();
        // `untracked` : token a usage unique, l'echange ne se rejoue pas.
        Effect::new(move |_| {
            let (token_hash, kind, dest) = query.with_untracked(|q| {
                (
                    q.get("token_hash").unwrap_or_default(),
                    q.get("type").unwrap_or_default(),
                    destination(q.get("redirect_to")),
                )
            });
            if token_hash.is_empty() || kind.is_empty() {
                error.set(Some("Lien incomplet ou invalide.".to_string()));
                return;
            }
            let navigate = navigate.clone();
            leptos::task::spawn_local(async move {
                match browser::verify_email_otp(&token_hash, &kind).await {
                    Ok(()) => navigate(&dest, Default::default()),
                    Err(msg) => error.set(Some(msg)),
                }
            });
        });
    }

    view! {
        <Title text="Confirmation - LibreJustice" />
        <Meta name="robots" content="noindex" />
        <div class="mx-auto flex max-w-sm flex-col gap-4 px-4 py-16">
            <h1 class="font-sans text-2xl text-[var(--color-ink)]">"Confirmation"</h1>
            <Show
                when=move || error.get().is_some()
                fallback=|| {
                    view! {
                        <p class="text-sm text-[var(--color-ink-muted)]">"Vérification du lien…"</p>
                    }
                }
            >
                <p class="text-sm text-red-600">{move || error.get().unwrap_or_default()}</p>
                <p class="text-sm text-[var(--color-ink-muted)]">
                    "Ce lien a peut-être expiré ou déjà été utilisé. "
                    <A
                        href="/connexion"
                        attr:class="text-[var(--color-accent)] underline hover:opacity-80"
                    >
                        "Demander un nouveau lien"
                    </A>
                </p>
            </Show>
        </div>
    }
}

/// Destination interne : `redirect_to` est absolu, on en retire l'origine et le
/// chemin restant se valide comme un `?next=` — hors origine, l'accueil.
#[cfg(feature = "hydrate")]
fn destination(raw: Option<String>) -> String {
    use super::login_page::browser;
    let raw = raw.unwrap_or_default();
    let origin = browser::location_origin();
    let path = raw
        .strip_prefix(origin.as_str())
        .unwrap_or(&raw)
        .to_string();
    super::login_page::next_url(Some(path))
}
