//! Plomberie DOM du scroll-spy du sommaire : mesure et écouteurs. La résolution
//! (section active + barre) reste pure, dans
//! `crate::pages::decision_page::toc_spy`.
#![cfg(feature = "hydrate")]

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;

use crate::pages::decision_page::toc_spy::SpyMetric;

/// Sous ce délai sans nouvel événement de scroll, un scroll programmé est
/// considéré terminé et le spy reprend la main.
const REPRISE_MS: i32 = 140;
/// Filet : une cible déjà visible ne déclenche aucun scroll, donc aucun
/// événement ne viendrait relâcher le gel.
const FILET_MS: i32 = 700;
/// Tolérance de détection du bas de page, en pixels.
const MARGE_BAS_PX: f64 = 2.0;

/// Mesure les ancres absolues et les centres d'items pour chaque id. Un id sans
/// ancre ou sans item est absent du résultat — c'est le filtre que le shim JS
/// exprimait en `NaN`.
fn mesurer(liste: &web_sys::Element, ids: &[String]) -> Vec<SpyMetric> {
    let Some((window, document)) = web_sys::window().and_then(|w| {
        let d = w.document()?;
        Some((w, d))
    }) else {
        return Vec::new();
    };
    let haut_liste = liste.get_bounding_client_rect().top();
    let scroll_y = window.scroll_y().unwrap_or(0.0);
    let items = liste.children();

    ids.iter()
        .enumerate()
        .filter_map(|(i, id)| {
            let ancre = document.get_element_by_id(id)?;
            let item = items.item(i as u32)?;
            let rect_item = item.get_bounding_client_rect();
            Some(SpyMetric {
                id: id.clone(),
                anchor_top: ancre.get_bounding_client_rect().top() + scroll_y,
                center: rect_item.top() - haut_liste + rect_item.height() / 2.0,
            })
        })
        .collect()
}

/// Hauteur de scroll maximale du document.
fn scroll_max(window: &web_sys::Window) -> f64 {
    let hauteur_doc = window
        .document()
        .and_then(|d| d.document_element())
        .map(|e| f64::from(e.scroll_height()))
        .unwrap_or(0.0);
    let hauteur_vue = window
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    hauteur_doc - hauteur_vue
}

/// Ligne de lecture et « on est en bas » pour une position de scroll donnée.
fn sonder(window: &web_sys::Window, scroll_y: f64, decalage: f64) -> (f64, bool) {
    let max = scroll_max(window);
    (
        scroll_y + decalage,
        max > 0.0 && scroll_y >= max - MARGE_BAS_PX,
    )
}

/// État partagé entre l'observateur et le clic : pendant un scroll programmé le
/// spy est gelé, et il reprend après un silence.
#[derive(Default)]
struct Gel {
    programme: bool,
    minuterie: Option<i32>,
}

impl Gel {
    fn annuler(&mut self, window: &web_sys::Window) {
        if let Some(id) = self.minuterie.take() {
            window.clear_timeout_with_handle(id);
        }
    }
}

/// Observateur branché : garde les écouteurs vivants et porte le gel que le clic
/// arme. Son `Drop` retire les écouteurs et annule la minuterie en cours.
pub struct SpyHandle {
    gel: Rc<RefCell<Gel>>,
    window: web_sys::Window,
    on_scroll: Closure<dyn FnMut()>,
    /// Gardé vivant pour la durée de l'observateur : la minuterie de reprise le
    /// rappelle après la fin d'un scroll programmé.
    _reprise: Rc<Closure<dyn FnMut()>>,
}

impl Drop for SpyHandle {
    fn drop(&mut self) {
        let f = self.on_scroll.as_ref().unchecked_ref();
        let _ = self.window.remove_event_listener_with_callback("scroll", f);
        let _ = self.window.remove_event_listener_with_callback("resize", f);
        self.gel.borrow_mut().annuler(&self.window);
    }
}

