//! Keys of the public API, on the account screen. An island rather than a
//! native form: the secret is shown once, in the response that creates it, and
//! a form would carry it in a redirect URL.

use leptos::prelude::*;
use lj_dtos::ApiKeySummary;

const CHAMP: &str =
    "flex-1 rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] \
     px-3 py-2 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]";
const BOUTON: &str = "rounded-md bg-[var(--color-accent)] px-4 py-2 text-sm font-medium \
     text-white transition-opacity hover:opacity-90 disabled:opacity-50";
const CARTE: &str =
    "flex items-center justify-between rounded-md border border-[var(--color-rule)] px-4 py-3";
const ACTION_RETRAIT: &str =
    "text-sm text-[var(--color-ink-muted)] underline hover:text-[var(--color-ink)]";

/// Fallback label: the API rejects an empty one.
#[cfg(feature = "hydrate")]
fn default_label() -> String {
    let d = js_sys::Date::new_0();
    format!(
        "Clé du {:02}/{:02}/{}",
        d.get_date(),
        d.get_month() + 1,
        d.get_full_year()
    )
}

#[component]
pub fn ApiKeysSettings() -> impl IntoView {
    let cles = RwSignal::new(Vec::<ApiKeySummary>::new());
    let secret = RwSignal::new(None::<String>);
    let erreur = RwSignal::new(None::<String>);
    let libelle = RwSignal::new(String::new());
    let en_cours = RwSignal::new(false);

    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        leptos::task::spawn_local(async move {
            match crate::api::client::ApiClient::from_context()
                .list_api_keys()
                .await
            {
                Ok(resp) => cles.set(resp.items),
                Err(e) => erreur.set(Some(e.message)),
            }
        });
    });

    let creer = move |_| {
        if en_cours.get() {
            return;
        }
        en_cours.set(true);
        erreur.set(None);
        #[cfg(feature = "hydrate")]
        {
            let saisi = libelle.get().trim().to_string();
            let nom = if saisi.is_empty() {
                default_label()
            } else {
                saisi
            };
            leptos::task::spawn_local(async move {
                match crate::api::client::ApiClient::from_context()
                    .create_api_key(&nom)
                    .await
                {
                    Ok(cree) => {
                        secret.set(Some(cree.secret));
                        cles.update(|c| c.insert(0, cree.key));
                        libelle.set(String::new());
                    }
                    Err(e) => erreur.set(Some(e.message)),
                }
                en_cours.set(false);
            });
        }
        #[cfg(not(feature = "hydrate"))]
        en_cours.set(false);
    };

    let revoquer = move |id: String| {
        #[cfg(feature = "hydrate")]
        leptos::task::spawn_local(async move {
            match crate::api::client::ApiClient::from_context()
                .revoke_api_key(&id)
                .await
            {
                Ok(()) => cles.update(|c| c.retain(|k| k.id != id)),
                Err(e) => erreur.set(Some(e.message)),
            }
        });
        #[cfg(not(feature = "hydrate"))]
        let _ = id;
    };

    view! {
        <div class="flex flex-col gap-3">
            <p class="text-sm text-[var(--color-ink-muted)]">
                "Une clé donne accès à l'API publique, la même recherche que le connecteur MCP, en REST. Elle est gratuite et se révoque à tout moment."
                <a href="/api-guide" class="ml-1 text-[var(--color-accent)] underline hover:opacity-80">
                    "Voir la documentation"
                </a>
            </p>

            <div class="flex gap-2">
                <input
                    type="text"
                    class=CHAMP
                    placeholder="Nom de la clé (facultatif)"
                    maxlength="64"
                    prop:value=move || libelle.get()
                    on:input:target=move |ev| libelle.set(ev.target().value())
                />
                <button type="button" class=BOUTON on:click=creer disabled=move || en_cours.get()>
                    "Créer une clé"
                </button>
            </div>

            {move || {
                secret
                    .get()
                    .map(|s| {
                        view! {
                            <div class="flex flex-col gap-2 rounded-md border border-[var(--color-accent)] bg-[var(--color-vellum)] p-4">
                                <p class="text-sm font-medium text-[var(--color-ink)]">
                                    "Copiez cette clé maintenant : elle ne sera plus affichée."
                                </p>
                                <code class="break-all rounded-md bg-[var(--color-parchment)] px-3 py-2 font-mono text-sm text-[var(--color-ink)]">
                                    {s}
                                </code>
                            </div>
                        }
                    })
            }}

            {move || {
                erreur
                    .get()
                    .map(|e| {
                        view! {
                            <p class="text-sm text-red-700 dark:text-red-400">
                                {"La clé n'a pas pu être traitée : "} {e}
                            </p>
                        }
                    })
            }}

            <For each=move || cles.get() key=|k| k.id.clone() let:cle>
                {
                    let id = cle.id.clone();
                    view! {
                        <div class=CARTE>
                            <div class="flex flex-col gap-0.5">
                                <p class="text-sm text-[var(--color-ink)]">{cle.label.clone()}</p>
                                <p class="font-mono text-xs text-[var(--color-ink-subtle)]">
                                    {format!("{}…", cle.prefix)}
                                </p>
                            </div>
                            <button
                                type="button"
                                class=ACTION_RETRAIT
                                on:click=move |_| revoquer(id.clone())
                            >
                                "Révoquer"
                            </button>
                        </div>
                    }
                }
            </For>
        </div>
    }
}
