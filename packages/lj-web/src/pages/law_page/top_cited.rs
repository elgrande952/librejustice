//! Palmarès « Articles les plus cités » de la colonne droite, partagé par la
//! page d'une division et celle d'un code entier : même classement, deux
//! périmètres.

use leptos::prelude::*;
use leptos_router::components::A;
use lj_dtos::TopCitedArticle;

use crate::helpers::{format_article_num, group_thousands};

/// Bloc du palmarès. Aucun article cité ⇒ pas de bloc.
pub(crate) fn top_cited_block(code: &str, top: Vec<TopCitedArticle>) -> AnyView {
    if top.is_empty() {
        return ().into_any();
    }
    let code = code.to_string();
    let items = top
        .into_iter()
        .map(|article| {
            let code = code.clone();
            view! { <TopCitedRow article=article code=code /> }
        })
        .collect_view();
    view! {
        <div class="flex flex-col gap-2">
            <h2 class="font-sans text-base text-[var(--color-ink)]">"Articles les plus cités"</h2>
            <ul class="flex flex-col gap-2">{items}</ul>
        </div>
    }
    .into_any()
}

/// Une entrée du palmarès : numéro formaté comme la vue-lecture (le `label` du
/// DTO est brut), hover card d'article sur le lien.
#[component]
fn TopCitedRow(article: TopCitedArticle, code: String) -> impl IntoView {
    use crate::components::hover_preview::{HoverPreview, PreviewKind};
    let label = format!("Article {}", format_article_num(&article.label));
    let count = format!("{} déc.", group_thousands(article.count));
    let link = match article.href {
        Some(href) => {
            let kind = PreviewKind::Article {
                code,
                num: article.num_key,
                date: None,
            };
            view! {
                <HoverPreview kind=kind>
                    <A
                        href=href
                        attr:class="text-sm text-[var(--color-ink)] underline-offset-4 hover:text-[var(--color-accent)] hover:underline"
                    >
                        {label}
                    </A>
                </HoverPreview>
            }
            .into_any()
        }
        None => view! { <span class="text-sm text-[var(--color-ink)]">{label}</span> }.into_any(),
    };
    view! {
        <li class="flex items-baseline justify-between gap-2">
            {link}
            <span class="shrink-0 text-xs tabular-nums text-[var(--color-ink-subtle)]">{count}</span>
        </li>
    }
}
