//! Le quantum d'une liquidation de préjudice corporel : le modèle lu, et le
//! tableur qui dit ce qu'il prouve.
//!
//! Une décision écrit des signaux redondants, et les relations qui les lient
//! sont arithmétiques : vérifiables sans relire le texte. Le même moteur juge
//! les deux extracteurs, celui qui lit au modèle de langue et celui qui ratisse
//! le fonds, et c'est la seule raison pour laquelle un chiffre tiré du fonds
//! vaut un chiffre relu à la main.

pub mod ancrage;
pub mod arbitration;
pub mod dispositif;
pub mod leftovers;
pub mod obligations;
pub mod observation;
pub mod path;
pub mod spreadsheet;
#[cfg(test)]
mod tests;
pub mod vocabulary;

use serde::{Deserialize, Serialize};

/// Un demi-centime : au-delà, deux chemins qui donnent deux valeurs se
/// contredisent ; en dessous, ils concordent au centime écrit.
pub const EPS: f64 = 0.005;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quantum {
    #[serde(default)]
    pub est_liquidation: bool,
    #[serde(default)]
    pub fait_generateur: Option<String>,
    /// La victime directe est-elle décédée ? Le préjudice d'affection d'un
    /// proche s'indexe sur deux barèmes disjoints selon la réponse, et les
    /// mélanger dans une médiane ne décrit rien.
    #[serde(default)]
    pub deces: Option<bool>,
    /// Un arrêt qui indemnise sur deux fondements les porte tous deux, et
    /// n'entre alors dans aucune des deux médianes.
    #[serde(default)]
    pub regimes: Vec<Regime>,
    /// La citation de `regimes` : ce que l'arrêt en dit, avec ses articles.
    #[serde(default)]
    pub regime_libelle: Option<String>,
    /// La table dont la cour recopie le montant, quand elle barémise : la ligne
    /// cesse alors d'être un témoignage indépendant. Rien à voir avec
    /// `bareme_capitalisation`, qui est la table de rente.
    #[serde(default)]
    pub referentiel_cite: Option<String>,
    #[serde(default)]
    pub devise: Option<String>,
    #[serde(default)]
    pub bareme_capitalisation: Option<Bareme>,
    /// Où chaque champ se lit dans l'arrêt. Rempli par machine, jamais par la
    /// lecture, et indexé par chemin pour atteindre autre chose qu'un poste.
    #[serde(default)]
    pub ancres: Vec<Ancre>,
    #[serde(default)]
    pub total_enonce: Option<f64>,
    /// Ce que le total énoncé additionne : `postes_pleins` ou `parts_victime`.
    /// Sans lui, la somme des victimes ne se rattache à rien.
    #[serde(default)]
    pub total_enonce_base: Option<String>,
    #[serde(default)]
    pub contradictions: Vec<Contradiction>,
    #[serde(default)]
    pub commentaire: Option<String>,
    #[serde(default)]
    pub lecture: Option<Lecture>,
    #[serde(default)]
    pub corrections: Vec<Correction>,
    /// Amounts the dispositif writes that no field holds, each with its cause.
    #[serde(default)]
    pub hors_record: Vec<HorsRecord>,
    #[serde(default)]
    pub victimes: Vec<Victime>,
}

/// An amount the dispositif writes and the record holds in no field: declared
/// with its cause rather than left unread.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HorsRecord {
    pub montant: f64,
    pub cause: CauseHorsRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CauseHorsRecord {
    RenteServie,
    DeboursDeCaisse,
    TiersPayeur,
    Pretention,
    RappelDuJugement,
    Provision,
    #[serde(rename = "article_700")]
    Article700,
    Depens,
    ChiffreExpertise,
    Intermediaire,
    AutreLitige,
}

