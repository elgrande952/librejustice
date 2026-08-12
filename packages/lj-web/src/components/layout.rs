//! Gabarit 3 colonnes canonique — source unique des classes de grille
//! partagées par la page décision (`DecisionLayout`), `/decisions` et
//! `/textes` (corps ET skeletons). Les trois pages doivent tomber exactement
//! sur les mêmes pistes : rail gauche 240px, contenu `minmax(0,1fr)`, colonne
//! droite 280px, gouttière 48, dès `lg`. Toute évolution du gabarit se fait
//! ICI, jamais en dupliquant une chaîne de classes côté page.

/// Conteneur de page : centré, borné à 92rem, paddings standard.
pub const SHELL: &str = "mx-auto w-full max-w-[92rem] px-4 py-8 sm:px-6 lg:px-8";

/// Pistes de la grille `rail gauche | contenu | colonne droite`.
pub const GRID_3COL: &str =
    "grid grid-cols-1 gap-8 lg:grid-cols-[240px_minmax(0,1fr)_280px] lg:gap-12";

/// Pistes de la grille `rail gauche | contenu` des pages à rail seul
/// (catalogue d'un texte, sections, annuaire) : même gouttière 240px, le rail
/// et le contenu tombent au même x que sur le gabarit 3 colonnes.
pub const GRID_2COL: &str = "grid gap-8 lg:grid-cols-[240px_minmax(0,1fr)] lg:gap-12";

/// Aside latéral des pages de recherche : masqué sous `lg`, sticky sous la
/// barre, scrollable s'il dépasse le viewport. (La page décision garde ses
/// propres variantes : rails visibles sur mobile, empilés après le contenu.)
pub const SIDE_RAIL: &str = "hidden min-w-0 lg:sticky lg:top-20 lg:block lg:max-h-[calc(100dvh-6rem)] lg:self-start lg:overflow-y-auto";

/// Colonne contenu des pages de recherche : pleine piste, mesure de lecture
/// bornée à `max-w-3xl` en grille 3 colonnes seulement — sous `lg` (colonne
/// unique) le contenu s'étire sur toute la largeur.
pub const CONTENT_COL: &str = "flex w-full min-w-0 flex-col gap-6 lg:max-w-3xl";
