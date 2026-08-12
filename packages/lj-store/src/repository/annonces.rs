//! Fond marchés du pot Annonces : upserts, cycle de vie, rattachement au graphe
//! des entités, recensement des profils d'acheteurs.
//!
//! Écriture en masse par `unnest` : chaque run ré-énumère le stock complet, donc
//! l'écrasante majorité des lignes existent déjà. Aucun état de cycle de vie
//! n'est stampé : on enregistre le flux, la fenêtre « ouverte » se calcule.

use tokio_postgres::types::{Json, ToSql};

use crate::error::Result;
use crate::repository::DecisionRepository;

/// Une ligne canonique prête à écrire, avec ses canaux et ses arêtes.
#[derive(Debug)]
pub struct AnnonceWriteItem {
    pub uid: String,
    /// Fond du pot (`marche`…) — vocabulaire de `lj_dtos::ANNONCE_FOND_*`.
    pub fond: String,
    pub famille_uid: Option<String>,
    pub titre: String,
    pub acteur_libelle: String,
    pub departement: Option<String>,
    pub cpv_uid: Option<String>,
    pub date_publication: chrono::NaiveDate,
    pub echeance: Option<chrono::DateTime<chrono::Utc>>,
    pub url_action: String,
    pub details: serde_json::Value,
    /// Arêtes vers le graphe par SIREN donné à la source (eForms BOAMP), avec
    /// leur rôle : `acheteur` sur une consultation, `titulaire` sur une
    /// attribution.
    pub aretes_siren: Vec<(String, String)>,
    pub sources: Vec<AnnonceSourceWriteItem>,
}

/// Un canal de publication d'une annonce.
#[derive(Debug)]
pub struct AnnonceSourceWriteItem {
    pub source_uid: String,
    pub url_avis: Option<String>,
}

/// Une ligne sans canal à régulariser : ses champs d'identification et les
/// URLs de ses liens, qui désignent ses cibles possibles.
#[derive(Debug)]
pub struct AnnonceOrpheline {
    pub uid: String,
    pub titre: String,
    pub acteur_libelle: String,
    pub echeance: Option<chrono::DateTime<chrono::Utc>>,
    pub urls: Vec<String>,
}

/// Les champs d'identification d'une ligne, pour départager des cibles.
#[derive(Debug)]
pub struct AnnonceChamps {
    pub uid: String,
    pub titre: String,
    pub acteur_libelle: String,
    pub echeance: Option<chrono::DateTime<chrono::Utc>>,
    /// L'entité acheteur de la ligne, quand elle en porte une seule — deux
    /// arêtes `acheteur` ne désignent personne.
    pub acheteur_uid: Option<String>,
}

/// Une ligne ouverte que seul le BOAMP publie, avec la plateforme de dépôt que
/// son avis déclare et les URLs de ses liens : la matière de `annonces-jumeler`.
#[derive(Debug)]
pub struct AnnonceAJumeler {
    pub uid: String,
    pub titre: String,
    pub acteur_libelle: String,
    pub echeance: Option<chrono::DateTime<chrono::Utc>>,
    /// `details.depot` — l'URL de la plateforme, sans identifiant de dossier.
    pub depot: String,
    pub urls: Vec<String>,
    /// Voir [`AnnonceChamps::acheteur_uid`].
    pub acheteur_uid: Option<String>,
}

/// Un canal de plateforme d'une annonce ouverte, avec les champs
/// d'identification de sa ligne.
#[derive(Debug)]
pub struct AnnonceCanalOuvert {
    pub source_uid: String,
    pub annonce: AnnonceChamps,
}

/// L'attribution à poser sur un marché déjà en stock. L'avis de résultat n'a
/// pas de ligne à lui : il devient un canal du marché qu'il clôt, et ses
/// titulaires deviennent des arêtes de ce marché.
#[derive(Debug)]
pub struct ResultatPose {
    pub annonce_uid: String,
    pub resultat: serde_json::Value,
    pub source: AnnonceSourceWriteItem,
    pub aretes_siren: Vec<(String, String)>,
}

/// Une ligne de la file d'enrichissement : où aller chercher la page de détail
/// de cette annonce, et par quel moteur la lire.
#[derive(Debug)]
pub struct AnnonceAEnrichir {
    pub uid: String,
    pub moteur: String,
    pub url_avis: String,
}

/// Ce qu'une page de détail lue apprend sur une annonce. `detail` est le bloc
/// jsonb à greffer — vide quand la page n'a rien donné, ce qui vaut quand même
/// « déjà visitée ».
#[derive(Debug)]
pub struct DetailPose {
    pub annonce_uid: String,
    pub cpv_uid: Option<String>,
    pub detail: serde_json::Value,
    /// Acheteur lu sur la fiche. Ne comble qu'un libellé vide.
    pub acheteur: Option<String>,
}

/// Récapitulatif d'un run d'écriture.
#[derive(Debug, Default)]
pub struct AnnonceWriteStats {
    pub annonces: u64,
    pub canaux: u64,
    pub aretes: u64,
}

/// Taille d'une table, index et TOAST compris.
#[derive(Debug)]
pub struct TableSize {
    pub table: String,
    pub bytes: i64,
}

// ── Lecture : la fenêtre servie par `/annonces` ─────────────────────────────

/// Ordre de lecture. `Nouveautes` trie par première apparition dans nos canaux
/// (`min(annonce_source.first_seen_at)`) : les moteurs scrapés n'ont pas de
/// curseur de publication fiable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnonceOrdre {
    Echeance,
    Publication,
    Nouveautes,
}

/// Filtre d'une lecture du fond. Les uids sont complets
/// (`nature_marche:TRAVAUX`, `cpv:45`) — la traduction depuis les suffixes de
/// l'API se fait à la frontière HTTP.
#[derive(Debug, Clone)]
pub struct AnnonceFiltre {
    pub fond: String,
    /// Filtre par mot sur le titre et l'acheteur, pas une recherche plein texte :
    /// le fond n'a pas d'index texte. Le motif est borné par mot
    /// (cf. [`motif_mot`]).
    pub q: Option<String>,
    pub departements: Vec<String>,
    pub familles: Vec<String>,
    pub cpv: Vec<String>,
    /// Fenêtre d'échéance : ne garde que `echeance <= now() + N jours`.
    pub echeance_jours: Option<i32>,
    /// Inclut les lignes closes (défaut : la fenêtre vivante seule).
    pub closes: bool,
    pub ordre: AnnonceOrdre,
    pub limit: i64,
    pub offset: i64,
}

impl AnnonceFiltre {
    pub fn nouveau(fond: &str) -> Self {
        Self {
            fond: fond.to_string(),
            q: None,
            departements: Vec::new(),
            familles: Vec::new(),
            cpv: Vec::new(),
            echeance_jours: None,
            closes: false,
            ordre: AnnonceOrdre::Echeance,
            limit: 20,
            offset: 0,
        }
    }
}

/// Une annonce lue, libellés de référentiel résolus.
#[derive(Debug)]
pub struct AnnonceRow {
    pub uid: String,
    pub fond: String,
    pub famille_uid: Option<String>,
    pub famille_label: Option<String>,
    pub titre: String,
    pub acteur_libelle: String,
    pub departement: Option<String>,
    pub cpv_uid: Option<String>,
    pub cpv_label: Option<String>,
    pub date_publication: chrono::NaiveDate,
    pub echeance: Option<chrono::DateTime<chrono::Utc>>,
    pub url_action: String,
    pub details: serde_json::Value,
    /// Moteurs qui publient l'annonce, lus dans le préfixe des `source_uid`.
    pub canaux: Vec<String>,
    /// Première apparition dans nos canaux — le curseur des alertes.
    pub premiere_vue: Option<chrono::DateTime<chrono::Utc>>,
}

/// Un canal de publication tel que servi par la fiche.
#[derive(Debug)]
pub struct AnnonceCanalRow {
    pub source_uid: String,
    pub url_avis: Option<String>,
    pub first_seen_at: chrono::DateTime<chrono::Utc>,
}