/// Le fondement de l'indemnisation, qui décide de quoi un montant se compare.
/// Une variante inconnue est refusée à la désérialisation : le vocabulaire
/// s'étend par une décision, il ne s'étend pas par une lecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Regime {
    /// Loi du 5 juillet 1985, accidents de la circulation.
    Badinter,
    /// Faute du professionnel ou de l'établissement de santé, L.1142-1 I du CSP.
    MedicaleFaute,
    /// Aléa thérapeutique à la charge de l'ONIAM, L.1142-1 II du CSP.
    MedicaleSolidarite,
    /// Infection nosocomiale : l'établissement répond sans faute.
    Nosocomiale,
    /// Faute inexcusable de l'employeur, L.452-1 et suivants du CSS.
    FauteInexcusable,
    /// FGTI : terrorisme et infractions, L.126-1 et L.422-1 du code des assurances.
    Fgti,
    /// FIVA : exposition à l'amiante, loi 2000-1257.
    Fiva,
    /// CIVEN : essais nucléaires, loi 2010-2.
    Civen,
    /// Garantie contractuelle accidents de la vie, qui déroge au droit commun.
    Gav,
    /// Responsabilité civile de droit commun, délictuelle ou contractuelle.
    DroitCommun,
}

/// La table de capitalisation, éclatée : une même rente ne vaut pas le même
/// capital à deux éditions ni à deux taux, donc ce sont des axes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bareme {
    #[serde(default)]
    pub table: Option<String>,
    #[serde(default)]
    pub edition: Option<String>,
    /// Signé, en points de pourcentage : `-1.0` pour un barème à taux négatif,
    /// `0.0` pour un taux neutre.
    #[serde(default)]
    pub taux_pct: Option<f64>,
    #[serde(default)]
    pub mortalite: Option<String>,
}

/// Le passage d'où sort un champ, nommé dans la grammaire de [`path`] : un
/// cofacteur de victime et une composante s'ancrent, pas seulement un poste.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ancre {
    pub champ: String,
    /// **Tous** les passages où le texte écrit la valeur, dans l'ordre. En
    /// retenir un serait un arbitrage que rien ne fonde : à six occurrences de
    /// « 68,13 », la donnée porte les six et la lecture voit ce qu'elle choisit.
    pub spans: Vec<Span>,
}

/// Un intervalle du texte, en caractères et non en octets.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Span {
    pub char_start: usize,
    pub char_end: usize,
}

/// Un compte de la décision qui ne ferme pas, sur la case où le contrôle le
/// trouve. Y figurer conserve le chiffre écrit, quand `corrections` le remplace,
/// et n'écarte que cette case : les postes restent mesurables.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contradiction {
    pub case: String,
    pub nature: Cause,
    pub raison: String,
}

/// Why a declared count does not close. Three are the court's own: it rounds or
/// truncates a figure, its arithmetic is wrong, or it rules outside what the
/// parties asked. The fourth is the model's: the court applies a rule the
/// spreadsheet has no relation for, a rate per poste, a preference right, a
/// subsidiary offer that binds nobody.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    Arrondi,
    Calcul,
    Pretentions,
    Modele,
}

/// Ce que la décision écrit et que la lecture ne retient pas : les champs du
/// record portent le retenu, ceci en garde la trace.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Correction {
    /// Le champ visé, dans la grammaire de [`path`] :
    /// `victimes[v1].postes[DFP].montant_alloue`.
    pub champ: String,
    #[serde(default)]
    pub nature: Nature,
    /// Ce que la source porte, tel quel : le nombre qu'écrit la décision, le
    /// libellé qu'un classement remplace, la chaîne qu'a mutilée le
    /// pseudonymiseur.
    #[serde(default)]
    pub cite: Option<serde_json::Value>,
    #[serde(default)]
    pub retenu: Option<serde_json::Value>,
    pub raison: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Nature {
    /// La décision écrit un chiffre que son propre calcul contredit.
    Texte,
    /// La pseudonymisation a mangé le chiffre : ce n'est pas la cour qui se
    /// trompe, c'est la source qui remplace `12 000 euros` par un jeton de date.
    Pseudonymisation,
    /// Aucun chiffre n'est faux : un champ est rempli par déduction.
    #[default]
    Interpretation,
}

