//! Page `/reset-password`.
//!
//! Atterrissage du lien envoyé par mail. Sécurité : le nouveau mot de passe part
//! AVEC le jeton du lien, en un seul POST — c'est le jeton qui prouve le
//! contrôle de la boîte, jamais la session courante. Sans JavaScript :
//! formulaire → `/internal/auth/reset` → 303 avec session ; une erreur attendue
//! rebondit ici, son code en query, le jeton conservé tant qu'il est valide.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};
use leptos_router::components::A;

use super::login_page::auth_errors::translate_auth_error;

const PASSWORD_HINT: &str =
    "Au moins 8 caractères, dont une minuscule, une majuscule et un chiffre.";

#[component]
pub fn ResetPasswordPage() -> impl IntoView {
    let query = leptos_router::hooks::use_query_map();
    let token = move || query.with(|q| q.get("token").unwrap_or_default());
    let error = move || query.with(|q| q.get("error"));

    view! {
        <Title text="Réinitialiser le mot de passe - LibreJustice" />
        <Meta name="robots" content="noindex" />
        <div class="mx-auto flex max-w-sm flex-col gap-8 px-4 py-16">
            <h1 class="font-sans text-2xl text-[var(--color-ink)]">
                "Réinitialiser le mot de passe"
            </h1>
            <Show
                when=move || !token().is_empty()
                fallback=move || {
                    view! {
                        <div class="flex flex-col gap-3">
                            <p class="text-sm text-red-600">
                                {move || match error() {
                                    Some(code) => translate_auth_error(Some(&code)),
                                    None => {
                                        "Lien incomplet : ouvrez celui reçu par courriel."
                                            .to_string()
                                    }
                                }}
                            </p>
                            <p class="text-sm text-[var(--color-ink-muted)]">
                                <A
                                    href="/login"
                                    attr:class="text-[var(--color-accent)] underline hover:opacity-80"
                                >
                                    "Demander un nouveau lien"
                                </A>
                            </p>
                        </div>
                    }
                }
            >
                <form method="post" action="/internal/auth/reset" class="flex flex-col gap-4">
                    <input type="hidden" name="token" value=token />
                    <div class="flex flex-col gap-1.5">
                        <label for="password" class="text-sm text-[var(--color-ink-muted)]">
                            "Nouveau mot de passe"
                        </label>
                        <input
                            id="password"
                            name="password"
                            type="password"
                            autocomplete="new-password"
                            required
                            minlength="8"
                            class="rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 py-2 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]"
                        />
                        <p class="text-xs text-[var(--color-ink-subtle)]">{PASSWORD_HINT}</p>
                    </div>
                    <div class="flex flex-col gap-1.5">
                        <label for="confirm" class="text-sm text-[var(--color-ink-muted)]">
                            "Confirmer le mot de passe"
                        </label>
                        <input
                            id="confirm"
                            name="confirm"
                            type="password"
                            autocomplete="new-password"
                            required
                            minlength="8"
                            class="rounded-md border border-[var(--color-rule)] bg-[var(--color-parchment)] px-3 py-2 text-sm text-[var(--color-ink)] outline-none focus:border-[var(--color-accent)]"
                        />
                    </div>
                    <Show when=move || error().is_some()>
                        <p class="text-sm text-red-600">
                            {move || translate_auth_error(error().as_deref())}
                        </p>
                    </Show>
                    <button
                        type="submit"
                        class="rounded-md bg-[var(--color-accent)] px-4 py-2.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
                    >
                        "Mettre à jour"
                    </button>
                </form>
            </Show>
        </div>
    }
}
