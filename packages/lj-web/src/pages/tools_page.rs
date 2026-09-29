//! Page `/tools`: the hub of what the site offers beyond its search bar.

use leptos::prelude::*;
use leptos_meta::{Meta, Title};

/// A card is a component, not inline markup: the children of a repeated
/// fragment must stay uniform, or hydration fails in release.
#[component]
fn ToolCard(
    state: &'static str,
    title: &'static str,
    body: &'static str,
    href: &'static str,
) -> impl IntoView {
    view! {
        <a
            href=href
            class="group flex flex-col gap-3 rounded-lg border border-[var(--color-rule)] bg-[var(--color-vellum)]/40 p-5 transition-colors hover:border-[var(--color-accent)]"
        >
            <span class="inline-flex w-fit items-center rounded-full border border-[var(--color-rule)] bg-[var(--color-parchment)] px-2.5 py-0.5 text-xs text-[var(--color-ink-subtle)]">
                {state}
            </span>
            <h2 class="font-sans text-xl text-[var(--color-ink)]">{title}</h2>
            <p class="text-sm leading-relaxed text-[var(--color-ink-muted)]">{body}</p>
        </a>
    }
}

const OUTILS: [(&str, &str, &str, &str); 3] = [
    (
        "Disponible",
        "Connecteur MCP",
        "Claude, ChatGPT ou votre propre agent cherchent dans la jurisprudence et les textes sans quitter leur fil de conversation.",
        "/mcp-guide",
    ),
    (
        "Disponible",
        "API publique",
        "La même base en REST, pour votre code. Recherche de décisions et de textes, clé gratuite.",
        "/api-guide",
    ),
    (
        "Bêta",
        "Anonymisation",
        "Des règles proposent les noms, vous cochez, et le PDF, le document Word ou l'image repart sans. Tout se passe dans votre navigateur : le fichier ne part nulle part.",
        "/redact",
    ),
];

#[component]
pub fn ToolsPage() -> impl IntoView {
    crate::pages::cache_public_5_min();

    view! {
        <Title text="Outils - LibreJustice" />
        <Meta
            name="description"
            content="Les outils LibreJustice : connecteur MCP pour les assistants, API REST, et anonymisation de documents dans le navigateur."
        />
        <div class="mx-auto flex w-full min-w-0 max-w-5xl flex-col gap-14 px-4 py-16 sm:px-6 lg:px-8">
            <section class="flex max-w-3xl flex-col gap-6">
                <p class="text-xs uppercase tracking-[0.2em] text-[var(--color-ink-subtle)]">
                    "Outils"
                </p>
                <h1
                    class="text-balance font-sans text-3xl leading-[1.05] tracking-tight text-[var(--color-ink)] sm:text-5xl"
                    style="font-variation-settings: 'wght' 300"
                >
                    "Ce que LibreJustice fait "
                    <em
                        class="not-italic text-[var(--color-accent)]"
                        style="font-variation-settings: 'wght' 750"
                    >
                        "en dehors de la recherche."
                    </em>
                </h1>
                <p class="text-lg leading-relaxed text-[var(--color-ink-muted)]">
                    "Le même fonds, que vous passiez par votre assistant ou par votre propre code, et de quoi anonymiser vos documents sans qu'ils quittent votre navigateur."
                </p>
            </section>

            <section class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                {OUTILS
                    .iter()
                    .map(|&(state, title, body, href)| {
                        view! { <ToolCard state=state title=title body=body href=href /> }
                    })
                    .collect_view()}
            </section>
        </div>
    }
}