impl Nature {
    pub fn name(self) -> &'static str {
        match self {
            Self::Texte => "texte",
            Self::Pseudonymisation => "pseudonymisation",
            Self::Interpretation => "interpretation",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lecture {
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub passes: u8,
    #[serde(default)]
    pub accord: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Victime {
    #[serde(default)]
    pub cle: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub lien: Option<Lien>,
    /// Les modalités qui découpent le lien : à parenté égale, un enfant mineur,
    /// un enfant majeur au foyer et un enfant majeur ailleurs relèvent de trois
    /// fourchettes disjointes.
    #[serde(default)]
    pub mineur: Option<bool>,
    #[serde(default)]
    pub cohabitation: Option<bool>,
    #[serde(default)]
    pub frequence_relations: Option<String>,
    #[serde(default)]
    pub sexe: Option<String>,
    #[serde(default)]
    pub profession: Option<String>,
    /// L'axe dont `profession` est le libellé : un retraité n'a pas de perte de
    /// gains futurs, et une médiane d'incidence professionnelle qui le compte
    /// ne décrit personne.
    #[serde(default)]
    pub statut_professionnel: Option<Statut>,
    #[serde(default)]
    pub date_naissance: Option<String>,
    #[serde(default)]
    pub age_fait: Option<f64>,
    #[serde(default)]
    pub date_consolidation: Option<String>,
    #[serde(default)]
    pub age_consolidation: Option<f64>,
    /// L'âge à la liquidation, celui auquel se lit un coefficient de
    /// capitalisation. Les deux autres âges ne le donnent pas.
    #[serde(default)]
    pub age_decision: Option<f64>,
    #[serde(default)]
    pub taux_dfp_pct: Option<f64>,
    /// De quoi le taux est fait : deux victimes à 65 % dont l'une est
    /// paraplégique et l'autre porte un syndrome anxio-dépressif ne relèvent
    /// pas de la même population.
    #[serde(default)]
    pub lesions: Vec<Lesion>,
    /// Un état antérieur réduit le taux imputable, et sans lui le montant
    /// paraît hors barème sans raison.
    #[serde(default)]
    pub etat_anterieur: Option<bool>,
    #[serde(default)]
    pub partage_responsabilite_pct: Option<f64>,
    #[serde(default)]
    pub perte_de_chance_pct: Option<f64>,
    #[serde(default)]
    pub total_alloue: Option<f64>,
    #[serde(default)]
    pub assiette_reduite: Option<f64>,
    #[serde(default)]
    pub provisions_deduites: Option<f64>,
    /// The insurer's pre-litigation offer as the decision recites it: one global
    /// amount and its date, never per poste.
    #[serde(default)]
    pub offre_amiable: Option<OffreAmiable>,
    #[serde(default)]
    pub net_victime: Option<f64>,
    #[serde(default)]
    pub creance_tiers_payeurs: Option<f64>,
    /// Le lecteur affirme que la liste est complète. Sans lui, aucune précision
    /// n'est mesurable.
    #[serde(default)]
    pub exhaustif: bool,
    #[serde(default)]
    pub postes: Vec<Poste>,
    /// Codes que la décision réserve sans les chiffrer. Ni attendus, ni fautifs.
    #[serde(default)]
    pub postes_reserves: Vec<String>,
    /// Codes que la cour chiffre déjà réduits, quand elle ne réduit pas tout.
    /// Le pourcentage dit de combien, celui-ci dit où.
    #[serde(default)]
    pub postes_reduits: Vec<String>,
    /// Cofacteurs que la décision énonce de deux façons contradictoires. `null`
    /// dirait « absent », donc punirait une extraction qui a lu l'une des deux
    /// phrases : ces champs sortent du dénominateur au lieu d'être notés.
    #[serde(default)]
    pub champs_indecidables: Vec<String>,
}

impl Victime {
    /// Un champ de poste sort du dénominateur sous la forme
    /// `postes.<code>.<champ>`, un cofacteur sous son seul nom.
    pub fn undecidable(&self, code: &str, path: &str) -> bool {
        let target = format!("postes.{code}.{path}");
        self.champs_indecidables.contains(&target)
    }
}

/// Le lien du proche à la victime directe, qui est l'abscisse du préjudice
/// d'affection. Fermé pour la même raison que [`Regime`] : une orthographe de
/// plus est un axe de plus. Le sexe n'y entre pas, `sexe` le porte déjà.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lien {
    Conjoint,
    Concubin,
    Partenaire,
    Enfant,
    Parent,
    Fratrie,
    GrandParent,
    PetitEnfant,
    BeauParent,
    BeauFils,
    OncleTante,
    NeveuNiece,
    Cousin,
    Autre,
}

impl Lien {
    pub fn name(self) -> &'static str {
        match self {
            Self::Conjoint => "conjoint",
            Self::Concubin => "concubin",
            Self::Partenaire => "partenaire",
            Self::Enfant => "enfant",
            Self::Parent => "parent",
            Self::Fratrie => "fratrie",
            Self::GrandParent => "grand_parent",
            Self::PetitEnfant => "petit_enfant",
            Self::BeauParent => "beau_parent",
            Self::BeauFils => "beau_fils",
            Self::OncleTante => "oncle_tante",
            Self::NeveuNiece => "neveu_niece",
            Self::Cousin => "cousin",
            Self::Autre => "autre",
        }
    }
}

/// L'unité du tarif. Fermée : `jour` et `jour de déficit fonctionnel temporaire
/// total` sont le même axe, et les séparer coupe la table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unite {
    Point,
    Jour,
    Semaine,
    Mois,
    An,
    Heure,
    Seance,
    /// Le prix d'un euro de rente annuelle : la quantité est alors un
    /// coefficient de capitalisation, et la valeur unitaire un revenu annuel.
    EuroDeRente,
}