/// Branche le spy sur `scroll` (passif) et `resize`.
///
/// À chaque événement, mesure et appelle `on_resolve(ligne_de_lecture, métriques,
/// en_bas)`. Un scroll libre rappelle d'abord `on_animate_off` — le suivi est
/// alors 1:1, sans transition. Pendant un scroll programmé (après
/// [`scroll_to_section`]) le spy est gelé, et reprend après [`REPRISE_MS`] sans
/// nouvel événement.
pub fn observe_scroll_spy(
    liste: web_sys::Element,
    ids: Vec<String>,
    decalage: f64,
    on_resolve: impl Fn(f64, Vec<SpyMetric>, bool) + 'static,
    on_animate_off: impl Fn() + 'static,
) -> Option<SpyHandle> {
    let window = web_sys::window()?;
    let gel = Rc::new(RefCell::new(Gel::default()));

    let resoudre = {
        let liste = liste.clone();
        let ids = ids.clone();
        let window = window.clone();
        Rc::new(move || {
            let (ligne, en_bas) = sonder(&window, window.scroll_y().unwrap_or(0.0), decalage);
            on_resolve(ligne, mesurer(&liste, &ids), en_bas);
        })
    };

    // Fin du scroll programmé : on dégèle et on résout une dernière fois, à la
    // position d'arrivée.
    let reprise = {
        let gel = Rc::clone(&gel);
        let resoudre = Rc::clone(&resoudre);
        Rc::new(Closure::<dyn FnMut()>::new(move || {
            gel.borrow_mut().programme = false;
            gel.borrow_mut().minuterie = None;
            resoudre();
        }))
    };

    let on_scroll = {
        let gel = Rc::clone(&gel);
        let resoudre = Rc::clone(&resoudre);
        let reprise = Rc::clone(&reprise);
        let window = window.clone();
        Closure::<dyn FnMut()>::new(move || {
            if !gel.borrow().programme {
                on_animate_off();
                resoudre();
                return;
            }
            // Chaque événement repousse la reprise : le scroll lisse en émet
            // une rafale, et c'est son silence qui dit qu'il est arrivé.
            let mut g = gel.borrow_mut();
            g.annuler(&window);
            g.minuterie = window
                .set_timeout_with_callback_and_timeout_and_arguments_0(
                    reprise.as_ref().as_ref().unchecked_ref(),
                    REPRISE_MS,
                )
                .ok();
        })
    };

    let f = on_scroll.as_ref().unchecked_ref();
    let options = web_sys::AddEventListenerOptions::new();
    options.set_passive(true);
    let _ = window
        .add_event_listener_with_callback_and_add_event_listener_options("scroll", f, &options);
    let _ = window.add_event_listener_with_callback("resize", f);
    resoudre();

    Some(SpyHandle {
        gel,
        window,
        on_scroll,
        _reprise: reprise,
    })
}

/// Clic sur une entrée du sommaire.
///
/// Pose l'état EXACT du point d'atterrissage AVANT le scroll via
/// `on_landing(ligne_de_lecture, métriques, en_bas)` — même résolveur que le
/// spy, donc aucun saut —, met à jour le hash, puis scrolle jusqu'à la cible.
///
/// Le gel de `handle` évite que la rafale d'événements du scroll lisse ne
/// repeigne le sommaire au passage de chaque section.
pub fn scroll_to_section(
    handle: Option<&SpyHandle>,
    id: &str,
    liste: &web_sys::Element,
    ids: &[String],
    decalage: f64,
    marge_ancre: f64,
    on_landing: impl Fn(f64, Vec<SpyMetric>, bool),
) {
    let Some(window) = web_sys::window() else {
        return;
    };
    if let Some(handle) = handle {
        let mut gel = handle.gel.borrow_mut();
        gel.programme = true;
        gel.annuler(&window);
    }

    let metriques = mesurer(liste, ids);
    let max = scroll_max(&window);
    if let Some(cible) = metriques.iter().find(|m| m.id == id) {
        let arrivee = (cible.anchor_top - marge_ancre).clamp(0.0, max.max(0.0));
        let (ligne, en_bas) = sonder(&window, arrivee, decalage);
        on_landing(ligne, metriques, en_bas);
    }

    if window.location().hash().ok().as_deref() != Some(&format!("#{id}")) {
        if let Ok(histoire) = window.history() {
            let etat = histoire.state().unwrap_or(JsValue::NULL);
            let _ = histoire.push_state_with_url(&etat, "", Some(&format!("#{id}")));
        }
    }
    if let Some(ancre) = window.document().and_then(|d| d.get_element_by_id(id)) {
        let options = web_sys::ScrollIntoViewOptions::new();
        options.set_behavior(web_sys::ScrollBehavior::Smooth);
        options.set_block(web_sys::ScrollLogicalPosition::Start);
        ancre.scroll_into_view_with_scroll_into_view_options(&options);
    }

    // Filet : une cible déjà visible ne produit aucun scroll, donc rien ne
    // viendrait relâcher le gel.
    if let Some(handle) = handle {
        let gel_pour_filet = Rc::clone(&handle.gel);
        let filet = Closure::once_into_js(move || {
            let mut g = gel_pour_filet.borrow_mut();
            g.programme = false;
            g.minuterie = None;
        });
        let mut gel = handle.gel.borrow_mut();
        gel.annuler(&window);
        gel.minuterie = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(filet.unchecked_ref(), FILET_MS)
            .ok();
    }
}
