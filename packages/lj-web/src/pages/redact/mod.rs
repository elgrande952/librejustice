//! The anonymisation workbench. One interface for the three kinds of document
//! the tool takes, and nothing leaves the machine: the file is read, drawn,
//! searched and rewritten in the page that shows it.

mod bridge;
mod engine;
mod sidebar;
mod state;
mod toolbar;
mod viewer;

use leptos::prelude::*;

pub use state::{Session, Status};
pub use toolbar::Toolbar;

const ACCEPT: &str = ".pdf,.docx,.odt,.doc,.htm,.html,.png,.jpg,.jpeg,.webp";
const CHOOSE: &str = "h-10 rounded-md bg-[var(--color-ink)] px-4 text-sm font-medium text-[var(--color-parchment)] transition-colors hover:bg-[var(--color-ink-muted)]";
const FRAME: &str = "flex min-h-0 w-full flex-1 flex-col overflow-hidden md:flex-row rounded-lg border border-[var(--color-rule)] bg-[var(--color-parchment)]";

#[component]
pub fn RedactTool(session: Session) -> impl IntoView {
    let bar = expect_context::<crate::components::decision_bar::WorkbenchSignal>();
    bar.set(Some(session));
    on_cleanup(move || bar.set(None));
    #[cfg(feature = "hydrate")]
    {
        engine::bind_keys(session);
        let texts = Memo::new(move |_| {
            session
                .needles
                .with(|list| list.iter().map(|n| n.text.clone()).collect::<Vec<_>>())
        });
        Effect::new(move |_| {
            let _ = session.generation.get();
            engine::refresh(session, texts.get());
        });
        Effect::new(move |_| {
            let _ = session.generation.get();
            engine::relabel(session);
        });
    }

    let on_change = move |event: leptos::ev::Event| {
        let _ = &event;
        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::JsCast;
            if let Some(input) = event
                .target()
                .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
            {
                if let Some(file) = input.files().and_then(|files| files.get(0)) {
                    engine::adopt(session, file);
                }
                input.set_value("");
            }
        }
    };
    let on_over = move |event: leptos::ev::DragEvent| event.prevent_default();
    let on_drop = move |event: leptos::ev::DragEvent| {
        event.prevent_default();
        #[cfg(feature = "hydrate")]
        if let Some(file) = event
            .data_transfer()
            .and_then(|transfer| transfer.files())
            .and_then(|files| files.get(0))
        {
            engine::adopt(session, file);
        }
    };

    view! {
        <div class="flex min-h-0 flex-1 flex-col gap-2">
            <input
                id="redact-file"
                type="file"
                accept=ACCEPT
                class="sr-only"
                on:change=on_change
            />
            <div class=FRAME on:dragover=on_over on:drop=on_drop>
                {move || match session.status.get() {
                    Status::Ready => view! { <Workbench session=session /> }.into_any(),
                    Status::Loading => view! { <Waiting /> }.into_any(),
                    Status::Failed(why) => view! { <Refused why=why /> }.into_any(),
                    Status::Empty => view! { <Dropzone /> }.into_any(),
                }}
            </div>
        </div>
    }
}

#[component]
fn Workbench(session: Session) -> impl IntoView {
    view! {
        <sidebar::Sidebar session=session />
        <div class="flex min-h-0 min-w-0 flex-1 flex-col">
            <viewer::Viewer session=session />
        </div>
    }
}

#[component]
fn Waiting() -> impl IntoView {
    view! {
        <div class="flex w-full flex-1 items-center justify-center p-10">
            <p class="text-sm text-[var(--color-ink-muted)]">"Lecture du document..."</p>
        </div>
    }
}

#[component]
fn Refused(why: String) -> impl IntoView {
    let on_pick = move |_| {
        #[cfg(feature = "hydrate")]
        engine::pick();
    };
    view! {
        <div class="flex w-full flex-1 flex-col items-center justify-center gap-4 p-10 text-center">
            <p class="font-sans text-base text-[var(--color-ink)]">
                "Ce document n'a pas été ouvert."
            </p>
            <p class="max-w-md text-sm leading-relaxed text-[var(--color-ink-muted)]">{why}</p>
            <button type="button" class=CHOOSE on:click=on_pick>
                "Choisir un autre fichier"
            </button>
        </div>
    }
}

#[component]
fn Dropzone() -> impl IntoView {
    let on_pick = move |_| {
        #[cfg(feature = "hydrate")]
        engine::pick();
    };
    view! {
        <div class="flex w-full flex-1 flex-col items-center justify-center gap-5 p-6 text-center sm:p-10">
            <svg
                viewBox="0 0 48 48"
                class="h-12 w-12 text-[var(--color-ink-subtle)]"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
            >
                <path d="M28 6 H14 a4 4 0 0 0 -4 4 v28 a4 4 0 0 0 4 4 h20 a4 4 0 0 0 4 -4 V16 Z" />
                <path d="M28 6 v10 h10" />
                <path d="M17 27 h14 v7 h-14 Z" fill="currentColor" stroke="none" />
            </svg>
            <div class="flex flex-col gap-1">
                <p class="font-sans text-lg text-[var(--color-ink)]">"Déposez un document ici"</p>
                <p class="text-sm text-[var(--color-ink-muted)]">
                    "PDF, Word, OpenDocument ou image."
                </p>
            </div>
            <button type="button" class=CHOOSE on:click=on_pick>
                "Choisir un fichier"
            </button>
            <p class="max-w-md text-xs leading-relaxed text-[var(--color-ink-subtle)]">
                "Le fichier "
                <strong class="font-semibold text-[var(--color-ink)]">"reste sur votre machine"</strong>
                " : il est lu et réécrit dans cette page, sans requête vers un serveur."
            </p>
        </div>
    }
}