/// Une arête vers le graphe des entités, dénomination résolue quand l'entité
/// existe encore : les référentiels se rechargent par remplacement de namespace,
/// donc l'arête peut pointer dans le vide.
#[derive(Debug)]
pub struct AnnonceEntiteRow {
    pub entity_uid: String,
    pub role: String,
    pub denomination: Option<String>,
}

/// Une annonce d'une entité, avec le rôle de l'arête et le total du rôle.
#[derive(Debug)]
pub struct EntityAnnonceRow {
    pub role: String,
    pub total_role: i64,
    pub annonce: AnnonceRow,
}

/// Les clés de `details` dont la moisson n'est pas la source : posées par un
/// autre flux (page de détail, avis d'attribution) qu'elle ne reverra jamais.
/// Re-greffées sur le jsonb du batch, elles survivent à la ré-énumération
/// nocturne ; léguées par une ligne absorbée, elles survivent à la fusion. Tout
/// le reste de `details` appartient au moteur, qui l'écrase.
const DETAILS_PRESERVES: &str = "ARRAY['detail', 'resultat']";

/// `details` d'une ligne revue : ce que le batch propose, les clés préservées
/// du stock re-greffées par-dessus. Sert aussi de terme droit au garde
/// write-once.
fn details_greffe() -> String {
    format!(
        "EXCLUDED.details || COALESCE((SELECT jsonb_object_agg(k, annonce.details -> k) \
         FROM unnest({DETAILS_PRESERVES}) AS k WHERE annonce.details ? k), '{{}}'::jsonb)"
    )
}

/// Ce qu'une ligne absorbée lègue à l'élue : les clés préservées que `source`
/// porte et que `cible` n'a pas. Au grain de la clé, et la cible gagne quand les
/// deux la portent — l'élue est la ligne vivante, celle que le batch vient
/// d'écrire.
///
/// Fragment `FROM … WHERE …` : il sert de corps à l'agrégat du legs et à la
/// garde `EXISTS` qui borne l'UPDATE aux lignes qui gagnent quelque chose —
/// `jsonb_object_agg` d'un ensemble vide rend NULL, et le `||` effacerait le
/// `details` de l'élue.
fn details_leguees(source: &str, cible: &str) -> String {
    format!(
        "FROM unnest({DETAILS_PRESERVES}) AS k \
         WHERE {source}.details ? k AND NOT {cible}.details ? k"
    )
}

/// Le prédicat « cette annonce est ouverte », dit une seule fois pour tout le
/// fond. Entièrement calculé — on enregistre le flux, on ne stampe aucun état :
///
/// - `echeance > NOW()` — le délai de dépôt court encore. Une annonce sans
///   échéance n'entre jamais dans la fenêtre : on ne sait pas borner son dépôt,
///   on ne prétend pas qu'il est ouvert.
/// - au moins un canal la porte — la réconciliation re-parente ses
///   `annonce_source` quand elle revoit un groupage. L'uid abandonné garde sa
///   page et son archive, mais plus aucun moteur ne publie ce point d'entrée.
fn ouverte(alias: &str) -> String {
    format!(
        "{alias}.echeance > NOW() \
         AND EXISTS (SELECT 1 FROM annonce_source s WHERE s.annonce_uid = {alias}.uid)"
    )
}

/// L'entité acheteur d'une ligne, quand elle en porte une seule. L'arête vient
/// du SIRET déclaré par la source ou de la dénomination pliée résolue sans
/// ambiguïté (`relier_annonces_par_denomination`) ; deux arêtes `acheteur` sur la
/// même ligne ne désignent personne, le `HAVING` les efface.
fn acheteur_unique(alias: &str) -> String {
    format!(
        "(SELECT max(ae.entity_uid) FROM annonce_entity ae \
          WHERE ae.annonce_uid = {alias}.uid AND ae.role = 'acheteur' \
          HAVING count(*) = 1)"
    )
}

/// Colonnes projetées par toute lecture d'annonce, libellés joints. `canaux`
/// vient d'un LATERAL sur `annonce_source` : un aller-retour par page, pas par
/// ligne.
const ANNONCE_COLS: &str = "
    a.uid, a.fond, a.famille_uid, ff.label AS famille_label, a.titre,
    a.acteur_libelle, a.departement, a.cpv_uid, fc.label AS cpv_label,
    a.date_publication, a.echeance, a.url_action, a.details,
    COALESCE(c.canaux, ARRAY[]::text[]) AS canaux, c.premiere_vue";

const ANNONCE_JOINS: &str = "
    LEFT JOIN facet_value ff ON ff.uid = a.famille_uid
    LEFT JOIN facet_value fc ON fc.uid = a.cpv_uid
    LEFT JOIN LATERAL (
      SELECT array_agg(DISTINCT split_part(s.source_uid, ':', 1)) AS canaux,
             min(s.first_seen_at) AS premiere_vue
      FROM annonce_source s WHERE s.annonce_uid = a.uid
    ) c ON true";

fn annonce_from_row(row: &tokio_postgres::Row) -> AnnonceRow {
    AnnonceRow {
        uid: row.get("uid"),
        fond: row.get("fond"),
        famille_uid: row.get("famille_uid"),
        famille_label: row.get("famille_label"),
        titre: row.get("titre"),
        acteur_libelle: row.get("acteur_libelle"),
        departement: row.get("departement"),
        cpv_uid: row.get("cpv_uid"),
        cpv_label: row.get("cpv_label"),
        date_publication: row.get("date_publication"),
        echeance: row.get("echeance"),
        url_action: row.get("url_action"),
        details: row.get("details"),
        canaux: row.get("canaux"),
        premiere_vue: row.get("premiere_vue"),
    }
}

/// Axe de facette du fond.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnonceAxe {
    Departement,
    Famille,
    Cpv,
}

impl AnnonceAxe {
    pub fn nom(self) -> &'static str {
        match self {
            AnnonceAxe::Departement => "departement",
            AnnonceAxe::Famille => "famille",
            AnnonceAxe::Cpv => "cpv",
        }
    }

    fn colonne(self) -> &'static str {
        match self {
            AnnonceAxe::Departement => "a.departement",
            AnnonceAxe::Famille => "a.famille_uid",
            AnnonceAxe::Cpv => "a.cpv_uid",
        }
    }
}

/// Motif POSIX borné par mot pour le filtre `q` : la sous-chaîne nue ramenait
/// le mot qui contient le motif, `PLU` sortait « pluvial ». Les bornes
/// `\m`/`\M` ne sont posées que si le motif commence / finit par un caractère
/// de mot — sinon elles ne matcheraient jamais.
///
/// L'entrée est du texte tapé : tout ce qui n'est ni alphanumérique ni espace
/// est échappé, faute de quoi `q=C++` serait lu comme un quantificateur.
fn motif_mot(q: &str) -> String {
    let mut motif = String::with_capacity(q.len() + 4);
    if q.starts_with(char::is_alphanumeric) {
        motif.push_str("\\m");
    }
    for c in q.chars() {
        if !c.is_alphanumeric() && c != ' ' {
            motif.push('\\');
        }
        motif.push(c);
    }
    if q.ends_with(char::is_alphanumeric) {
        motif.push_str("\\M");
    }
    motif
}

/// Accumulateur de paramètres : `$1` est toujours le fond, les filtres suivent
/// dans l'ordre où ils sont posés.
struct Filtre {
    /// `Send` requis : ces paramètres traversent un `await` dans un handler
    /// axum, dont la future doit rester `Send`.
    params: Vec<Box<dyn ToSql + Sync + Send>>,
    clauses: String,
}

impl Filtre {
    /// Construit les clauses `AND …` du filtre. `sauf` retire un axe du
    /// prédicat — c'est ce qui rend la multi-sélection d'une facette possible :
    /// son propre compte se calcule sans elle.
    fn new(f: &AnnonceFiltre, sauf: Option<AnnonceAxe>) -> Self {
        Self::construire(f, sauf, true)
    }

