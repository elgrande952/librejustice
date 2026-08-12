use leptos::prelude::*;
use leptos_meta::{Meta, Title};
use leptos_router::components::A;

use crate::components::app_shell::SuppressSiteDescription;
use crate::seo::not_found_meta;

#[component]
pub fn NotFound() -> impl IntoView {
    #[cfg(feature = "ssr")]
    {
        use axum::http::StatusCode;
        use leptos_axum::ResponseOptions;
        let response = expect_context::<ResponseOptions>();
        response.set_status(StatusCode::NOT_FOUND);
    }

    // Le 404 pose `robots noindex` sans description : on supprime la
    // `<meta description>` générique du shell, rétablie au démontage.
    if let Some(suppress) = use_context::<SuppressSiteDescription>() {
        suppress.0.set(true);
        on_cleanup(move || suppress.0.set(false));
    }

    let meta = not_found_meta();
    let robots = meta.robots.expect("404 meta exposes a robots directive");

    view! {
        <Title text=meta.title />
        <Meta name="robots" content=robots />
        <div class="mx-auto flex w-full max-w-2xl flex-1 flex-col items-start gap-4 px-4 py-24 sm:px-6 lg:px-8">
            <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                "Erreur 404"
            </p>
            <h1 class="font-sans text-4xl text-[var(--color-ink)]">"Page introuvable"</h1>
            <p class="text-[var(--color-ink-muted)]">
                "L'adresse demandée n'existe pas ou a été déplacée."
            </p>
            <A
                href="/"
                attr:class="mt-auto inline-flex items-center gap-1.5 pt-6 text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)]"
            >
                <span aria-hidden="true">"←"</span>
                " Retour à l'accueil"
            </A>
        </div>
    }
}
