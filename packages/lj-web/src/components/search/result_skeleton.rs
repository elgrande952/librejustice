//! `ResultSkeleton` : cartes placeholder calquées sur la structure et les
//! hauteurs de `ResultCard`.

use leptos::prelude::*;

use crate::components::ui::Skeleton;

#[component]
pub fn ResultSkeleton() -> impl IntoView {
    view! {
        <div class="flex flex-col">
            {(0..5)
                .map(|_| {
                    view! {
                        <div class="grid grid-cols-[auto_1fr] gap-x-6 border-t border-[var(--color-rule)] py-7">
                            <Skeleton class="mt-0.5 h-7 w-9" />
                            <div class="flex flex-col gap-2">
                                <Skeleton class="h-7 w-11/12" />
                                <Skeleton class="h-7 w-2/3" />
                                <Skeleton class="h-7 w-44" />
                                <Skeleton class="h-6 w-full" />
                                <Skeleton class="h-6 w-5/6" />
                                <Skeleton class="h-6 w-40 self-end" />
                            </div>
                        </div>
                    }
                })
                .collect::<Vec<_>>()}
        </div>
    }
}