impl Unite {
    pub fn name(self) -> &'static str {
        match self {
            Self::Point => "point",
            Self::Jour => "jour",
            Self::Semaine => "semaine",
            Self::Mois => "mois",
            Self::An => "an",
            Self::Heure => "heure",
            Self::Seance => "seance",
            Self::EuroDeRente => "euro_de_rente",
        }
    }
}

/// La nature de l'aide humaine, seul axe que les référentiels tarifent à part :
/// l'heure d'aide active et l'heure de présence responsable n'ont pas le même
/// prix, et une médiane qui les mêle ne décrit ni l'une ni l'autre. Ce à quoi
/// l'aide sert, ménage ou jardin, reste au libellé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NatureAide {
    Active,
    PresenceResponsable,
    Autre,
}

impl NatureAide {
    pub fn name(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::PresenceResponsable => "presence_responsable",
            Self::Autre => "autre",
        }
    }
}

/// Une bande d'un tarif lui-même composé : 404 euros par jour valant
/// « (10 h x 25 euros) + (14 h x 11 euros) ». Sans elle le taux horaire, qui est
/// la valeur standard de l'aide humaine, est perdu derrière un prix de journée.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tranche {
    #[serde(default)]
    pub libelle: Option<String>,
    #[serde(default)]
    pub nature_aide: Option<NatureAide>,
    #[serde(default)]
    pub valeur_unitaire: Option<f64>,
    #[serde(default)]
    pub quantite: Option<f64>,
    #[serde(default)]
    pub unite: Option<Unite>,
}

/// Ce que la victime faisait, comme axe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Statut {
    Actif,
    Independant,
    SansEmploi,
    Retraite,
    Etudiant,
    Enfant,
    AuFoyer,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lesion {
    #[serde(default)]
    pub libelle: Option<String>,
    #[serde(default)]
    pub localisation: Option<String>,
    /// Le taux que l'expertise impute à cette lésion, quand elle le ventile.
    #[serde(default)]
    pub aipp_pct: Option<f64>,
}

/// Une indemnité servie en rente. Sa présence interrompt la somme du découpage,
/// qui sinon accuserait la lecture de l'écart qu'elle vient de déclarer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rente {
    /// L'arrérage tel que la cour l'écrit, dans la période de `periodicite`.
    /// Une rente trimestrielle ne se multiplie pas par quatre à la lecture : la
    /// mise à l'année est une dérivation, elle se calcule à l'usage.
    #[serde(default)]
    pub montant: Option<f64>,
    #[serde(default)]
    pub periodicite: Option<String>,
    #[serde(default)]
    pub depart: Option<String>,
    #[serde(default)]
    pub viagere: Option<bool>,
    #[serde(default)]
    pub indexation: Option<String>,
    #[serde(default)]
    pub suspension: Option<String>,
}