    /// Variante sans la fenêtre d'échéance — le prédicat des comptes de fenêtres,
    /// qui ne peut pas se compter à travers lui-même.
    fn sans_fenetre(f: &AnnonceFiltre) -> Self {
        Self::construire(f, None, false)
    }

    fn construire(f: &AnnonceFiltre, sauf: Option<AnnonceAxe>, fenetre: bool) -> Self {
        let mut me = Filtre {
            params: vec![Box::new(f.fond.clone())],
            clauses: String::new(),
        };
        if !f.closes {
            me.clauses.push_str(&format!(" AND {}", ouverte("a")));
        }
        if let Some(q) = f.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
            let ph = me.push(Box::new(motif_mot(q)));
            me.clauses.push_str(&format!(
                " AND (a.titre ~* ${ph} OR a.acteur_libelle ~* ${ph})"
            ));
        }
        for (axe, valeurs) in [
            (AnnonceAxe::Departement, &f.departements),
            (AnnonceAxe::Famille, &f.familles),
            (AnnonceAxe::Cpv, &f.cpv),
        ] {
            if valeurs.is_empty() || sauf == Some(axe) {
                continue;
            }
            let colonne = axe.colonne();
            let ph = me.push(Box::new(valeurs.clone()));
            me.clauses
                .push_str(&format!(" AND {colonne} = ANY(${ph}::text[])"));
        }
        if let Some(jours) = f.echeance_jours.filter(|_| fenetre) {
            let ph = me.push(Box::new(jours));
            me.clauses.push_str(&format!(
                " AND a.echeance IS NOT NULL \
                  AND a.echeance <= NOW() + make_interval(days => ${ph})"
            ));
        }
        me
    }

    fn push(&mut self, value: Box<dyn ToSql + Sync + Send>) -> usize {
        self.params.push(value);
        self.params.len()
    }

    fn refs(&self) -> Vec<&(dyn ToSql + Sync)> {
        self.params
            .iter()
            .map(|b| b.as_ref() as &(dyn ToSql + Sync))
            .collect()
    }
}

/// Fragment `ORDER BY`. L'échéance croissante est l'ordre du produit ; elle
/// emprunte l'index `annonce_fond_echeance_idx`.
fn order_by(ordre: AnnonceOrdre) -> &'static str {
    match ordre {
        AnnonceOrdre::Echeance => "a.echeance ASC NULLS LAST, a.uid",
        AnnonceOrdre::Publication => "a.date_publication DESC, a.uid",
        AnnonceOrdre::Nouveautes => "c.premiere_vue DESC NULLS LAST, a.uid",
    }
}

