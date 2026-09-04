//! Page `/email-change` : les deux jetons du changement d'adresse atterrissent
//! ici — celui de l'adresse actuelle d'abord, puis celui de la
//! nouvelle. La consommation exige le POST du bouton, jamais le GET du lien —
//! que les scanners de boîtes suivent. Sans JavaScript : formulaire →
//! `/internal/auth/email-change` → 303 ; le serveur reconnaît l'étape au jeton.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

use super::login_page::auth_errors::translate_auth_error;

#[component]
pub fn EmailChangePage() -> impl IntoView {
    let query = leptos_router::hooks::use_query_map();
    let token = move || query.with(|q| q.get("token").unwrap_or_default());
    let sent = move || query.with(|q| q.get("sent").is_some());
    let error = move || query.with(|q| q.get("error"));

    view! {
        <Title text="Changement d'adresse - LibreJustice" />
        <Meta name="robots" content="noindex" />
        <div class="mx-auto flex max-w-sm flex-col gap-4 px-4 py-16">
            <h1 class="font-sans text-2xl text-[var(--color-ink)]">"Changement d'adresse"</h1>
            <Show
                when=move || !sent() && error().is_none() && !token().is_empty()
                fallback=move || {
                    view! {
                        <Show
                            when=sent
                            fallback=move || {
                                view! {
                                    <p class="text-sm text-red-600">
                                        {move || match error() {
                                            Some(code) => translate_auth_error(Some(&code)),
                                            None => "Lien incomplet : ouvrez celui reçu par courriel.".to_string(),
                                        }}
                                    </p>
                                    <p class="text-sm text-[var(--color-ink-muted)]">
                                        <a
                                            href="/profile"
                                            class="text-[var(--color-accent)] underline hover:opacity-80"
                                        >
                                            "Retour au profil"
                                        </a>
                                    </p>
                                }
                            }
                        >
                            <p class="text-sm text-[var(--color-ink-muted)]">
                                "Demande confirmée. Un lien de vérification vient de partir vers la nouvelle adresse : ouvrez-le pour finaliser le changement."
                            </p>
                        </Show>
                    }
                }
            >
                <p class="text-sm text-[var(--color-ink-muted)]">
                    "Confirmez cette étape du changement d'adresse de votre compte."
                </p>
                <form method="post" action="/internal/auth/email-change" class="flex flex-col gap-3">
                    <input type="hidden" name="token" value=token />
                    <button
                        type="submit"
                        class="rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
                    >
                        "Confirmer"
                    </button>
                </form>
            </Show>
        </div>
    }
}
