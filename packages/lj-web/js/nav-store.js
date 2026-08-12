// Shim `sessionStorage` : les features web-sys héritées de leptos n'exposent pas
// `Storage`. Bindé via wasm-bindgen (`crate::dom`), comme `js/clipboard.js`.
// Porte la graine de navigation décision (origine recherche + prev/next) d'un
// rechargement à l'autre, dans la portée d'un onglet.
//
// Pourquoi `sessionStorage` et pas `localStorage` : la graine est un contexte de
// navigation, pas une préférence. `localStorage` serait partagé par tous les
// onglets — deux recherches ouvertes se voleraient leur retour —, survivrait à la
// fermeture du navigateur (contexte périmé rouvert trois jours plus tard) et
// laisserait les termes de recherche sur le disque.
//
// `sessionStorage` survit au F5 et n'est pas
// recopié dans un onglet ouvert au clic milieu ni par `target="_blank"` — seul
// `window.open` le recopie. Un nouvel onglet repart donc sans graine, sans
// conséquence : l'onglet de la liste reste ouvert derrière, c'est lui le retour.
//
// L'accès jette quand le stockage est refusé (mode privé strict, cookies
// bloqués) et quand le quota est plein : on rend `null` / on ne fait rien, la
// graine en mémoire tient alors jusqu'au rechargement, comme avant ce shim.
const key = (id) => `lj-nav:${id}`;

export function navSeedGet(id) {
  try {
    return sessionStorage.getItem(key(id));
  } catch {
    return null;
  }
}

export function navSeedSet(id, json) {
  try {
    sessionStorage.setItem(key(id), json);
  } catch {
    // Stockage indisponible : rien à rattraper, la mémoire vive prend le relais.
  }
}