impl DecisionRepository<'_> {
    /// Upsert d'un lot d'annonces canoniques et de leurs canaux, write-once : une
    /// ligne revue identique n'est pas réécrite. `first_seen_at` ne bouge jamais
    /// (c'est le curseur des alertes). Les compteurs rendus ne comptent donc que le
    /// nouveau et le modifié.
    ///
    /// Le batch n'écrase pas ce dont il n'est pas la source : un `cpv_uid` absent
    /// laisse celui du stock, et les clés [`DETAILS_PRESERVES`] de `details` sont
    /// re-greffées.
    #[tracing::instrument(name = "db.upsert_annonces", skip(self, items), fields(db.system = "postgresql", n = items.len()))]
    pub async fn upsert_annonces(&self, items: &[AnnonceWriteItem]) -> Result<AnnonceWriteStats> {
        if items.is_empty() {
            return Ok(AnnonceWriteStats::default());
        }
        let uid: Vec<&str> = items.iter().map(|i| i.uid.as_str()).collect();
        let fond: Vec<&str> = items.iter().map(|i| i.fond.as_str()).collect();
        let famille: Vec<Option<&str>> = items.iter().map(|i| i.famille_uid.as_deref()).collect();
        let titre: Vec<&str> = items.iter().map(|i| i.titre.as_str()).collect();
        let acteur: Vec<&str> = items.iter().map(|i| i.acteur_libelle.as_str()).collect();
        let dept: Vec<Option<&str>> = items.iter().map(|i| i.departement.as_deref()).collect();
        let cpv: Vec<Option<&str>> = items.iter().map(|i| i.cpv_uid.as_deref()).collect();
        let pub_: Vec<chrono::NaiveDate> = items.iter().map(|i| i.date_publication).collect();
        let ech: Vec<Option<chrono::DateTime<chrono::Utc>>> =
            items.iter().map(|i| i.echeance).collect();
        let url: Vec<&str> = items.iter().map(|i| i.url_action.as_str()).collect();
        let details: Vec<Json<&serde_json::Value>> =
            items.iter().map(|i| Json(&i.details)).collect();

        let greffe = details_greffe();
        // Une liste muette n'efface pas l'acheteur que la fiche a nommé : sur Atexo,
        // l'API ne le porte pas et la fiche oui.
        let acheteur = "COALESCE(NULLIF(EXCLUDED.acteur_libelle, ''), annonce.acteur_libelle)";
        let annonces = self
            .conn
            .execute(
                &format!(
                    "
                INSERT INTO annonce
                  (uid, fond, famille_uid, titre, acteur_libelle, departement, cpv_uid,
                   date_publication, echeance, url_action, details)
                SELECT u, fo, f, t, a, d, c, p, e, l, j
                FROM unnest($1::text[], $2::text[], $3::text[], $4::text[], $5::text[],
                            $6::text[], $7::text[], $8::date[], $9::timestamptz[],
                            $10::text[], $11::jsonb[])
                     AS s(u, fo, f, t, a, d, c, p, e, l, j)
                ON CONFLICT (uid) DO UPDATE SET
                  famille_uid = EXCLUDED.famille_uid,
                  titre = EXCLUDED.titre,
                  acteur_libelle = {acheteur},
                  departement = EXCLUDED.departement,
                  cpv_uid = COALESCE(EXCLUDED.cpv_uid, annonce.cpv_uid),
                  date_publication = LEAST(annonce.date_publication, EXCLUDED.date_publication),
                  echeance = EXCLUDED.echeance,
                  url_action = EXCLUDED.url_action,
                  details = {greffe}
                WHERE (annonce.famille_uid, annonce.titre, annonce.acteur_libelle,
                       annonce.departement, annonce.cpv_uid, annonce.echeance,
                       annonce.url_action, annonce.details)
                      IS DISTINCT FROM
                      (EXCLUDED.famille_uid, EXCLUDED.titre, {acheteur},
                       EXCLUDED.departement, COALESCE(EXCLUDED.cpv_uid, annonce.cpv_uid),
                       EXCLUDED.echeance, EXCLUDED.url_action, {greffe})
                   OR EXCLUDED.date_publication < annonce.date_publication
                "
                ),
                &[
                    &uid, &fond, &famille, &titre, &acteur, &dept, &cpv, &pub_, &ech, &url,
                    &details,
                ],
            )
            .await?;

        let mut s_uid: Vec<&str> = Vec::new();
        let mut s_annonce: Vec<&str> = Vec::new();
        let mut s_url: Vec<Option<&str>> = Vec::new();
        for item in items {
            for src in &item.sources {
                s_uid.push(&src.source_uid);
                s_annonce.push(&item.uid);
                s_url.push(src.url_avis.as_deref());
            }
        }
        let canaux = self.ecrire_canaux(&s_uid, &s_annonce, &s_url).await?;

        let mut e_annonce: Vec<&str> = Vec::new();
        let mut e_entity: Vec<String> = Vec::new();
        let mut e_role: Vec<&str> = Vec::new();
        for item in items {
            for (siren, role) in &item.aretes_siren {
                e_annonce.push(&item.uid);
                e_entity.push(format!("siren:{siren}"));
                e_role.push(role);
            }
        }
        let aretes = self.ecrire_aretes(&e_annonce, &e_entity, &e_role).await?;

        Ok(AnnonceWriteStats {
            annonces,
            canaux,
            aretes,
        })
    }

    /// Rattache des `source_uid` à leur annonce. Partagé par la création d'une
    /// ligne et par la pose d'un résultat sur une ligne existante : un avis de
    /// résultat est un canal de plus du marché qu'il clôt.
    async fn ecrire_canaux(
        &self,
        source_uid: &[&str],
        annonce_uid: &[&str],
        url_avis: &[Option<&str>],
    ) -> Result<u64> {
        if source_uid.is_empty() {
            return Ok(0);
        }
        Ok(self
            .conn
            .execute(
                "
                INSERT INTO annonce_source
                  (source_uid, annonce_uid, url_avis, first_seen_at)
                SELECT s, a, u, NOW()
                FROM unnest($1::text[], $2::text[], $3::text[]) AS t(s, a, u)
                ON CONFLICT (source_uid) DO UPDATE SET
                  annonce_uid = EXCLUDED.annonce_uid,
                  url_avis = EXCLUDED.url_avis
                WHERE (annonce_source.annonce_uid, annonce_source.url_avis)
                      IS DISTINCT FROM (EXCLUDED.annonce_uid, EXCLUDED.url_avis)
                ",
                &[&source_uid, &annonce_uid, &url_avis],
            )
            .await?)
    }

    /// Arêtes par SIREN donné à la source (eForms BOAMP).
    async fn ecrire_aretes(
        &self,
        annonce_uid: &[&str],
        entity_uid: &[String],
        role: &[&str],
    ) -> Result<u64> {
        if annonce_uid.is_empty() {
            return Ok(0);
        }
        Ok(self
            .conn
            .execute(
                "
                INSERT INTO annonce_entity (annonce_uid, entity_uid, role)
                SELECT a, e, r
                FROM unnest($1::text[], $2::text[], $3::text[]) AS t(a, e, r)
                ON CONFLICT DO NOTHING
                ",
                &[&annonce_uid, &entity_uid, &role],
            )
            .await?)
    }

    /// Résout des canaux vers leur annonce : `source_uid` → `annonce_uid`. Le
    /// rattachement d'un résultat de marché à sa consultation : le `annonce_lie`
    /// BOAMP est un `source_uid`, la ligne canonique est derrière.
    #[tracing::instrument(name = "db.annonces_par_source", skip(self, source_uids), fields(db.system = "postgresql", n = source_uids.len()))]
    pub async fn annonces_par_source(
        &self,
        source_uids: &[String],
    ) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid, annonce_uid FROM annonce_source \
                 WHERE source_uid = ANY($1::text[])",
                &[&source_uids],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// La cible de redirection d'un uid absorbé. `None` = uid vivant ou inconnu.
    #[tracing::instrument(name = "db.redirection_annonce", skip(self), fields(db.system = "postgresql"))]
    pub async fn redirection_annonce(&self, uid: &str) -> Result<Option<String>> {
        let row = self
            .conn
            .query_opt(
                "SELECT nouvel_uid FROM annonce_redirect WHERE ancien_uid = $1",
                &[&uid],
            )
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    /// Applique des fusions `(ancien, nouveau)` décidées par la réconciliation :
    /// re-parente les canaux et les arêtes de l'absorbée, lui fait léguer ses clés
    /// [`DETAILS_PRESERVES`] à l'élue, pose la redirection — en re-pointant les
    /// chaînes qui visaient l'absorbée — et supprime la ligne. Transactionnel : une
    /// fusion à moitié appliquée referait une orpheline.
    #[tracing::instrument(name = "db.fusionner_annonces", skip(self, fusions), fields(db.system = "postgresql", n = fusions.len()))]
    pub async fn fusionner_annonces(&self, fusions: &[(String, String)]) -> Result<u64> {
        if fusions.is_empty() {
            return Ok(0);
        }
        let ancien: Vec<&str> = fusions.iter().map(|(a, _)| a.as_str()).collect();
        let nouveau: Vec<&str> = fusions.iter().map(|(_, n)| n.as_str()).collect();
        self.conn.batch_execute("BEGIN").await?;
        let applique = async {
            self.conn
                .execute(
                    "UPDATE annonce_source s SET annonce_uid = t.n \
                     FROM unnest($1::text[], $2::text[]) AS t(a, n) \
                     WHERE s.annonce_uid = t.a",
                    &[&ancien, &nouveau],
                )
                .await?;
            self.conn
                .execute(
                    "INSERT INTO annonce_entity (annonce_uid, entity_uid, role) \
                     SELECT t.n, e.entity_uid, e.role \
                     FROM annonce_entity e \
                     JOIN unnest($1::text[], $2::text[]) AS t(a, n) ON e.annonce_uid = t.a \
                     ON CONFLICT DO NOTHING",
                    &[&ancien, &nouveau],
                )
                .await?;
            self.conn
                .execute(
                    "DELETE FROM annonce_entity e \
                     USING unnest($1::text[]) AS t(a) WHERE e.annonce_uid = t.a",
                    &[&ancien],
                )
                .await?;
            let legs = details_leguees("a", "n");
            self.conn
                .execute(
                    &format!(
                        "UPDATE annonce n \
                         SET details = n.details \
                             || (SELECT jsonb_object_agg(k, a.details -> k) {legs}) \
                         FROM annonce a, unnest($1::text[], $2::text[]) AS t(a_uid, n_uid) \
                         WHERE n.uid = t.n_uid AND a.uid = t.a_uid \
                           AND EXISTS (SELECT 1 {legs})"
                    ),
                    &[&ancien, &nouveau],
                )
                .await?;
            self.conn
                .execute(
                    "UPDATE annonce_redirect r SET nouvel_uid = t.n \
                     FROM unnest($1::text[], $2::text[]) AS t(a, n) \
                     WHERE r.nouvel_uid = t.a",
                    &[&ancien, &nouveau],
                )
                .await?;
            self.conn
                .execute(
                    "INSERT INTO annonce_redirect (ancien_uid, nouvel_uid) \
                     SELECT t.a, t.n FROM unnest($1::text[], $2::text[]) AS t(a, n) \
                     ON CONFLICT (ancien_uid) DO UPDATE SET nouvel_uid = EXCLUDED.nouvel_uid",
                    &[&ancien, &nouveau],
                )
                .await?;
            self.conn
                .execute(
                    "DELETE FROM annonce WHERE uid = ANY($1::text[])",
                    &[&ancien],
                )
                .await
        }
        .await;
        match applique {
            Ok(supprimees) => {
                self.conn.batch_execute("COMMIT").await?;
                Ok(supprimees)
            }
            Err(e) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
                Err(e.into())
            }
        }
    }

    /// Les lignes d'un fond sans plus aucun canal, avec les URLs de leurs
    /// `details.liens` : la matière de `annonces-regulariser`.
    #[tracing::instrument(name = "db.annonces_orphelines", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonces_orphelines(&self, fond: &str) -> Result<Vec<AnnonceOrpheline>> {
        let rows = self
            .conn
            .query(
                "SELECT a.uid, a.titre, a.acteur_libelle, a.echeance, \
                        COALESCE(array_agg(u.url) FILTER (WHERE u.url IS NOT NULL), '{}') \
                 FROM annonce a \
                 LEFT JOIN LATERAL ( \
                   SELECT unnest(ARRAY[el->>'avis', el->>'rc', el->>'dce']) AS url \
                   FROM jsonb_array_elements(a.details->'liens') el \
                 ) u ON true \
                 WHERE a.fond = $1 \
                   AND NOT EXISTS (SELECT 1 FROM annonce_source s WHERE s.annonce_uid = a.uid) \
                 GROUP BY a.uid",
                &[&fond],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| AnnonceOrpheline {
                uid: r.get(0),
                titre: r.get(1),
                acteur_libelle: r.get(2),
                echeance: r.get(3),
                urls: r.get(4),
            })
            .collect())
    }

    /// Titre et libellé d'acheteur de tout le pot — assiette de la re-normalisation
    /// rétroactive de la régularisation : une règle de forme nouvelle s'applique au
    /// stock clos, que la moisson ne réécrit plus.
    #[tracing::instrument(name = "db.formes_annonces", skip(self), fields(db.system = "postgresql"))]
    pub async fn formes_annonces(&self) -> Result<Vec<(String, String, String)>> {
        let rows = self
            .conn
            .query("SELECT uid, titre, acteur_libelle FROM annonce", &[])
            .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    #[tracing::instrument(name = "db.corriger_formes", skip(self, corrections), fields(db.system = "postgresql", n = corrections.len()))]
    pub async fn corriger_formes(&self, corrections: &[(String, String, String)]) -> Result<u64> {
        if corrections.is_empty() {
            return Ok(0);
        }
        let uid: Vec<&str> = corrections.iter().map(|(u, _, _)| u.as_str()).collect();
        let titre: Vec<&str> = corrections.iter().map(|(_, t, _)| t.as_str()).collect();
        let acteur: Vec<&str> = corrections.iter().map(|(_, _, a)| a.as_str()).collect();
        Ok(self
            .conn
            .execute(
                "UPDATE annonce a SET titre = t.titre, acteur_libelle = t.acteur \
                 FROM unnest($1::text[], $2::text[], $3::text[]) AS t(uid, titre, acteur) \
                 WHERE a.uid = t.uid",
                &[&uid, &titre, &acteur],
            )
            .await?)
    }

    /// Les blocs `details.resultat` du pot — assiette de la re-normalisation
    /// rétroactive des lots d'attribution.
    #[tracing::instrument(name = "db.resultats_annonces", skip(self), fields(db.system = "postgresql"))]
    pub async fn resultats_annonces(&self) -> Result<Vec<(String, serde_json::Value)>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, details->'resultat' FROM annonce WHERE details ? 'resultat'",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    #[tracing::instrument(name = "db.corriger_resultats", skip(self, corrections), fields(db.system = "postgresql", n = corrections.len()))]
    pub async fn corriger_resultats(
        &self,
        corrections: &[(String, serde_json::Value)],
    ) -> Result<u64> {
        if corrections.is_empty() {
            return Ok(0);
        }
        let uid: Vec<&str> = corrections.iter().map(|(u, _)| u.as_str()).collect();
        let resultat: Vec<&serde_json::Value> = corrections.iter().map(|(_, v)| v).collect();
        Ok(self
            .conn
            .execute(
                "UPDATE annonce a SET details = jsonb_set(a.details, '{resultat}', t.resultat) \
                 FROM unnest($1::text[], $2::jsonb[]) AS t(uid, resultat) \
                 WHERE a.uid = t.uid",
                &[&uid, &resultat],
            )
            .await?)
    }

    /// Les champs d'identification d'un lot de lignes — pour départager les
    /// cibles d'une orpheline lors de la régularisation.
    #[tracing::instrument(name = "db.annonces_champs", skip(self, uids), fields(db.system = "postgresql", n = uids.len()))]
    pub async fn annonces_champs(&self, uids: &[String]) -> Result<Vec<AnnonceChamps>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT a.uid, a.titre, a.acteur_libelle, a.echeance, {} \
                     FROM annonce a WHERE a.uid = ANY($1::text[])",
                    acheteur_unique("a")
                ),
                &[&uids],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| AnnonceChamps {
                uid: r.get(0),
                titre: r.get(1),
                acteur_libelle: r.get(2),
                echeance: r.get(3),
                acheteur_uid: r.get(4),
            })
            .collect())
    }

    /// Résout des URLs de canal vers leur annonce (`url_avis` exact). Pas
    /// d'index sur la colonne : réservé au one-shot de régularisation.
    #[tracing::instrument(name = "db.annonces_par_url_avis", skip(self, urls), fields(db.system = "postgresql", n = urls.len()))]
    pub async fn annonces_par_url_avis(&self, urls: &[String]) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT url_avis, annonce_uid FROM annonce_source \
                 WHERE url_avis = ANY($1::text[])",
                &[&urls],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Les lignes ouvertes d'un fond que seul le BOAMP publie et dont l'avis
    /// déclare une plateforme de dépôt : la moisson n'a pas su les rattacher, seule
    /// leur jumelle plateforme peut les réunir.
    #[tracing::instrument(name = "db.annonces_a_jumeler", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonces_a_jumeler(&self, fond: &str) -> Result<Vec<AnnonceAJumeler>> {
        let rows = self
            .conn
            .query(
                &format!(
                "SELECT a.uid, a.titre, a.acteur_libelle, a.echeance, a.details->>'depot', \
                        COALESCE(array_agg(u.url) FILTER (WHERE u.url IS NOT NULL), '{{}}'), \
                        {} \
                 FROM annonce a \
                 LEFT JOIN LATERAL ( \
                   SELECT unnest(ARRAY[el->>'avis', el->>'rc', el->>'dce']) AS url \
                   FROM jsonb_array_elements(a.details->'liens') el \
                 ) u ON true \
                 WHERE a.fond = $1 AND a.echeance > NOW() \
                   AND a.details->>'depot' IS NOT NULL \
                   AND EXISTS (SELECT 1 FROM annonce_source s WHERE s.annonce_uid = a.uid) \
                   AND NOT EXISTS (SELECT 1 FROM annonce_source s \
                                   WHERE s.annonce_uid = a.uid AND s.source_uid NOT LIKE 'boamp:%') \
                 GROUP BY a.uid",
                    acheteur_unique("a")
                ),
                &[&fond],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| AnnonceAJumeler {
                uid: r.get(0),
                titre: r.get(1),
                acteur_libelle: r.get(2),
                echeance: r.get(3),
                depot: r.get(4),
                urls: r.get(5),
                acheteur_uid: r.get(6),
            })
            .collect())
    }

    /// Tous les canaux de plateforme des lignes ouvertes d'un fond — le vivier du
    /// jumelage. Une ligne à plusieurs canaux rend une entrée par canal : c'est le
    /// canal, et non le moteur, qui désigne l'instance.
    #[tracing::instrument(name = "db.canaux_plateforme_ouverts", skip(self), fields(db.system = "postgresql"))]
    pub async fn canaux_plateforme_ouverts(&self, fond: &str) -> Result<Vec<AnnonceCanalOuvert>> {
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT s.source_uid, a.uid, a.titre, a.acteur_libelle, a.echeance, {} \
                     FROM annonce a JOIN annonce_source s ON s.annonce_uid = a.uid \
                     WHERE a.fond = $1 AND a.echeance > NOW() \
                       AND s.source_uid NOT LIKE 'boamp:%'",
                    acheteur_unique("a")
                ),
                &[&fond],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| AnnonceCanalOuvert {
                source_uid: r.get(0),
                annonce: AnnonceChamps {
                    uid: r.get(1),
                    titre: r.get(2),
                    acteur_libelle: r.get(3),
                    echeance: r.get(4),
                    acheteur_uid: r.get(5),
                },
            })
            .collect())
    }

    /// Le recensement des profils d'acheteurs, hôte par hôte : à quel moteur
    /// appartient une plateforme. Les hôtes que la sonde n'a pas su classer
    /// n'en sont pas.
    #[tracing::instrument(name = "db.moteurs_par_hote", skip(self), fields(db.system = "postgresql"))]
    pub async fn moteurs_par_hote(&self) -> Result<Vec<(String, String)>> {
        let rows = self
            .conn
            .query(
                "SELECT host, moteur FROM profil_acheteur WHERE moteur IS NOT NULL",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Pose l'attribution sur la ligne du marché déjà en stock : le bloc
    /// `details.resultat` write-once, l'avis de résultat comme canal de plus, et les
    /// arêtes `titulaire`. La ligne garde son titre, son acheteur et son échéance de
    /// consultation — seul son état change.
    #[tracing::instrument(name = "db.poser_resultats", skip(self, poses), fields(db.system = "postgresql", n = poses.len()))]
    pub async fn poser_resultats(&self, poses: &[ResultatPose]) -> Result<u64> {
        if poses.is_empty() {
            return Ok(0);
        }
        let uid: Vec<&str> = poses.iter().map(|p| p.annonce_uid.as_str()).collect();
        let resultat: Vec<Json<&serde_json::Value>> =
            poses.iter().map(|p| Json(&p.resultat)).collect();
        let poses_count = self
            .conn
            .execute(
                "
                UPDATE annonce a
                SET details = jsonb_set(a.details, '{resultat}', t.j)
                FROM unnest($1::text[], $2::jsonb[]) AS t(u, j)
                WHERE a.uid = t.u AND a.details->'resultat' IS DISTINCT FROM t.j
                ",
                &[&uid, &resultat],
            )
            .await?;

        let s_uid: Vec<&str> = poses.iter().map(|p| p.source.source_uid.as_str()).collect();
        let s_url: Vec<Option<&str>> = poses.iter().map(|p| p.source.url_avis.as_deref()).collect();
        self.ecrire_canaux(&s_uid, &uid, &s_url).await?;

        let mut e_annonce: Vec<&str> = Vec::new();
        let mut e_entity: Vec<String> = Vec::new();
        let mut e_role: Vec<&str> = Vec::new();
        for p in poses {
            for (siren, role) in &p.aretes_siren {
                e_annonce.push(&p.annonce_uid);
                e_entity.push(format!("siren:{siren}"));
                e_role.push(role);
            }
        }
        self.ecrire_aretes(&e_annonce, &e_entity, &e_role).await?;

        Ok(poses_count)
    }

    /// La file d'enrichissement par page de détail, dérivée : rien n'est stampé,
    /// elle se calcule comme la fenêtre ouverte. Les annonces ouvertes du fond sans
    /// bloc `details.detail`, hors BOAMP — dont le flux porte déjà cpv et procédure.
    /// Une ligne par annonce : le canal de plus haute confiance (l'ordre `PRIORITE`
    /// de `lj-core`) qui porte une `url_avis`. Les plus proches de leur échéance
    /// d'abord.
    ///
    /// `sans_acheteur` échange le critère de file : les lignes sans libellé
    /// d'acheteur, visitées ou non. C'est le rattrapage des fiches lues avant que
    /// l'extracteur ne sache lire l'acheteur, que le bloc `detail` posé avait
    /// sorties de la file pour toujours.
    #[tracing::instrument(name = "db.annonces_a_enrichir", skip(self, priorite), fields(db.system = "postgresql"))]
    pub async fn annonces_a_enrichir(
        &self,
        fond: &str,
        priorite: &[&str],
        limite: i64,
        sans_acheteur: bool,
    ) -> Result<Vec<AnnonceAEnrichir>> {
        let file = if sans_acheteur {
            "a.acteur_libelle = ''"
        } else {
            "NOT a.details ? 'detail'"
        };
        let rows = self
            .conn
            .query(
                &format!(
                    "SELECT a.uid, split_part(s.source_uid, ':', 1) AS moteur, s.url_avis \
                 FROM annonce a \
                 JOIN LATERAL ( \
                   SELECT s.source_uid, s.url_avis FROM annonce_source s \
                   WHERE s.annonce_uid = a.uid AND s.url_avis IS NOT NULL \
                   ORDER BY array_position($2::text[], split_part(s.source_uid, ':', 1)) \
                            NULLS LAST, s.source_uid \
                   LIMIT 1 \
                 ) s ON true \
                 WHERE a.fond = $1 AND a.echeance > NOW() \
                   AND {file} \
                   AND NOT EXISTS (SELECT 1 FROM annonce_source b \
                                   WHERE b.annonce_uid = a.uid \
                                     AND b.source_uid LIKE 'boamp:%') \
                 ORDER BY a.echeance \
                 LIMIT $3"
                ),
                &[&fond, &priorite, &limite],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| AnnonceAEnrichir {
                uid: r.get(0),
                moteur: r.get(1),
                url_avis: r.get(2),
            })
            .collect())
    }

    /// Pose le bloc `details.detail` d'un lot d'annonces. Le bloc est écrit même
    /// vide : c'est le marqueur « déjà visité » qui sort la ligne de la file. Un
    /// `cpv_uid` absent laisse celui du stock.
    #[tracing::instrument(name = "db.poser_detail_annonces", skip(self, poses), fields(db.system = "postgresql", n = poses.len()))]
    pub async fn poser_detail_annonces(&self, poses: &[DetailPose]) -> Result<u64> {
        if poses.is_empty() {
            return Ok(0);
        }
        let uid: Vec<&str> = poses.iter().map(|p| p.annonce_uid.as_str()).collect();
        let cpv: Vec<Option<&str>> = poses.iter().map(|p| p.cpv_uid.as_deref()).collect();
        let detail: Vec<Json<&serde_json::Value>> = poses.iter().map(|p| Json(&p.detail)).collect();
        let acheteur: Vec<Option<&str>> = poses.iter().map(|p| p.acheteur.as_deref()).collect();
        Ok(self
            .conn
            .execute(
                "
                UPDATE annonce a
                SET cpv_uid = COALESCE(t.c, a.cpv_uid),
                    details = a.details || jsonb_build_object('detail', t.j),
                    acteur_libelle = COALESCE(NULLIF(a.acteur_libelle, ''), t.p, a.acteur_libelle)
                FROM unnest($1::text[], $2::text[], $3::jsonb[], $4::text[]) AS t(u, c, j, p)
                WHERE a.uid = t.u
                ",
                &[&uid, &cpv, &detail, &acheteur],
            )
            .await?)
    }

    /// Graphies d'une dénomination d'acheteur à confronter au répertoire, en plus
    /// de celle que l'avis publie. Expose `v.cle`, déjà pliée.
    ///
    /// L'Insee aplatit la ponctuation — « DEPARTEMENT DE L ISERE », « SEINE
    /// MARITIME » — là où un avis signe « Département de l'Isère » et
    /// « Seine-Maritime ». Il abrège aussi les formes longues (« CC
    /// PLEYBEN-CHATEAULIN-PORZAY ») et nomme « COMMUNE DE X » ce que la mairie signe
    /// « Ville de X ». Les ponts s'appliquent après pliage, donc sur du minuscule
    /// sans accent — un motif accentué ne filtrerait rien.
    const VARIANTES_ACHETEUR: &'static str = r#"
        CROSS JOIN LATERAL (
          SELECT lj_fold(regexp_replace(a.acteur_libelle, '[''’\-]', ' ', 'g')) AS p
        ) AS n
        CROSS JOIN LATERAL (VALUES
          (n.p),
          (regexp_replace(n.p, '^(ville|mairie) ', 'commune ')),
          (regexp_replace(n.p, '^conseil (departemental|general) ', 'departement ')),
          (regexp_replace(n.p, '^conseil regional ', 'region ')),
          (regexp_replace(n.p, '^communaute de communes ', 'cc ')),
          (regexp_replace(n.p, '^communaute d agglomeration ', 'ca ')),
          (regexp_replace(n.p, '^communaute urbaine ', 'cu ')),
          (regexp_replace(n.p, '^(centre hospitalier|chu|chr) ', 'centre hospitalier '))
        ) AS v(cle)
    "#;

    /// Rattache les annonces ouvertes sans arête d'acheteur à une entité, par
    /// dénomination pliée, en essayant les graphies de [`VARIANTES_ACHETEUR`]. Le
    /// pliage n'est stocké nulle part : `lj_fold`, jumeau SQL du pliage Rust, le
    /// calcule des deux côtés. Une dénomination portée par plusieurs entités ne
    /// rattache rien (`count(DISTINCT uid) = 1`), et une ligne dont les variantes
    /// désignent deux entités différentes non plus.
    ///
    /// Le sondage passe par l'opérateur de recouvrement `&&` sur
    /// `lj_fold_all(denominations)`, seule forme qui emprunte le GIN
    /// `entity_folded_all_idx` : un `IN (…)` sur la colonne dépliée fait recalculer
    /// `lj_fold_all` sur les 29 M d'entités. L'array des clés est passé en
    /// paramètre, donc constant pour le planificateur. La population est restreinte
    /// avant que les variantes ne soient étendues.
    #[tracing::instrument(name = "db.relier_annonces_par_denomination", skip(self), fields(db.system = "postgresql"))]
    pub async fn relier_annonces_par_denomination(&self) -> Result<u64> {
        let ouverte_a = ouverte("a");
        let variantes = Self::VARIANTES_ACHETEUR;
        let cles: Vec<String> = self
            .conn
            .query(
                &format!(
                    "SELECT DISTINCT v.cle \
                     FROM annonce a {variantes} \
                     WHERE {ouverte_a} AND a.acteur_libelle <> '' \
                       AND NOT EXISTS (SELECT 1 FROM annonce_entity e \
                                       WHERE e.annonce_uid = a.uid AND e.role = 'acheteur')"
                ),
                &[],
            )
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect();
        if cles.is_empty() {
            return Ok(0);
        }
        Ok(self
            .conn
            .execute(
                &format!(
                    "
                    WITH cle AS MATERIALIZED (
                      SELECT DISTINCT c FROM unnest($1::text[]) AS c
                    ), noms AS (
                      SELECT e.uid, f.folded
                      FROM entity e,
                           LATERAL unnest(lj_fold_all(e.denominations)) AS f(folded)
                      WHERE lj_fold_all(e.denominations) && $1::text[]
                        AND f.folded = ANY(SELECT c FROM cle)
                        AND e.uid NOT LIKE 'cnb:%' AND e.uid NOT LIKE 'oacc:%'
                    ), hit AS (
                      SELECT folded, min(uid) AS uid FROM noms
                      GROUP BY folded HAVING count(DISTINCT uid) = 1
                    ), candidate AS MATERIALIZED (
                      SELECT a.uid, a.acteur_libelle
                      FROM annonce a
                      WHERE {ouverte_a} AND a.acteur_libelle <> ''
                        AND NOT EXISTS (SELECT 1 FROM annonce_entity e
                                        WHERE e.annonce_uid = a.uid AND e.role = 'acheteur')
                    ), lie AS (
                      SELECT a.uid AS annonce_uid, min(h.uid) AS entity_uid
                      FROM candidate a {variantes}
                      JOIN hit h ON h.folded = v.cle
                      GROUP BY a.uid HAVING count(DISTINCT h.uid) = 1
                    )
                    INSERT INTO annonce_entity (annonce_uid, entity_uid, role)
                    SELECT annonce_uid, entity_uid, 'acheteur' FROM lie
                    ON CONFLICT DO NOTHING
                    "
                ),
                &[&cles],
            )
            .await?)
    }

    #[tracing::instrument(name = "db.upsert_profils_acheteurs", skip(self, profils), fields(db.system = "postgresql", n = profils.len()))]
    pub async fn upsert_profils_acheteurs(
        &self,
        profils: &[(String, Option<String>, i32)],
    ) -> Result<u64> {
        if profils.is_empty() {
            return Ok(0);
        }
        let host: Vec<&str> = profils.iter().map(|(h, _, _)| h.as_str()).collect();
        let moteur: Vec<Option<&str>> = profils.iter().map(|(_, m, _)| m.as_deref()).collect();
        let avis: Vec<i32> = profils.iter().map(|(_, _, a)| *a).collect();
        Ok(self
            .conn
            .execute(
                "
                INSERT INTO profil_acheteur (host, moteur, avis_boamp, first_seen_at, last_seen_at)
                SELECT h, m, a, NOW(), NOW()
                FROM unnest($1::text[], $2::text[], $3::int[]) AS t(h, m, a)
                ON CONFLICT (host) DO UPDATE SET
                  -- Un moteur identifié n'est jamais effacé par un sondage muet
                  -- (hôte momentanément injoignable).
                  moteur = COALESCE(EXCLUDED.moteur, profil_acheteur.moteur),
                  avis_boamp = EXCLUDED.avis_boamp,
                  last_seen_at = NOW()
                ",
                &[&host, &moteur, &avis],
            )
            .await?)
    }

    /// Les hôtes recensés d'un moteur multi-tenant (Atexo, Safetender).
    #[tracing::instrument(name = "db.hotes_profil_acheteur", skip(self), fields(db.system = "postgresql"))]
    pub async fn hotes_profil_acheteur(&self, moteur: &str) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT host FROM profil_acheteur WHERE moteur = $1 ORDER BY avis_boamp DESC, host",
                &[&moteur],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Tous les hôtes recensés et classés par moteur — les plateformes avérées,
    /// validateurs du dépôt de dernier ressort. Jamais les hôtes non classés : un
    /// avis cite aussi le site institutionnel de son acheteur.
    #[tracing::instrument(name = "db.hotes_plateformes_classees", skip(self), fields(db.system = "postgresql"))]
    pub async fn hotes_plateformes_classees(&self) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT host FROM profil_acheteur WHERE moteur IS NOT NULL",
                &[],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    #[tracing::instrument(name = "db.tailles_tables_annonces", skip(self), fields(db.system = "postgresql"))]
    pub async fn tailles_tables_annonces(&self) -> Result<Vec<TableSize>> {
        let rows = self
            .conn
            .query(
                "SELECT t, pg_total_relation_size(t)::bigint \
                 FROM unnest(ARRAY['annonce','annonce_source','annonce_entity','profil_acheteur']) AS t",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| TableSize {
                table: r.get(0),
                bytes: r.get(1),
            })
            .collect())
    }

    #[tracing::instrument(name = "db.annonces", skip(self, f), fields(db.system = "postgresql", fond = %f.fond))]
    pub async fn annonces(&self, f: &AnnonceFiltre) -> Result<Vec<AnnonceRow>> {
        let mut filtre = Filtre::new(f, None);
        let limit = filtre.push(Box::new(f.limit));
        let offset = filtre.push(Box::new(f.offset));
        let sql = format!(
            "SELECT {ANNONCE_COLS} FROM annonce a {ANNONCE_JOINS} \
             WHERE a.fond = $1{} ORDER BY {} LIMIT ${limit} OFFSET ${offset}",
            filtre.clauses,
            order_by(f.ordre),
        );
        let rows = self.conn.query(&sql, &filtre.refs()).await?;
        Ok(rows.iter().map(annonce_from_row).collect())
    }

    #[tracing::instrument(name = "db.annonces_total", skip(self, f), fields(db.system = "postgresql"))]
    pub async fn annonces_total(&self, f: &AnnonceFiltre) -> Result<i64> {
        let filtre = Filtre::new(f, None);
        let sql = format!(
            "SELECT count(*)::bigint FROM annonce a WHERE a.fond = $1{}",
            filtre.clauses
        );
        let row = self.conn.query_one(&sql, &filtre.refs()).await?;
        Ok(row.get(0))
    }

    /// Comptes d'un axe de facette, son propre filtre retiré (multi-sélection).
    /// La valeur nulle (axe non renseigné par la source) n'est pas offerte au
    /// filtre, donc pas comptée.
    #[tracing::instrument(name = "db.annonces_facette", skip(self, f), fields(db.system = "postgresql", axe = axe.nom()))]
    pub async fn annonces_facette(
        &self,
        f: &AnnonceFiltre,
        axe: AnnonceAxe,
    ) -> Result<Vec<(String, Option<String>, i64)>> {
        let filtre = Filtre::new(f, Some(axe));
        let colonne = axe.colonne();
        // Les axes référentiels (`nature_marche:*`, `cpv:*`) tiennent leur libellé
        // et leur ordre éditorial de `facet_value` ; le département est un code brut,
        // son nom est résolu côté API.
        let sql = match axe {
            AnnonceAxe::Departement => format!(
                "SELECT {colonne} AS cle, NULL::text AS label, count(*)::bigint AS n \
                 FROM annonce a WHERE a.fond = $1{} AND {colonne} IS NOT NULL \
                 GROUP BY 1 ORDER BY 1",
                filtre.clauses
            ),
            AnnonceAxe::Famille | AnnonceAxe::Cpv => format!(
                "SELECT {colonne} AS cle, fv.label AS label, count(*)::bigint AS n \
                 FROM annonce a JOIN facet_value fv ON fv.uid = {colonne} \
                 WHERE a.fond = $1{} GROUP BY 1, 2, fv.sort ORDER BY fv.sort",
                filtre.clauses
            ),
        };
        let rows = self.conn.query(&sql, &filtre.refs()).await?;
        Ok(rows
            .iter()
            .map(|r| (r.get("cle"), r.get("label"), r.get("n")))
            .collect())
    }

    /// Comptes des fenêtres d'échéance (« ferme sous N jours »), le filtre
    /// d'échéance courant retiré.
    #[tracing::instrument(name = "db.annonces_fenetres_echeance", skip(self, f), fields(db.system = "postgresql"))]
    pub async fn annonces_fenetres_echeance(
        &self,
        f: &AnnonceFiltre,
        fenetres: &[i32],
    ) -> Result<Vec<(i32, i64)>> {
        let mut filtre = Filtre::sans_fenetre(f);
        let ph = filtre.push(Box::new(fenetres.to_vec()));
        let sql = format!(
            "SELECT j, count(a.uid)::bigint AS n \
             FROM unnest(${ph}::int[]) AS j \
             LEFT JOIN annonce a ON a.fond = $1{} \
                  AND a.echeance IS NOT NULL \
                  AND a.echeance <= NOW() + make_interval(days => j) \
             GROUP BY j ORDER BY j",
            filtre.clauses
        );
        let rows = self.conn.query(&sql, &filtre.refs()).await?;
        Ok(rows.iter().map(|r| (r.get("j"), r.get("n"))).collect())
    }

    /// Une annonce par son uid, closes comprises (la fiche d'une consultation
    /// clôturée reste servie : elle a été indexée, et c'est l'archive).
    #[tracing::instrument(name = "db.annonce", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonce(&self, uid: &str) -> Result<Option<AnnonceRow>> {
        let sql = format!("SELECT {ANNONCE_COLS} FROM annonce a {ANNONCE_JOINS} WHERE a.uid = $1");
        Ok(self
            .conn
            .query_opt(&sql, &[&uid])
            .await?
            .as_ref()
            .map(annonce_from_row))
    }

    /// Les canaux d'une annonce, le plus anciennement vu d'abord.
    #[tracing::instrument(name = "db.annonce_canaux", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonce_canaux(&self, uid: &str) -> Result<Vec<AnnonceCanalRow>> {
        let rows = self
            .conn
            .query(
                "SELECT source_uid, url_avis, first_seen_at \
                 FROM annonce_source WHERE annonce_uid = $1 ORDER BY first_seen_at, source_uid",
                &[&uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| AnnonceCanalRow {
                source_uid: r.get(0),
                url_avis: r.get(1),
                first_seen_at: r.get(2),
            })
            .collect())
    }

    #[tracing::instrument(name = "db.annonce_entites", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonce_entites(&self, uid: &str) -> Result<Vec<AnnonceEntiteRow>> {
        let rows = self
            .conn
            .query(
                "SELECT ae.entity_uid, ae.role, e.denomination \
                 FROM annonce_entity ae \
                 LEFT JOIN entity e ON e.uid = ae.entity_uid \
                 WHERE ae.annonce_uid = $1 ORDER BY ae.role, ae.entity_uid",
                &[&uid],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| AnnonceEntiteRow {
                entity_uid: r.get(0),
                role: r.get(1),
                denomination: r.get(2),
            })
            .collect())
    }

    /// Les annonces ouvertes d'une entité, `par_role` au plus par rôle, par
    /// échéance. Le fond `attribution` échappe à la fenêtre — un fait constaté ne
    /// s'ouvre ni ne se ferme — et se lit par parution : c'est le palmarès
    /// « Marchés remportés ». Le total du rôle accompagne chaque ligne : la section
    /// de la fiche annonce « 12 marchés en cours » en n'en rendant que 5.
    #[tracing::instrument(name = "db.annonces_entite", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonces_entite(
        &self,
        entity_uid: &str,
        par_role: i64,
    ) -> Result<Vec<EntityAnnonceRow>> {
        let ouverte_an = ouverte("an");
        let sql = format!(
            "WITH arete AS (
               SELECT ae.role, ae.annonce_uid,
                      count(*) OVER (PARTITION BY ae.role) AS total_role,
                      row_number() OVER (
                        PARTITION BY ae.role
                        ORDER BY an.echeance ASC NULLS LAST,
                                 an.date_publication DESC, an.uid
                      ) AS rang
               FROM annonce_entity ae
               JOIN annonce an ON an.uid = ae.annonce_uid
               WHERE ae.entity_uid = $1
                 AND (an.fond = 'attribution' OR {ouverte_an})
             )
             SELECT arete.role, arete.total_role, {ANNONCE_COLS}
             FROM arete
             JOIN annonce a ON a.uid = arete.annonce_uid
             {ANNONCE_JOINS}
             WHERE arete.rang <= $2
             ORDER BY arete.role, arete.rang"
        );
        let rows = self.conn.query(&sql, &[&entity_uid, &par_role]).await?;
        Ok(rows
            .iter()
            .map(|r| EntityAnnonceRow {
                role: r.get("role"),
                total_role: r.get("total_role"),
                annonce: annonce_from_row(r),
            })
            .collect())
    }

    /// Les uids à publier au sitemap : la fenêtre ouverte et l'archive récente,
    /// avec la date de parution en `lastmod` — la seule date stable de la ligne. La
    /// récence se lit sur l'horloge la mieux connue : l'échéance, sinon la parution.
    #[tracing::instrument(name = "db.annonces_sitemap", skip(self), fields(db.system = "postgresql"))]
    pub async fn annonces_sitemap(
        &self,
        fond: &str,
        archive_jours: i32,
    ) -> Result<Vec<(String, chrono::NaiveDate)>> {
        let rows = self
            .conn
            .query(
                "SELECT uid, date_publication FROM annonce \
                 WHERE fond = $1 \
                   AND COALESCE(echeance, date_publication::timestamptz) \
                       >= NOW() - make_interval(days => $2) \
                 ORDER BY uid",
                &[&fond, &archive_jours],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Les uids encore ouverts apparus depuis `since`, pour la soumission
    /// IndexNow — critère « première vue tous canaux ».
    ///
    /// La borne d'échéance sépare les deux canaux d'indexation : IndexNow est la
    /// voie rapide de ce qui a une date de péremption, le sitemap couvre le reste
    /// jusqu'à 180 jours.
    #[tracing::instrument(
        name = "db.annonces_vues_depuis",
        skip(self),
        fields(db.system = "postgresql")
    )]
    pub async fn annonces_vues_depuis(
        &self,
        fond: &str,
        since: chrono::DateTime<chrono::Utc>,
        limit: i64,
    ) -> Result<Vec<String>> {
        let rows = self
            .conn
            .query(
                "SELECT a.uid FROM annonce a \
                 JOIN annonce_source s ON s.annonce_uid = a.uid \
                 WHERE a.fond = $1 AND a.echeance > now() \
                 GROUP BY a.uid \
                 HAVING min(s.first_seen_at) >= $2 \
                 ORDER BY min(s.first_seen_at) DESC \
                 LIMIT $3",
                &[&fond, &since, &limit],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::motif_mot;

    #[test]
    fn motif_borne_le_mot_et_echappe_la_ponctuation() {
        assert_eq!(motif_mot("PLU"), "\\mPLU\\M");
        assert_eq!(motif_mot("voirie communale"), "\\mvoirie communale\\M");
        assert_eq!(motif_mot("C++"), "\\mC\\+\\+");
        assert_eq!(motif_mot("(lot"), "\\(lot\\M");
    }
}
