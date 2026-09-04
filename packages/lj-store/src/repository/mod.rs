//! Repository de décisions chunk-aware (Postgres + pgvector).
//! Aucun SQL ailleurs.
//!
//! Méthodes clés : [`DecisionRepository::upsert`] (INSERT/UPDATE métadonnées),
//! `replace_chunks` (DELETE + bulk INSERT chunks), `replace_citations`
//! (`legal_citation` à plat, module `citations`), `delete`. Le pipeline appelle
//! ces méthodes dans une transaction unique.
//!
//! Découpé par domaine en sous-modules (blocs `impl DecisionRepository`
//! répartis) ; ce `mod.rs` détient le struct, le constructeur et les
//! réexports publics.

mod alias;
mod annonces;
mod bench;
mod case_names;
mod cases;
mod chunks;
mod citations;
mod corpus_counts;
mod decisions;
mod entities;
mod identity;
mod instrument_move;
mod jurisdiction_hubs;
mod legal_links;
mod legal_toc;
mod links;
mod llm_keys;
mod norm_hubs;
mod parties;
mod quantum;
mod referential;
mod rekey;
mod renvois;
mod scopes;
mod suggest;
mod support;
mod types;
mod usage_terms;

use crate::db::Connection;

pub use alias::{AliasCandidat, AliasRevue};
pub use annonces::{
    AnnonceAEnrichir, AnnonceAJumeler, AnnonceAxe, AnnonceCanalOuvert, AnnonceCanalRow,
    AnnonceChamps, AnnonceEntiteRow, AnnonceFiltre, AnnonceOrdre, AnnonceOrpheline, AnnonceRow,
    AnnonceSourceWriteItem, AnnonceWriteItem, AnnonceWriteStats, DetailPose, EntityAnnonceRow,
    ResultatPose, TableSize, TrouCouverture,
};
pub use bench::{ManualFields, ManualUpsert};
pub use cases::{CaseCitationWriteItem, ChronoCoverageRow};
pub use citations::CitationWriteItem;
pub use decisions::canonical_jurisdiction_code;
pub use entities::{
    AffiliationEdge, CnbEntityState, CnbEntityUpdate, EntityHistoryWriteItem, EntityWriteItem,
};
pub use instrument_move::TextRenameCounts;
pub use links::DecisionLinkWriteItem;
pub use parties::DecisionPartyWriteItem;
pub use quantum::{QuantumObservationRow, QuantumWriteItem};
pub use referential::TransitRecueil;
pub use rekey::StarConflict;
pub use renvois::{
    CaseNameMatch, ElectedCaseName, NamedDecision, Renvoi, RenvoiNomme, RenvoyantRow, TitleInputs,
};
pub use scopes::{ArticleScopeRow, CompteurArticleRow};
pub use suggest::SUGGEST_FST_KEY;
pub use support::source_from_source_uid;
pub use types::{
    ArticleBrowseFilters, ArticleCitationSpanRow, ArticleNeighborRow, ArticleRankHit, ArticleRrf,
    ArticleSearchRow, ArticleSearchStats, ArticleTitleMode, BulkDecisionWrite, CaseCitationRow,
    ChamberRow, ChunkWrite, CitationOccurrenceRow, CitingDecisionRow, CoCitedArticleRow,
    DecisionLinkRow, DecisionPartyReadRow, DecisionPartyRow, EntityContentieuxCounts,
    EntityCounselRow, EntityDecisionRow, EntityDenominationReadRow, EntityDirectoryRow,
    EntityHeaderRow, EntityJurisdictionCountRow, EntityYearCountRow, EuTwin, ExistingDecisionState,
    ExtractedFields, FacetCount, FacetValueRow, GtDoc, HubDecisionRow, InstrumentCandidate,
    InstrumentResolution, JurisdictionHubPages, JurisdictionHubRow, JurisdictionRow,
    LawCodeSummaryRow, LawVersionRow, LegalArticleRow, LegalLinkOwner, LegalLinkRow,
    LegalTextCatalogRow, LegalTextMeta, LegalTextRow, MissingSummaryRow, NormHubPages, NormTextRow,
    ResolvedLegalLink, SitemapRow, SlugSourceRow, TextCaseCitationRow, TextLegalCitationRow,
    TocArticleRow, TocEdgeRow, TocOwner, TocReadingRow, TocTreeRow, UpsertResult, UpsertStatus,
};

pub use support::REEXTRACTABLE_FIELDS;

/// Accès CRUD typé aux tables chunk-aware. Détient une connexion checkout du
/// pool ; le pipeline wrappe ses appels dans une transaction.
pub struct DecisionRepository<'a> {
    pub conn: &'a Connection,
}

impl<'a> DecisionRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }
}