/// Ce qu'un `1 872 x 35,597` ne dit pas : à quel âge le coefficient a été lu et
/// jusqu'où il court. Le coefficient lui-même reste `quantite`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capitalisation {
    #[serde(default)]
    pub viagere: Option<bool>,
    #[serde(default)]
    pub age: Option<f64>,
    /// L'âge auquel s'arrête une capitalisation temporaire : 65 ans pour une
    /// perte de gains, la fin des études pour un préjudice scolaire.
    #[serde(default)]
    pub age_terme: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OffreAmiable {
    #[serde(default)]
    pub montant: Option<f64>,
    #[serde(default)]
    pub date: Option<String>,
    /// The court's own verdict on the offer: `true` when it doubles the legal
    /// interest for a late or missing offer, `false` when it refuses to, `None`
    /// when nothing was asked.
    #[serde(default)]
    pub doublement_interets: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Poste {
    /// `null` pour un libellé hors des trente codes : forcer un code approchant
    /// est interdit, et l'appariement se rabat sur le libellé.
    #[serde(default)]
    pub poste: Option<String>,
    #[serde(default)]
    pub libelle_decision: Option<String>,
    #[serde(default)]
    pub montant_demande: Option<f64>,
    #[serde(default)]
    pub montant_offert: Option<f64>,
    /// What the first judge awarded on this poste, as the appeal recites it.
    #[serde(default)]
    pub alloue_premiere_instance: Option<f64>,
    #[serde(default)]
    pub montant_alloue: Option<f64>,
    /// What earlier liquidations already paid on this poste, each sum as the
    /// decision recites it, when it only repairs an aggravation:
    /// `montant_alloue` stays the complement as written.
    #[serde(default)]
    pub indemnites_acquises: Vec<f64>,
    /// L'indemnité mise à la charge du responsable, telle que la décision
    /// l'écrit. `montant_alloue` est le poste plein, celui-ci le résultat après
    /// partage ou perte de chance.
    #[serde(default)]
    pub montant_du: Option<f64>,
    #[serde(default)]
    pub part_victime: Option<f64>,
    #[serde(default)]
    pub part_tiers_payeurs: Option<f64>,
    /// Abscisses du montant, sans lesquelles il ne se compare à rien : la note
    /// sur 7, et le calcul que la cour écrit.
    #[serde(default)]
    pub cotation: Option<f64>,
    /// Le tarif PLEIN. Un prorata écrasé dedans mélangerait 6,25 et 25 euros
    /// dans la même médiane : la fraction vit dans `taux_pct`.
    #[serde(default)]
    pub valeur_unitaire: Option<f64>,
    #[serde(default)]
    pub quantite: Option<f64>,
    #[serde(default)]
    pub unite: Option<Unite>,
    /// Le taux qui proratise la tranche, en points de pourcentage.
    #[serde(default)]
    pub taux_pct: Option<f64>,
    #[serde(default)]
    pub nature_aide: Option<NatureAide>,
    /// Le détail de `valeur_unitaire` quand la cour l'obtient en additionnant des
    /// bandes horaires. La somme de leurs produits vaut ce tarif.
    #[serde(default)]
    pub tranches: Vec<Tranche>,
    #[serde(default)]
    pub capitalisation: Option<Capitalisation>,
    /// A lifetime award brought back to the time the victim actually lived.
    #[serde(default)]
    pub prorata_survie: Option<ProrataSurvie>,
    #[serde(default)]
    pub composantes: Vec<Composante>,
}

/// A permanent poste the court prices for life, then reduces to the time the
/// victim actually lived when they died of another cause before liquidation:
/// « 20 000 euros / 25,761 x 1,21 an ». The lifetime award is the value the
/// court set for the cotation; `montant_alloue` stays the result as written.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProrataSurvie {
    pub montant_viager: f64,
    pub euro_de_rente: f64,
    pub duree_ans: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Composante {
    #[serde(default)]
    pub libelle: Option<String>,
    #[serde(default)]
    pub montant: Option<f64>,
    #[serde(default)]
    pub valeur_unitaire: Option<f64>,
    #[serde(default)]
    pub quantite: Option<f64>,
    #[serde(default)]
    pub unite: Option<Unite>,
    #[serde(default)]
    pub taux_pct: Option<f64>,
    #[serde(default)]
    pub nature_aide: Option<NatureAide>,
    #[serde(default)]
    pub tranches: Vec<Tranche>,
    #[serde(default)]
    pub capitalisation: Option<Capitalisation>,
    #[serde(default)]
    pub rente: Option<Rente>,
}

/// Deux chemins, deux valeurs, sur la même case.
#[derive(Debug, Clone, Serialize)]
pub struct Conflict {
    pub cell: String,
    pub first: f64,
    pub first_source: String,
    pub second: f64,
    pub second_source: String,
}

/// Un écart sous la tolérance de la relation qui l'a posé : la dérive
/// consommée, dite plutôt que tue.
#[derive(Debug, Clone, Serialize)]
pub struct Rounding {
    pub cell: String,
    pub gap: f64,
    pub tolerance: f64,
    pub source: String,
}

/// What the victim receives on a poste, outside what the parties set: above the
/// claim, or below the offer. The cell names the bound, `award` the share that
/// crossed it.
#[derive(Debug, Clone, Serialize)]
pub struct Breach {
    pub cell: String,
    pub value: f64,
    pub award: f64,
}

/// Ce que la lecture prouve. `confirmees` est la seule mesure de force : une
/// case que deux chemins indépendants déterminent en concordant.
#[derive(Debug, Clone, Serialize)]
pub struct Verdict {
    pub cells: usize,
    pub confirmed: usize,
    pub conflicts: Vec<Conflict>,
    pub roundings: Vec<Rounding>,
    pub breaches: Vec<Breach>,
}

impl Verdict {
    /// Un record se tient quand aucune case ne porte deux valeurs.
    pub fn holds(&self) -> bool {
        self.conflicts.is_empty()
    }
}

/// Le nombre de décimales que porte la valeur. Un entier est exact ; un prix de
/// l'euro de rente à trois décimales porte un demi-millième d'incertitude, et
/// c'est ce demi-millième qui fixe la tolérance du produit où il entre.
pub fn decimals(x: f64) -> u32 {
    let s = format!("{x:.10}");
    let s = s.trim_end_matches('0');
    match s.split_once('.') {
        Some((_, frac)) => frac.len() as u32,
        None => 0,
    }
}

/// L'incertitude d'un produit est celle de ses facteurs, propagée : un facteur
/// arrondi au millième vaut plus ou moins un demi-millième, et cela pèse sur le
/// produit à proportion des autres facteurs. 1 872 x 35,597 tolère ainsi
/// 0,94 euro, non un centime.
pub fn product_tolerance(factors: &[f64]) -> f64 {
    let mut t = EPS;
    for (i, f) in factors.iter().enumerate() {
        let d = decimals(*f);
        if d == 0 {
            continue;
        }
        let others: f64 = factors
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, g)| g.abs())
            .product();
        t += others * 0.5 * 10f64.powi(-(d as i32));
    }
    t
}

/// Le calcul que la cour écrit, en trois facteurs : un tarif unitaire, une
/// quantité, et le taux qui proratise la tranche. « 25 euros x 25 % x 198
/// jours » vaut 1 237,50, et le tarif y reste celui du jour plein.
///
/// Le taux entre dans la tolérance en points de pourcentage, où un 25 est
/// exact ; le passer en 0,25 lui prêterait un demi-centième d'incertitude, soit
/// 24 euros de marge sur ce produit.
pub fn product(
    valeur_unitaire: Option<f64>,
    quantite: Option<f64>,
    taux_pct: Option<f64>,
) -> Option<(f64, String, f64)> {
    let (seen_one, qt) = (valeur_unitaire?, quantite?);
    match taux_pct {
        None => Some((
            seen_one * qt,
            format!("{seen_one:.2} x {qt}"),
            product_tolerance(&[seen_one, qt]),
        )),
        Some(tx) => Some((
            seen_one * qt * tx / 100.0,
            format!("{seen_one:.2} x {tx} % x {qt}"),
            product_tolerance(&[seen_one, qt, tx]) / 100.0,
        )),
    }
}
