//! Runner de migrations idempotent.
//!
//! Les `.sql` sont embarqués via `include_str!` (pas de lecture fs au runtime).
//! Crée `schema_migrations`, applique les versions non-vues dans une transaction
//! unique chacune, lève si la base contient une version absente sur disque.
//!
//! **Sérialisation inter-process.** Plusieurs runners peuvent appeler
//! `apply_migrations` en même temps : toute l'application est préfixée par un
//! verrou consultatif de session. Le perdant attend, puis relit les versions et
//! ne trouve plus rien à faire.

use crate::db::Connection;
use crate::error::{Result, StoreError};

/// Une migration découverte (version + SQL embarqué).
#[derive(Debug, Clone)]
pub struct Migration {
    pub version: i32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Liste statique des migrations embarquées, triées par version croissante.
/// Source : `packages/lj-store/migrations/NNNN_*.sql`.
pub fn embedded_migrations() -> Vec<Migration> {
    vec![
        Migration {
            version: 1,
            name: "0001_init.sql",
            sql: include_str!("../migrations/0001_init.sql"),
        },
        Migration {
            version: 2,
            name: "0002_drop_decision_articles.sql",
            sql: include_str!("../migrations/0002_drop_decision_articles.sql"),
        },
        Migration {
            version: 3,
            name: "0003_add_source_xml_gzip.sql",
            sql: include_str!("../migrations/0003_add_source_xml_gzip.sql"),
        },
        Migration {
            version: 4,
            name: "0004_decisions_public_id.sql",
            sql: include_str!("../migrations/0004_decisions_public_id.sql"),
        },
        Migration {
            version: 5,
            name: "0005_drop_source_xml_size_columns.sql",
            sql: include_str!("../migrations/0005_drop_source_xml_size_columns.sql"),
        },
        Migration {
            version: 6,
            name: "0006_gzip_blob_nullable.sql",
            sql: include_str!("../migrations/0006_gzip_blob_nullable.sql"),
        },
        Migration {
            version: 7,
            name: "0007_drop_gzip_blob_columns.sql",
            sql: include_str!("../migrations/0007_drop_gzip_blob_columns.sql"),
        },
        Migration {
            version: 8,
            name: "0008_rename_source_xml_gzip.sql",
            sql: include_str!("../migrations/0008_rename_source_xml_gzip.sql"),
        },
        Migration {
            version: 9,
            name: "0009_decisions_extracted_fields.sql",
            sql: include_str!("../migrations/0009_decisions_extracted_fields.sql"),
        },
        Migration {
            version: 10,
            name: "0010_mcp_oauth_tables.sql",
            sql: include_str!("../migrations/0010_mcp_oauth_tables.sql"),
        },
        Migration {
            version: 11,
            name: "0011_semantic_keywords_tables.sql",
            sql: include_str!("../migrations/0011_semantic_keywords_tables.sql"),
        },
        Migration {
            version: 12,
            name: "0012_semantic_keyword_scores.sql",
            sql: include_str!("../migrations/0012_semantic_keyword_scores.sql"),
        },
        Migration {
            version: 13,
            name: "0013_decisions_jurisdiction_name.sql",
            sql: include_str!("../migrations/0013_decisions_jurisdiction_name.sql"),
        },
        Migration {
            version: 14,
            name: "0014_corpus_token_df_tables.sql",
            sql: include_str!("../migrations/0014_corpus_token_df_tables.sql"),
        },
        Migration {
            version: 15,
            name: "0015_decisions_search_title.sql",
            sql: include_str!("../migrations/0015_decisions_search_title.sql"),
        },
        Migration {
            version: 16,
            name: "0016_search_title_bm25_stopwords.sql",
            sql: include_str!("../migrations/0016_search_title_bm25_stopwords.sql"),
        },
        Migration {
            version: 17,
            name: "0017_drop_unused_tables.sql",
            sql: include_str!("../migrations/0017_drop_unused_tables.sql"),
        },
        Migration {
            version: 18,
            name: "0018_decisions_content_checksum.sql",
            sql: include_str!("../migrations/0018_decisions_content_checksum.sql"),
        },
        Migration {
            version: 19,
            name: "0019_full_text_payload_and_format.sql",
            sql: include_str!("../migrations/0019_full_text_payload_and_format.sql"),
        },
        Migration {
            version: 20,
            name: "0020_drop_chunks_search_title.sql",
            sql: include_str!("../migrations/0020_drop_chunks_search_title.sql"),
        },
        Migration {
            version: 21,
            name: "0021_search_title_bm25_regex_stopwords.sql",
            sql: include_str!("../migrations/0021_search_title_bm25_regex_stopwords.sql"),
        },
        Migration {
            version: 22,
            name: "0022_search_title_bm25_tokenizer_fix.sql",
            sql: include_str!("../migrations/0022_search_title_bm25_tokenizer_fix.sql"),
        },
        Migration {
            version: 23,
            name: "0023_chunks_embedding_rabitq8.sql",
            sql: include_str!("../migrations/0023_chunks_embedding_rabitq8.sql"),
        },
        Migration {
            version: 24,
            name: "0024_drop_lab_tables.sql",
            sql: include_str!("../migrations/0024_drop_lab_tables.sql"),
        },
        Migration {
            version: 25,
            name: "0025_search_title_bm25_stopword_a.sql",
            sql: include_str!("../migrations/0025_search_title_bm25_stopword_a.sql"),
        },
        Migration {
            version: 26,
            name: "0026_decision_chunks_denorm.sql",
            sql: include_str!("../migrations/0026_decision_chunks_denorm.sql"),
        },
        Migration {
            version: 27,
            name: "0027_legal_codes_3nf.sql",
            sql: include_str!("../migrations/0027_legal_codes_3nf.sql"),
        },
        Migration {
            version: 28,
            name: "0028_dlr_trigger_guard.sql",
            sql: include_str!("../migrations/0028_dlr_trigger_guard.sql"),
        },
        Migration {
            version: 29,
            name: "0029_dlr_trigger_statement_level.sql",
            sql: include_str!("../migrations/0029_dlr_trigger_statement_level.sql"),
        },
        Migration {
            version: 30,
            name: "0030_drop_docket_numbers_gin.sql",
            sql: include_str!("../migrations/0030_drop_docket_numbers_gin.sql"),
        },
        Migration {
            version: 31,
            name: "0031_payload_gzip_storage_external.sql",
            sql: include_str!("../migrations/0031_payload_gzip_storage_external.sql"),
        },
        Migration {
            version: 32,
            name: "0032_drop_decisions_content_hash.sql",
            sql: include_str!("../migrations/0032_drop_decisions_content_hash.sql"),
        },
        Migration {
            version: 33,
            name: "0033_chunks_vec_ivf_lists.sql",
            sql: include_str!("../migrations/0033_chunks_vec_ivf_lists.sql"),
        },
        Migration {
            version: 34,
            name: "0034_users_table.sql",
            sql: include_str!("../migrations/0034_users_table.sql"),
        },
        Migration {
            version: 35,
            name: "0035_user_bookmarks_table.sql",
            sql: include_str!("../migrations/0035_user_bookmarks_table.sql"),
        },
        Migration {
            version: 36,
            name: "0036_user_search_history_table.sql",
            sql: include_str!("../migrations/0036_user_search_history_table.sql"),
        },
        Migration {
            version: 37,
            name: "0037_mcp_clients_redirect_uris.sql",
            sql: include_str!("../migrations/0037_mcp_clients_redirect_uris.sql"),
        },
        Migration {
            version: 38,
            name: "0038_chunks_legal_article_labels.sql",
            sql: include_str!("../migrations/0038_chunks_legal_article_labels.sql"),
        },
        Migration {
            version: 39,
            name: "0039_decisions_summary.sql",
            sql: include_str!("../migrations/0039_decisions_summary.sql"),
        },
        Migration {
            version: 40,
            name: "0040_search_source_decision_views.sql",
            sql: include_str!("../migrations/0040_search_source_decision_views.sql"),
        },
        Migration {
            version: 41,
            name: "0041_publication_codes_array.sql",
            sql: include_str!("../migrations/0041_publication_codes_array.sql"),
        },
        Migration {
            version: 42,
            name: "0042_users_track_activity.sql",
            sql: include_str!("../migrations/0042_users_track_activity.sql"),
        },
        Migration {
            version: 43,
            name: "0043_mcp_user_fk.sql",
            sql: include_str!("../migrations/0043_mcp_user_fk.sql"),
        },
        Migration {
            version: 44,
            name: "0044_mcp_client_fk.sql",
            sql: include_str!("../migrations/0044_mcp_client_fk.sql"),
        },
        Migration {
            version: 45,
            name: "0045_sitemaps_table.sql",
            sql: include_str!("../migrations/0045_sitemaps_table.sql"),
        },
        Migration {
            version: 46,
            name: "0046_jurisdiction_name_literal.sql",
            sql: include_str!("../migrations/0046_jurisdiction_name_literal.sql"),
        },
        Migration {
            version: 47,
            name: "0047_chunks_legal_article_composite.sql",
            sql: include_str!("../migrations/0047_chunks_legal_article_composite.sql"),
        },
        Migration {
            version: 48,
            name: "0048_chunks_bm25_unique.sql",
            sql: include_str!("../migrations/0048_chunks_bm25_unique.sql"),
        },
        Migration {
            version: 49,
            name: "0049_legal_codes_canonical.sql",
            sql: include_str!("../migrations/0049_legal_codes_canonical.sql"),
        },
        Migration {
            version: 50,
            name: "0050_decisions_extract_version.sql",
            sql: include_str!("../migrations/0050_decisions_extract_version.sql"),
        },
        Migration {
            version: 51,
            name: "0051_full_text_source_fields_embed_ver.sql",
            sql: include_str!("../migrations/0051_full_text_source_fields_embed_ver.sql"),
        },
        Migration {
            version: 52,
            name: "0052_decisions_legal_arrays.sql",
            sql: include_str!("../migrations/0052_decisions_legal_arrays.sql"),
        },
        Migration {
            version: 53,
            name: "0053_decisions_bm25.sql",
            sql: include_str!("../migrations/0053_decisions_bm25.sql"),
        },
        Migration {
            version: 54,
            name: "0054_drop_chunks_bm25_body_payload.sql",
            sql: include_str!("../migrations/0054_drop_chunks_bm25_body_payload.sql"),
        },
        Migration {
            version: 55,
            name: "0055_decision_links.sql",
            sql: include_str!("../migrations/0055_decision_links.sql"),
        },
        Migration {
            version: 56,
            name: "0056_decision_sources_and_ecli.sql",
            sql: include_str!("../migrations/0056_decision_sources_and_ecli.sql"),
        },
        Migration {
            version: 57,
            name: "0057_legi_codes_and_articles.sql",
            sql: include_str!("../migrations/0057_legi_codes_and_articles.sql"),
        },
        Migration {
            version: 58,
            name: "0058_full_text_payload_format_html.sql",
            sql: include_str!("../migrations/0058_full_text_payload_format_html.sql"),
        },
        Migration {
            version: 59,
            name: "0059_full_text_payload_format_pdf.sql",
            sql: include_str!("../migrations/0059_full_text_payload_format_pdf.sql"),
        },
        Migration {
            version: 60,
            name: "0060_referential_multi_source.sql",
            sql: include_str!("../migrations/0060_referential_multi_source.sql"),
        },
        Migration {
            version: 61,
            name: "0061_multisource_provenance_pivot.sql",
            sql: include_str!("../migrations/0061_multisource_provenance_pivot.sql"),
        },
        Migration {
            version: 62,
            name: "0062_drop_decision_links.sql",
            sql: include_str!("../migrations/0062_drop_decision_links.sql"),
        },
        Migration {
            version: 63,
            name: "0063_drop_decisions_mono_source.sql",
            sql: include_str!("../migrations/0063_drop_decisions_mono_source.sql"),
        },
        Migration {
            version: 64,
            name: "0064_decision_sources_payload_format_pdf.sql",
            sql: include_str!("../migrations/0064_decision_sources_payload_format_pdf.sql"),
        },
        Migration {
            version: 65,
            name: "0065_rename_identity_key_to_canonical_ref.sql",
            sql: include_str!("../migrations/0065_rename_identity_key_to_canonical_ref.sql"),
        },
        Migration {
            version: 66,
            name: "0066_ambiguous_canonical_ref.sql",
            sql: include_str!("../migrations/0066_ambiguous_canonical_ref.sql"),
        },
        Migration {
            version: 67,
            name: "0067_canonical_ref_ambiguous_column.sql",
            sql: include_str!("../migrations/0067_canonical_ref_ambiguous_column.sql"),
        },
        Migration {
            version: 68,
            name: "0068_drop_canonical_ref_ambiguous.sql",
            sql: include_str!("../migrations/0068_drop_canonical_ref_ambiguous.sql"),
        },
        Migration {
            version: 69,
            name: "0069_ecli_non_unique.sql",
            sql: include_str!("../migrations/0069_ecli_non_unique.sql"),
        },
        Migration {
            version: 70,
            name: "0070_source_rank_generated.sql",
            sql: include_str!("../migrations/0070_source_rank_generated.sql"),
        },
        Migration {
            version: 71,
            name: "0071_payload_format_docx.sql",
            sql: include_str!("../migrations/0071_payload_format_docx.sql"),
        },
        Migration {
            version: 72,
            name: "0072_legal_text_legal_article.sql",
            sql: include_str!("../migrations/0072_legal_text_legal_article.sql"),
        },
        Migration {
            version: 73,
            name: "0073_cited_reference_decision_citation.sql",
            sql: include_str!("../migrations/0073_cited_reference_decision_citation.sql"),
        },
        Migration {
            version: 74,
            name: "0074_legal_text_title_key_trgm.sql",
            sql: include_str!("../migrations/0074_legal_text_title_key_trgm.sql"),
        },
        Migration {
            version: 75,
            name: "0075_drop_title_key_trgm.sql",
            sql: include_str!("../migrations/0075_drop_title_key_trgm.sql"),
        },
        Migration {
            version: 76,
            name: "0076_legal_arrays_from_cited_reference.sql",
            sql: include_str!("../migrations/0076_legal_arrays_from_cited_reference.sql"),
        },
        Migration {
            version: 77,
            name: "0077_drop_legacy_3nf_citations.sql",
            sql: include_str!("../migrations/0077_drop_legacy_3nf_citations.sql"),
        },
        Migration {
            version: 78,
            name: "0078_legal_article_bm25.sql",
            sql: include_str!("../migrations/0078_legal_article_bm25.sql"),
        },
        Migration {
            version: 79,
            name: "0079_legal_article_search_title.sql",
            sql: include_str!("../migrations/0079_legal_article_search_title.sql"),
        },
        Migration {
            version: 80,
            name: "0080_legal_text_identity_cascade.sql",
            sql: include_str!("../migrations/0080_legal_text_identity_cascade.sql"),
        },
        Migration {
            version: 81,
            name: "0081_decisions_bm25_en_stopwords.sql",
            sql: include_str!("../migrations/0081_decisions_bm25_en_stopwords.sql"),
        },
        Migration {
            version: 82,
            name: "0082_legal_article_lang_translation.sql",
            sql: include_str!("../migrations/0082_legal_article_lang_translation.sql"),
        },
        Migration {
            version: 83,
            name: "0083_full_text_toast_lz4.sql",
            sql: include_str!("../migrations/0083_full_text_toast_lz4.sql"),
        },
        Migration {
            version: 84,
            name: "0084_lang_rank_authority.sql",
            sql: include_str!("../migrations/0084_lang_rank_authority.sql"),
        },
        Migration {
            version: 85,
            name: "0085_legal_article_source_asof.sql",
            sql: include_str!("../migrations/0085_legal_article_source_asof.sql"),
        },
        Migration {
            version: 86,
            name: "0086_treaty_jorf_get_date.sql",
            sql: include_str!("../migrations/0086_treaty_jorf_get_date.sql"),
        },
        Migration {
            version: 87,
            name: "0087_legal_text_num_prefix_agnostic.sql",
            sql: include_str!("../migrations/0087_legal_text_num_prefix_agnostic.sql"),
        },
        Migration {
            version: 88,
            name: "0088_legal_text_facet_indexes.sql",
            sql: include_str!("../migrations/0088_legal_text_facet_indexes.sql"),
        },
        Migration {
            version: 89,
            name: "0089_citation_override_certified_version.sql",
            sql: include_str!("../migrations/0089_citation_override_certified_version.sql"),
        },
        Migration {
            version: 90,
            name: "0090_decision_citation_spans.sql",
            sql: include_str!("../migrations/0090_decision_citation_spans.sql"),
        },
        Migration {
            version: 91,
            name: "0091_legal_citation_direct_link.sql",
            sql: include_str!("../migrations/0091_legal_citation_direct_link.sql"),
        },
        Migration {
            version: 92,
            name: "0092_gold_annotation.sql",
            sql: include_str!("../migrations/0092_gold_annotation.sql"),
        },
        Migration {
            version: 93,
            name: "0093_decision_extraction_columns.sql",
            sql: include_str!("../migrations/0093_decision_extraction_columns.sql"),
        },
        Migration {
            version: 94,
            name: "0094_trim_extraction_columns.sql",
            sql: include_str!("../migrations/0094_trim_extraction_columns.sql"),
        },
        Migration {
            version: 95,
            name: "0095_drop_legal_citation_source.sql",
            sql: include_str!("../migrations/0095_drop_legal_citation_source.sql"),
        },
        Migration {
            version: 96,
            name: "0096_citation_model.sql",
            sql: include_str!("../migrations/0096_citation_model.sql"),
        },
        Migration {
            version: 97,
            name: "0097_citation_flat.sql",
            sql: include_str!("../migrations/0097_citation_flat.sql"),
        },
        Migration {
            version: 98,
            name: "0098_facet_arrays_from_legal_citation.sql",
            sql: include_str!("../migrations/0098_facet_arrays_from_legal_citation.sql"),
        },
        Migration {
            version: 99,
            name: "0099_drop_legacy_citation_tables.sql",
            sql: include_str!("../migrations/0099_drop_legacy_citation_tables.sql"),
        },
        Migration {
            version: 100,
            name: "0100_facet_value_jurisdiction_tables.sql",
            sql: include_str!("../migrations/0100_facet_value_jurisdiction_tables.sql"),
        },
        Migration {
            version: 101,
            name: "0101_facet_sync_functions_report_drift.sql",
            sql: include_str!("../migrations/0101_facet_sync_functions_report_drift.sql"),
        },
        Migration {
            version: 102,
            name: "0102_facet_juridiction_types.sql",
            sql: include_str!("../migrations/0102_facet_juridiction_types.sql"),
        },
        Migration {
            version: 103,
            name: "0103_gold_annotation_revive.sql",
            sql: include_str!("../migrations/0103_gold_annotation_revive.sql"),
        },
        Migration {
            version: 104,
            name: "0104_drop_gold_annotation.sql",
            sql: include_str!("../migrations/0104_drop_gold_annotation.sql"),
        },
        Migration {
            version: 105,
            name: "0105_add_served_lang_drop_lang_rank.sql",
            sql: include_str!("../migrations/0105_add_served_lang_drop_lang_rank.sql"),
        },
        Migration {
            version: 106,
            name: "0106_drop_served_lang_add_lang_column.sql",
            sql: include_str!("../migrations/0106_drop_served_lang_add_lang_column.sql"),
        },
        Migration {
            version: 107,
            name: "0107_drop_legacy_facet_columns.sql",
            sql: include_str!("../migrations/0107_drop_legacy_facet_columns.sql"),
        },
        Migration {
            version: 108,
            name: "0108_decision_themes.sql",
            sql: include_str!("../migrations/0108_decision_themes.sql"),
        },
        Migration {
            version: 109,
            name: "0109_sync_arrays_collate_c.sql",
            sql: include_str!("../migrations/0109_sync_arrays_collate_c.sql"),
        },
        Migration {
            version: 110,
            name: "0110_decision_links.sql",
            sql: include_str!("../migrations/0110_decision_links.sql"),
        },
        Migration {
            version: 111,
            name: "0111_legal_text_slug_unique.sql",
            sql: include_str!("../migrations/0111_legal_text_slug_unique.sql"),
        },
        Migration {
            version: 112,
            name: "0112_fix_jurisdiction_labels.sql",
            sql: include_str!("../migrations/0112_fix_jurisdiction_labels.sql"),
        },
        Migration {
            version: 113,
            name: "0113_case_citation.sql",
            sql: include_str!("../migrations/0113_case_citation.sql"),
        },
        Migration {
            version: 114,
            name: "0114_facet_portee_seed_publication_relabel.sql",
            sql: include_str!("../migrations/0114_facet_portee_seed_publication_relabel.sql"),
        },
        Migration {
            version: 115,
            name: "0115_gin_docket_fastupdate_off.sql",
            sql: include_str!("../migrations/0115_gin_docket_fastupdate_off.sql"),
        },
        Migration {
            version: 116,
            name: "0116_drop_unused_juridiction_facet_values.sql",
            sql: include_str!("../migrations/0116_drop_unused_juridiction_facet_values.sql"),
        },
        Migration {
            version: 117,
            name: "0117_decisions_chambre_formation_columns.sql",
            sql: include_str!("../migrations/0117_decisions_chambre_formation_columns.sql"),
        },
        Migration {
            version: 118,
            name: "0118_facet_chambre_terres_civi.sql",
            sql: include_str!("../migrations/0118_facet_chambre_terres_civi.sql"),
        },
        Migration {
            version: 119,
            name: "0119_search_title_drop_expression.sql",
            sql: include_str!("../migrations/0119_search_title_drop_expression.sql"),
        },
        Migration {
            version: 120,
            name: "0120_drop_legacy_title_columns.sql",
            sql: include_str!("../migrations/0120_drop_legacy_title_columns.sql"),
        },
        Migration {
            version: 121,
            name: "0121_jurisdiction_cassation_court_grain.sql",
            sql: include_str!("../migrations/0121_jurisdiction_cassation_court_grain.sql"),
        },
        Migration {
            version: 122,
            name: "0122_legal_link_graph.sql",
            sql: include_str!("../migrations/0122_legal_link_graph.sql"),
        },
        Migration {
            version: 123,
            name: "0123_legal_toc_edge.sql",
            sql: include_str!("../migrations/0123_legal_toc_edge.sql"),
        },
        Migration {
            version: 124,
            name: "0124_legal_text_upcoming_versions.sql",
            sql: include_str!("../migrations/0124_legal_text_upcoming_versions.sql"),
        },
        Migration {
            version: 125,
            name: "0125_entity_denomination_tables.sql",
            sql: include_str!("../migrations/0125_entity_denomination_tables.sql"),
        },
        Migration {
            version: 126,
            name: "0126_decisions_intervenors.sql",
            sql: include_str!("../migrations/0126_decisions_intervenors.sql"),
        },
        Migration {
            version: 127,
            name: "0127_decision_party.sql",
            sql: include_str!("../migrations/0127_decision_party.sql"),
        },
        Migration {
            version: 128,
            name: "0128_drop_office_juge_referes.sql",
            sql: include_str!("../migrations/0128_drop_office_juge_referes.sql"),
        },
        Migration {
            version: 129,
            name: "0129_juridiction_cnil_source_rank.sql",
            sql: include_str!("../migrations/0129_juridiction_cnil_source_rank.sql"),
        },
        Migration {
            version: 130,
            name: "0130_decision_party_evidence_columns.sql",
            sql: include_str!("../migrations/0130_decision_party_evidence_columns.sql"),
        },
        Migration {
            version: 131,
            name: "0131_decision_party_barreau.sql",
            sql: include_str!("../migrations/0131_decision_party_barreau.sql"),
        },
        Migration {
            version: 132,
            name: "0132_entity_contentieux.sql",
            sql: include_str!("../migrations/0132_entity_contentieux.sql"),
        },
        Migration {
            version: 133,
            name: "0133_decision_party_role.sql",
            sql: include_str!("../migrations/0133_decision_party_role.sql"),
        },
        Migration {
            version: 134,
            name: "0134_entity_surname_key.sql",
            sql: include_str!("../migrations/0134_entity_surname_key.sql"),
        },
        Migration {
            version: 135,
            name: "0135_registry_announcements.sql",
            sql: include_str!("../migrations/0135_registry_announcements.sql"),
        },
        Migration {
            version: 136,
            name: "0136_legal_text_body_case_citation.sql",
            sql: include_str!("../migrations/0136_legal_text_body_case_citation.sql"),
        },
        Migration {
            version: 137,
            name: "0137_jurisdiction_code_by_city.sql",
            sql: include_str!("../migrations/0137_jurisdiction_code_by_city.sql"),
        },
        Migration {
            version: 138,
            name: "0138_jurisdiction_referential_repair.sql",
            sql: include_str!("../migrations/0138_jurisdiction_referential_repair.sql"),
        },
        Migration {
            version: 139,
            name: "0139_drop_registry_announcements.sql",
            sql: include_str!("../migrations/0139_drop_registry_announcements.sql"),
        },
        Migration {
            version: 140,
            name: "0140_pg_trgm_extension.sql",
            sql: include_str!("../migrations/0140_pg_trgm_extension.sql"),
        },
        Migration {
            version: 141,
            name: "0141_legal_text_slug_cleanup.sql",
            sql: include_str!("../migrations/0141_legal_text_slug_cleanup.sql"),
        },
        Migration {
            version: 142,
            name: "0142_jurisdiction_type_rename.sql",
            sql: include_str!("../migrations/0142_jurisdiction_type_rename.sql"),
        },
        Migration {
            version: 143,
            name: "0143_search_history_jurisdiction_type_key.sql",
            sql: include_str!("../migrations/0143_search_history_jurisdiction_type_key.sql"),
        },
        Migration {
            version: 144,
            name: "0144_article_commentary.sql",
            sql: include_str!("../migrations/0144_article_commentary.sql"),
        },
        Migration {
            version: 145,
            name: "0145_facet_value_axis_rename.sql",
            sql: include_str!("../migrations/0145_facet_value_axis_rename.sql"),
        },
        Migration {
            version: 146,
            name: "0146_suggest_index.sql",
            sql: include_str!("../migrations/0146_suggest_index.sql"),
        },
        Migration {
            version: 147,
            name: "0147_text_legal_citation.sql",
            sql: include_str!("../migrations/0147_text_legal_citation.sql"),
        },
        Migration {
            version: 148,
            name: "0148_purge_legal_text_version_shells.sql",
            sql: include_str!("../migrations/0148_purge_legal_text_version_shells.sql"),
        },
        Migration {
            version: 149,
            name: "0149_legal_citation_family_expansion.sql",
            sql: include_str!("../migrations/0149_legal_citation_family_expansion.sql"),
        },
        Migration {
            version: 150,
            name: "0150_directory_registry_totals.sql",
            sql: include_str!("../migrations/0150_directory_registry_totals.sql"),
        },
        Migration {
            version: 151,
            name: "0151_legal_citation_family_public_keys.sql",
            sql: include_str!("../migrations/0151_legal_citation_family_public_keys.sql"),
        },
        Migration {
            version: 152,
            name: "0152_mcp_refresh_tokens.sql",
            sql: include_str!("../migrations/0152_mcp_refresh_tokens.sql"),
        },
        Migration {
            version: 153,
            name: "0153_entity_directory_complete.sql",
            sql: include_str!("../migrations/0153_entity_directory_complete.sql"),
        },
        Migration {
            version: 154,
            name: "0154_entity_search_litigation_index.sql",
            sql: include_str!("../migrations/0154_entity_search_litigation_index.sql"),
        },
        Migration {
            version: 155,
            name: "0155_entity_single_table_fold_index.sql",
            sql: include_str!("../migrations/0155_entity_single_table_fold_index.sql"),
        },
        Migration {
            version: 156,
            name: "0156_mistral_key_status.sql",
            sql: include_str!("../migrations/0156_mistral_key_status.sql"),
        },
        Migration {
            version: 157,
            name: "0157_legal_text_role_alias_of.sql",
            sql: include_str!("../migrations/0157_legal_text_role_alias_of.sql"),
        },
        Migration {
            version: 158,
            name: "0158_legal_citation_jsonb_per_decision.sql",
            sql: include_str!("../migrations/0158_legal_citation_jsonb_per_decision.sql"),
        },
        Migration {
            version: 159,
            name: "0159_legal_article_usage_terms.sql",
            sql: include_str!("../migrations/0159_legal_article_usage_terms.sql"),
        },
        Migration {
            version: 160,
            name: "0160_legal_article_usage_table.sql",
            sql: include_str!("../migrations/0160_legal_article_usage_table.sql"),
        },
        Migration {
            version: 161,
            name: "0161_drop_legal_text_alias_of.sql",
            sql: include_str!("../migrations/0161_drop_legal_text_alias_of.sql"),
        },
        Migration {
            version: 162,
            name: "0162_citing_decision_counts.sql",
            sql: include_str!("../migrations/0162_citing_decision_counts.sql"),
        },
        Migration {
            version: 163,
            name: "0163_user_search_history_engine.sql",
            sql: include_str!("../migrations/0163_user_search_history_engine.sql"),
        },
        Migration {
            version: 164,
            name: "0164_decisions_jurisdiction_code_date_idx.sql",
            sql: include_str!("../migrations/0164_decisions_jurisdiction_code_date_idx.sql"),
        },
        Migration {
            version: 165,
            name: "0165_legal_article_denorm_search_columns.sql",
            sql: include_str!("../migrations/0165_legal_article_denorm_search_columns.sql"),
        },
        Migration {
            version: 166,
            name: "0166_legal_text_norm_browse_index.sql",
            sql: include_str!("../migrations/0166_legal_text_norm_browse_index.sql"),
        },
        Migration {
            version: 167,
            name: "0167_decisions_jur_code_date_partial_idx.sql",
            sql: include_str!("../migrations/0167_decisions_jur_code_date_partial_idx.sql"),
        },
        Migration {
            version: 168,
            name: "0168_legal_text_curated_flag.sql",
            sql: include_str!("../migrations/0168_legal_text_curated_flag.sql"),
        },
        Migration {
            version: 169,
            name: "0169_legal_article_usage_text_uid_idx.sql",
            sql: include_str!("../migrations/0169_legal_article_usage_text_uid_idx.sql"),
        },
        Migration {
            version: 170,
            name: "0170_entity_folded_gin_fastupdate_off.sql",
            sql: include_str!("../migrations/0170_entity_folded_gin_fastupdate_off.sql"),
        },
        Migration {
            version: 171,
            name: "0171_legal_citation_terms_gin_fastupdate_off.sql",
            sql: include_str!("../migrations/0171_legal_citation_terms_gin_fastupdate_off.sql"),
        },
        Migration {
            version: 172,
            name: "0172_decision_chunks_gin_fastupdate_off.sql",
            sql: include_str!("../migrations/0172_decision_chunks_gin_fastupdate_off.sql"),
        },
        Migration {
            version: 173,
            name: "0173_legal_citation_autovacuum_tuning.sql",
            sql: include_str!("../migrations/0173_legal_citation_autovacuum_tuning.sql"),
        },
        Migration {
            version: 174,
            name: "0174_article_commentary_slug_anchor.sql",
            sql: include_str!("../migrations/0174_article_commentary_slug_anchor.sql"),
        },
        Migration {
            version: 175,
            name: "0175_article_commentary_drop_text_uid.sql",
            sql: include_str!("../migrations/0175_article_commentary_drop_text_uid.sql"),
        },
        Migration {
            version: 176,
            name: "0176_entity_uid_pattern_idx.sql",
            sql: include_str!("../migrations/0176_entity_uid_pattern_idx.sql"),
        },
        Migration {
            version: 177,
            name: "0177_legal_article_reading_rank.sql",
            sql: include_str!("../migrations/0177_legal_article_reading_rank.sql"),
        },
        Migration {
            version: 178,
            name: "0178_entity_activity_affiliation.sql",
            sql: include_str!("../migrations/0178_entity_activity_affiliation.sql"),
        },
        Migration {
            version: 179,
            name: "0179_public_procurement_notices.sql",
            sql: include_str!("../migrations/0179_public_procurement_notices.sql"),
        },
        Migration {
            version: 180,
            name: "0180_legal_citation_family_by_date.sql",
            sql: include_str!("../migrations/0180_legal_citation_family_by_date.sql"),
        },
        Migration {
            version: 181,
            name: "0181_decisions_updated_at_content_date.sql",
            sql: include_str!("../migrations/0181_decisions_updated_at_content_date.sql"),
        },
        Migration {
            version: 182,
            name: "0182_decision_party_firm_ord.sql",
            sql: include_str!("../migrations/0182_decision_party_firm_ord.sql"),
        },
        Migration {
            version: 183,
            name: "0183_drop_legal_text_curated.sql",
            sql: include_str!("../migrations/0183_drop_legal_text_curated.sql"),
        },
        Migration {
            version: 184,
            name: "0184_procurement_notice_stream_idx.sql",
            sql: include_str!("../migrations/0184_procurement_notice_stream_idx.sql"),
        },
        Migration {
            version: 185,
            name: "0185_legal_text_bm25_v2_facet_fields.sql",
            sql: include_str!("../migrations/0185_legal_text_bm25_v2_facet_fields.sql"),
        },
        Migration {
            version: 186,
            name: "0186_legal_article_kind.sql",
            sql: include_str!("../migrations/0186_legal_article_kind.sql"),
        },
        Migration {
            version: 187,
            name: "0187_legal_article_bm25_v2_validity_window.sql",
            sql: include_str!("../migrations/0187_legal_article_bm25_v2_validity_window.sql"),
        },
        Migration {
            version: 188,
            name: "0188_legal_text_bm25_index_swap.sql",
            sql: include_str!("../migrations/0188_legal_text_bm25_index_swap.sql"),
        },
        Migration {
            version: 189,
            name: "0189_legal_article_bm25_index_swap.sql",
            sql: include_str!("../migrations/0189_legal_article_bm25_index_swap.sql"),
        },
        Migration {
            version: 190,
            name: "0190_decision_sources_drop_id.sql",
            sql: include_str!("../migrations/0190_decision_sources_drop_id.sql"),
        },
        Migration {
            version: 191,
            name: "0191_notice_rename_actor_drop_stale.sql",
            sql: include_str!("../migrations/0191_notice_rename_actor_drop_stale.sql"),
        },
        Migration {
            version: 192,
            name: "0192_legal_text_bm25_v2_text_uid.sql",
            sql: include_str!("../migrations/0192_legal_text_bm25_v2_text_uid.sql"),
        },
        Migration {
            version: 193,
            name: "0193_legal_text_bm25_text_uid_swap.sql",
            sql: include_str!("../migrations/0193_legal_text_bm25_text_uid_swap.sql"),
        },
        Migration {
            version: 194,
            name: "0194_notice_source_drop_checksum.sql",
            sql: include_str!("../migrations/0194_notice_source_drop_checksum.sql"),
        },
        Migration {
            version: 195,
            name: "0195_mcp_tokens_sha256_hashing.sql",
            sql: include_str!("../migrations/0195_mcp_tokens_sha256_hashing.sql"),
        },
        Migration {
            version: 196,
            name: "0196_users_sessions_valid_from.sql",
            sql: include_str!("../migrations/0196_users_sessions_valid_from.sql"),
        },
        Migration {
            version: 197,
            name: "0197_notice_title_actor_trgm_idx.sql",
            sql: include_str!("../migrations/0197_notice_title_actor_trgm_idx.sql"),
        },
        Migration {
            version: 198,
            name: "0198_ingest_cursor.sql",
            sql: include_str!("../migrations/0198_ingest_cursor.sql"),
        },
        Migration {
            version: 199,
            name: "0199_drop_ingest_cursor.sql",
            sql: include_str!("../migrations/0199_drop_ingest_cursor.sql"),
        },
        Migration {
            version: 200,
            name: "0200_drop_decision_sources_active_idx.sql",
            sql: include_str!("../migrations/0200_drop_decision_sources_active_idx.sql"),
        },
        Migration {
            version: 201,
            name: "0201_notice_redirect.sql",
            sql: include_str!("../migrations/0201_notice_redirect.sql"),
        },
        Migration {
            version: 202,
            name: "0202_mcp_tokens_drop_plaintext.sql",
            sql: include_str!("../migrations/0202_mcp_tokens_drop_plaintext.sql"),
        },
        Migration {
            version: 203,
            name: "0203_article_scope_and_geo_facets.sql",
            sql: include_str!("../migrations/0203_article_scope_and_geo_facets.sql"),
        },
        Migration {
            version: 204,
            name: "0204_text_filiation.sql",
            sql: include_str!("../migrations/0204_text_filiation.sql"),
        },
        Migration {
            version: 205,
            name: "0205_scope_membership_default_scope.sql",
            sql: include_str!("../migrations/0205_scope_membership_default_scope.sql"),
        },
        Migration {
            version: 206,
            name: "0206_legal_article_usage_bm25_v2_num_key.sql",
            sql: include_str!("../migrations/0206_legal_article_usage_bm25_v2_num_key.sql"),
        },
        Migration {
            version: 207,
            name: "0207_legal_article_usage_bm25_swap.sql",
            sql: include_str!("../migrations/0207_legal_article_usage_bm25_swap.sql"),
        },
        Migration {
            version: 208,
            name: "0208_decisions_public_id_check_not_valid.sql",
            sql: include_str!("../migrations/0208_decisions_public_id_check_not_valid.sql"),
        },
        Migration {
            version: 209,
            name: "0209_decisions_public_id_not_null.sql",
            sql: include_str!("../migrations/0209_decisions_public_id_not_null.sql"),
        },
        Migration {
            version: 210,
            name: "0210_auth_password_sessions_email_tokens.sql",
            sql: include_str!("../migrations/0210_auth_password_sessions_email_tokens.sql"),
        },
        Migration {
            version: 211,
            name: "0211_lj_docket_keys_function.sql",
            sql: include_str!("../migrations/0211_lj_docket_keys_function.sql"),
        },
        Migration {
            version: 212,
            name: "0212_idx_decisions_docket_keys_gin.sql",
            sql: include_str!("../migrations/0212_idx_decisions_docket_keys_gin.sql"),
        },
        Migration {
            version: 213,
            name: "0213_decisions_bm25_number_tokenizer.sql",
            sql: include_str!("../migrations/0213_decisions_bm25_number_tokenizer.sql"),
        },
        Migration {
            version: 214,
            name: "0214_decisions_bm25_swap_v2.sql",
            sql: include_str!("../migrations/0214_decisions_bm25_swap_v2.sql"),
        },
        Migration {
            version: 215,
            name: "0215_legal_text_slug_redirect.sql",
            sql: include_str!("../migrations/0215_legal_text_slug_redirect.sql"),
        },
        Migration {
            version: 216,
            name: "0216_code_slugs_drop_artefacts.sql",
            sql: include_str!("../migrations/0216_code_slugs_drop_artefacts.sql"),
        },
        Migration {
            version: 217,
            name: "0217_user_identities_and_email_change.sql",
            sql: include_str!("../migrations/0217_user_identities_and_email_change.sql"),
        },
        Migration {
            version: 218,
            name: "0218_legal_article_bm25_int_tokenizer.sql",
            sql: include_str!("../migrations/0218_legal_article_bm25_int_tokenizer.sql"),
        },
        Migration {
            version: 219,
            name: "0219_legal_text_body_bm25_int_tokenizer.sql",
            sql: include_str!("../migrations/0219_legal_text_body_bm25_int_tokenizer.sql"),
        },
        Migration {
            version: 220,
            name: "0220_legal_article_usage_bm25_int_tokenizer.sql",
            sql: include_str!("../migrations/0220_legal_article_usage_bm25_int_tokenizer.sql"),
        },
        Migration {
            version: 221,
            name: "0221_legal_text_engine_bm25_swap.sql",
            sql: include_str!("../migrations/0221_legal_text_engine_bm25_swap.sql"),
        },
        Migration {
            version: 222,
            name: "0222_drop_users_google_sub.sql",
            sql: include_str!("../migrations/0222_drop_users_google_sub.sql"),
        },
        Migration {
            version: 223,
            name: "0223_legal_text_norm_browse_idx.sql",
            sql: include_str!("../migrations/0223_legal_text_norm_browse_idx.sql"),
        },
        Migration {
            version: 224,
            name: "0224_jurisdiction_label_overseas_fix.sql",
            sql: include_str!("../migrations/0224_jurisdiction_label_overseas_fix.sql"),
        },
        Migration {
            version: 225,
            name: "0225_jurisdiction_year_counts.sql",
            sql: include_str!("../migrations/0225_jurisdiction_year_counts.sql"),
        },
        Migration {
            version: 226,
            name: "0226_idx_decisions_jur_code_date_v2.sql",
            sql: include_str!("../migrations/0226_idx_decisions_jur_code_date_v2.sql"),
        },
        Migration {
            version: 227,
            name: "0227_idx_decisions_jur_code_date_swap.sql",
            sql: include_str!("../migrations/0227_idx_decisions_jur_code_date_swap.sql"),
        },
        Migration {
            version: 228,
            name: "0228_legal_article_search_title_star.sql",
            sql: include_str!("../migrations/0228_legal_article_search_title_star.sql"),
        },
        Migration {
            version: 229,
            name: "0229_facet_value_hint_and_publication.sql",
            sql: include_str!("../migrations/0229_facet_value_hint_and_publication.sql"),
        },
        Migration {
            version: 230,
            name: "0230_facet_value_publication_sort.sql",
            sql: include_str!("../migrations/0230_facet_value_publication_sort.sql"),
        },
        Migration {
            version: 231,
            name: "0231_decision_facet_counts.sql",
            sql: include_str!("../migrations/0231_decision_facet_counts.sql"),
        },
        Migration {
            version: 232,
            name: "0232_idx_decisions_date_lecture_actives.sql",
            sql: include_str!("../migrations/0232_idx_decisions_date_lecture_actives.sql"),
        },
        Migration {
            version: 233,
            name: "0233_drop_decisions_publication_uid.sql",
            sql: include_str!("../migrations/0233_drop_decisions_publication_uid.sql"),
        },
        Migration {
            version: 234,
            name: "0234_decisions_add_chamber_key.sql",
            sql: include_str!("../migrations/0234_decisions_add_chamber_key.sql"),
        },
        Migration {
            version: 235,
            name: "0235_decision_quantum_tables.sql",
            sql: include_str!("../migrations/0235_decision_quantum_tables.sql"),
        },
        Migration {
            version: 236,
            name: "0236_decisions_add_matiere_uid.sql",
            sql: include_str!("../migrations/0236_decisions_add_matiere_uid.sql"),
        },
        Migration {
            version: 237,
            name: "0237_quantum_observation_table.sql",
            sql: include_str!("../migrations/0237_quantum_observation_table.sql"),
        },
        Migration {
            version: 238,
            name: "0238_alias_candidate_table.sql",
            sql: include_str!("../migrations/0238_alias_candidate_table.sql"),
        },
        Migration {
            version: 239,
            name: "0239_drop_decisions_chamber_position_uid.sql",
            sql: include_str!("../migrations/0239_drop_decisions_chamber_position_uid.sql"),
        },
        Migration {
            version: 240,
            name: "0240_quantum_observation_add_deces.sql",
            sql: include_str!("../migrations/0240_quantum_observation_add_deces.sql"),
        },
        Migration {
            version: 241,
            name: "0241_quantum_observation_add_comparabilite.sql",
            sql: include_str!("../migrations/0241_quantum_observation_add_comparabilite.sql"),
        },
        Migration {
            version: 242,
            name: "0242_quantum_observation_categorie_manque.sql",
            sql: include_str!("../migrations/0242_quantum_observation_categorie_manque.sql"),
        },
        Migration {
            version: 243,
            name: "0243_chamber_table.sql",
            sql: include_str!("../migrations/0243_chamber_table.sql"),
        },
        Migration {
            version: 244,
            name: "0244_decision_renvoi_table.sql",
            sql: include_str!("../migrations/0244_decision_renvoi_table.sql"),
        },
        Migration {
            version: 245,
            name: "0245_decisions_bm25_v3_facet_axes.sql",
            sql: include_str!("../migrations/0245_decisions_bm25_v3_facet_axes.sql"),
        },
        Migration {
            version: 246,
            name: "0246_decisions_bm25_v3_swap.sql",
            sql: include_str!("../migrations/0246_decisions_bm25_v3_swap.sql"),
        },
        Migration {
            version: 247,
            name: "0247_decision_sources_rank_rebuild.sql",
            sql: include_str!("../migrations/0247_decision_sources_rank_rebuild.sql"),
        },
        Migration {
            version: 248,
            name: "0248_move_account_tables_to_comptes_schema.sql",
            sql: include_str!("../migrations/0248_move_account_tables_to_comptes_schema.sql"),
        },
        Migration {
            version: 249,
            name: "0249_decisions_bm25_v4_procedure_uid.sql",
            sql: include_str!("../migrations/0249_decisions_bm25_v4_procedure_uid.sql"),
        },
        Migration {
            version: 250,
            name: "0250_decisions_bm25_v4_swap.sql",
            sql: include_str!("../migrations/0250_decisions_bm25_v4_swap.sql"),
        },
        Migration {
            version: 251,
            name: "0251_decisions_add_case_name.sql",
            sql: include_str!("../migrations/0251_decisions_add_case_name.sql"),
        },
        Migration {
            version: 252,
            name: "0252_docket_keys_index_fastupdate_off.sql",
            sql: include_str!("../migrations/0252_docket_keys_index_fastupdate_off.sql"),
        },
        Migration {
            version: 253,
            name: "0253_decision_sources_rank_cnil_fix.sql",
            sql: include_str!("../migrations/0253_decision_sources_rank_cnil_fix.sql"),
        },
        Migration {
            version: 254,
            name: "0254_facet_value_matiere_parent_uid.sql",
            sql: include_str!("../migrations/0254_facet_value_matiere_parent_uid.sql"),
        },
        Migration {
            version: 255,
            name: "0255_idx_decisions_notable.sql",
            sql: include_str!("../migrations/0255_idx_decisions_notable.sql"),
        },
        Migration {
            version: 256,
            name: "0256_decisions_case_name_source.sql",
            sql: include_str!("../migrations/0256_decisions_case_name_source.sql"),
        },
        Migration {
            version: 257,
            name: "0257_idx_decisions_case_name_source.sql",
            sql: include_str!("../migrations/0257_idx_decisions_case_name_source.sql"),
        },
        Migration {
            version: 258,
            name: "0258_chamber_add_speciality_uid.sql",
            sql: include_str!("../migrations/0258_chamber_add_speciality_uid.sql"),
        },
        Migration {
            version: 259,
            name: "0259_lj_party_fold_function.sql",
            sql: include_str!("../migrations/0259_lj_party_fold_function.sql"),
        },
        Migration {
            version: 260,
            name: "0260_decision_party_representation_edge.sql",
            sql: include_str!("../migrations/0260_decision_party_representation_edge.sql"),
        },
        Migration {
            version: 261,
            name: "0261_decisions_bm25_v5_drop_matiere.sql",
            sql: include_str!("../migrations/0261_decisions_bm25_v5_drop_matiere.sql"),
        },
        Migration {
            version: 262,
            name: "0262_decisions_bm25_v5_swap.sql",
            sql: include_str!("../migrations/0262_decisions_bm25_v5_swap.sql"),
        },
        Migration {
            version: 263,
            name: "0263_drop_decisions_matiere_uid.sql",
            sql: include_str!("../migrations/0263_drop_decisions_matiere_uid.sql"),
        },
        Migration {
            version: 264,
            name: "0264_comptes_desabonnements_table.sql",
            sql: include_str!("../migrations/0264_comptes_desabonnements_table.sql"),
        },
        Migration {
            version: 265,
            name: "0265_users_add_locale_columns.sql",
            sql: include_str!("../migrations/0265_users_add_locale_columns.sql"),
        },
        Migration {
            version: 266,
            name: "0266_auth_email_tokens_purpose_login.sql",
            sql: include_str!("../migrations/0266_auth_email_tokens_purpose_login.sql"),
        },
        Migration {
            version: 267,
            name: "0267_rename_desabonnements_to_unsubscribes.sql",
            sql: include_str!("../migrations/0267_rename_desabonnements_to_unsubscribes.sql"),
        },
        Migration {
            version: 268,
            name: "0268_facet_value_jurisdiction_type_sort.sql",
            sql: include_str!("../migrations/0268_facet_value_jurisdiction_type_sort.sql"),
        },
        Migration {
            version: 269,
            name: "0269_facet_value_publication_hints.sql",
            sql: include_str!("../migrations/0269_facet_value_publication_hints.sql"),
        },
        Migration {
            version: 270,
            name: "0270_facet_value_matiere_labels.sql",
            sql: include_str!("../migrations/0270_facet_value_matiere_labels.sql"),
        },
        Migration {
            version: 271,
            name: "0271_legal_text_base_piece_uid.sql",
            sql: include_str!("../migrations/0271_legal_text_base_piece_uid.sql"),
        },
        Migration {
            version: 272,
            name: "0272_profil_acheteur_echecs_consecutifs.sql",
            sql: include_str!("../migrations/0272_profil_acheteur_echecs_consecutifs.sql"),
        },
        Migration {
            version: 273,
            name: "0273_chamber_label_nullable_truncate.sql",
            sql: include_str!("../migrations/0273_chamber_label_nullable_truncate.sql"),
        },
        Migration {
            version: 274,
            name: "0274_chamber_label_default_empty.sql",
            sql: include_str!("../migrations/0274_chamber_label_default_empty.sql"),
        },
        Migration {
            version: 275,
            name: "0275_corpus_counts_rename_and_seed.sql",
            sql: include_str!("../migrations/0275_corpus_counts_rename_and_seed.sql"),
        },
        Migration {
            version: 276,
            name: "0276_chamber_drop_label.sql",
            sql: include_str!("../migrations/0276_chamber_drop_label.sql"),
        },
        Migration {
            version: 277,
            name: "0277_api_keys.sql",
            sql: include_str!("../migrations/0277_api_keys.sql"),
        },
        Migration {
            version: 278,
            name: "0278_drop_decision_facet_counts_view.sql",
            sql: include_str!("../migrations/0278_drop_decision_facet_counts_view.sql"),
        },
        Migration {
            version: 279,
            name: "0279_drop_unsubscribes.sql",
            sql: include_str!("../migrations/0279_drop_unsubscribes.sql"),
        },
        Migration {
            version: 280,
            name: "0280_chamber_drop_speciality_uid.sql",
            sql: include_str!("../migrations/0280_chamber_drop_speciality_uid.sql"),
        },
        Migration {
            version: 281,
            name: "0281_facet_value_chambre_du_conseil.sql",
            sql: include_str!("../migrations/0281_facet_value_chambre_du_conseil.sql"),
        },
        Migration {
            version: 282,
            name: "0282_decisions_seat_and_seat_paths.sql",
            sql: include_str!("../migrations/0282_decisions_seat_and_seat_paths.sql"),
        },
        Migration {
            version: 283,
            name: "0283_quantum_preuve_equations_verdict.sql",
            sql: include_str!("../migrations/0283_quantum_preuve_equations_verdict.sql"),
        },
        Migration {
            version: 284,
            name: "0284_quantum_observation_one_row_per_decision.sql",
            sql: include_str!("../migrations/0284_quantum_observation_one_row_per_decision.sql"),
        },
        Migration {
            version: 285,
            name: "0285_quantum_observation_origine_indemnites.sql",
            sql: include_str!("../migrations/0285_quantum_observation_origine_indemnites.sql"),
        },
        Migration {
            version: 286,
            name: "0286_decisions_bm25_v6_seat_paths.sql",
            sql: include_str!("../migrations/0286_decisions_bm25_v6_seat_paths.sql"),
        },
        Migration {
            version: 287,
            name: "0287_decisions_seat_paths_gin_index.sql",
            sql: include_str!("../migrations/0287_decisions_seat_paths_gin_index.sql"),
        },
        Migration {
            version: 288,
            name: "0288_user_search_history_chamber_to_seat.sql",
            sql: include_str!("../migrations/0288_user_search_history_chamber_to_seat.sql"),
        },
        Migration {
            version: 289,
            name: "0289_decisions_bm25_swap_seat_paths.sql",
            sql: include_str!("../migrations/0289_decisions_bm25_swap_seat_paths.sql"),
        },
        Migration {
            version: 290,
            name: "0290_drop_chamber_key_and_chamber_table.sql",
            sql: include_str!("../migrations/0290_drop_chamber_key_and_chamber_table.sql"),
        },
        Migration {
            version: 291,
            name: "0291_decisions_bm25_v7_docket_keys.sql",
            sql: include_str!("../migrations/0291_decisions_bm25_v7_docket_keys.sql"),
        },
        Migration {
            version: 292,
            name: "0292_decisions_bm25_swap_docket_keys.sql",
            sql: include_str!("../migrations/0292_decisions_bm25_swap_docket_keys.sql"),
        },
        Migration {
            version: 293,
            name: "0293_decisions_summary_engine.sql",
            sql: include_str!("../migrations/0293_decisions_summary_engine.sql"),
        },
        Migration {
            version: 294,
            name: "0294_gemini_key_day_quota.sql",
            sql: include_str!("../migrations/0294_gemini_key_day_quota.sql"),
        },
        Migration {
            version: 295,
            name: "0295_annonce_lien_sonde.sql",
            sql: include_str!("../migrations/0295_annonce_lien_sonde.sql"),
        },
        Migration {
            version: 296,
            name: "0296_legal_article_kind_generated.sql",
            sql: include_str!("../migrations/0296_legal_article_kind_generated.sql"),
        },
        Migration {
            version: 297,
            name: "0297_annonce_zones_array.sql",
            sql: include_str!("../migrations/0297_annonce_zones_array.sql"),
        },
        Migration {
            version: 298,
            name: "0298_disable_aggregate_custom_scan.sql",
            sql: include_str!("../migrations/0298_disable_aggregate_custom_scan.sql"),
        },
        Migration {
            version: 299,
            name: "0299_annonce_bm25_index.sql",
            sql: include_str!("../migrations/0299_annonce_bm25_index.sql"),
        },
        Migration {
            version: 300,
            name: "0300_annonce_description_column.sql",
            sql: include_str!("../migrations/0300_annonce_description_column.sql"),
        },
        Migration {
            version: 301,
            name: "0301_journal_wording_assertion_tables.sql",
            sql: include_str!("../migrations/0301_journal_wording_assertion_tables.sql"),
        },
        Migration {
            version: 302,
            name: "0302_users_mcp_connected_at.sql",
            sql: include_str!("../migrations/0302_users_mcp_connected_at.sql"),
        },
        Migration {
            version: 303,
            name: "0303_assertion_text_uid_foreign_keys.sql",
            sql: include_str!("../migrations/0303_assertion_text_uid_foreign_keys.sql"),
        },
        Migration {
            version: 304,
            name: "0304_quantum_observation_composante_tranche.sql",
            sql: include_str!("../migrations/0304_quantum_observation_composante_tranche.sql"),
        },
        Migration {
            version: 305,
            name: "0305_text_place_coverage_table.sql",
            sql: include_str!("../migrations/0305_text_place_coverage_table.sql"),
        },
        Migration {
            version: 306,
            name: "0306_text_place_coverage_journal_nature.sql",
            sql: include_str!("../migrations/0306_text_place_coverage_journal_nature.sql"),
        },
        Migration {
            version: 307,
            name: "0307_llm_credential_table.sql",
            sql: include_str!("../migrations/0307_llm_credential_table.sql"),
        },
        Migration {
            version: 308,
            name: "0308_llm_credential_label_to_tenant.sql",
            sql: include_str!("../migrations/0308_llm_credential_label_to_tenant.sql"),
        },
        Migration {
            version: 309,
            name: "0309_llm_credential_account_column.sql",
            sql: include_str!("../migrations/0309_llm_credential_account_column.sql"),
        },
        Migration {
            version: 310,
            name: "0310_llm_credential_schema_comptes.sql",
            sql: include_str!("../migrations/0310_llm_credential_schema_comptes.sql"),
        },
        Migration {
            version: 311,
            name: "0311_drop_mistral_key_status.sql",
            sql: include_str!("../migrations/0311_drop_mistral_key_status.sql"),
        },
        Migration {
            version: 312,
            name: "0312_legal_text_slug_jurisdiction_unique.sql",
            sql: include_str!("../migrations/0312_legal_text_slug_jurisdiction_unique.sql"),
        },
        Migration {
            version: 313,
            name: "0313_annonce_metiers_array.sql",
            sql: include_str!("../migrations/0313_annonce_metiers_array.sql"),
        },
        Migration {
            version: 314,
            name: "0314_drop_legal_text_slug_index.sql",
            sql: include_str!("../migrations/0314_drop_legal_text_slug_index.sql"),
        },
        Migration {
            version: 315,
            name: "0315_jurisdiction_place_column.sql",
            sql: include_str!("../migrations/0315_jurisdiction_place_column.sql"),
        },
        Migration {
            version: 316,
            name: "0316_llm_credential_priority_column.sql",
            sql: include_str!("../migrations/0316_llm_credential_priority_column.sql"),
        },
        Migration {
            version: 317,
            name: "0317_pending_target_court_docket_date.sql",
            sql: include_str!("../migrations/0317_pending_target_court_docket_date.sql"),
        },
        Migration {
            version: 318,
            name: "0318_llm_credential_purpose_nullable.sql",
            sql: include_str!("../migrations/0318_llm_credential_purpose_nullable.sql"),
        },
        Migration {
            version: 319,
            name: "0319_drop_llm_credential_purpose.sql",
            sql: include_str!("../migrations/0319_drop_llm_credential_purpose.sql"),
        },
        Migration {
            version: 320,
            name: "0320_users_browser_languages_country.sql",
            sql: include_str!("../migrations/0320_users_browser_languages_country.sql"),
        },
        Migration {
            version: 321,
            name: "0321_lj_docket_search_keys_function.sql",
            sql: include_str!("../migrations/0321_lj_docket_search_keys_function.sql"),
        },
        Migration {
            version: 322,
            name: "0322_lj_docket_search_keys_schema_qualify.sql",
            sql: include_str!("../migrations/0322_lj_docket_search_keys_schema_qualify.sql"),
        },
        Migration {
            version: 323,
            name: "0323_decisions_bm25_v8_docket_search.sql",
            sql: include_str!("../migrations/0323_decisions_bm25_v8_docket_search.sql"),
        },
        Migration {
            version: 324,
            name: "0324_decisions_bm25_swap_docket_search.sql",
            sql: include_str!("../migrations/0324_decisions_bm25_swap_docket_search.sql"),
        },
        Migration {
            version: 325,
            name: "0325_lj_fold_function.sql",
            sql: include_str!("../migrations/0325_lj_fold_function.sql"),
        },
        Migration {
            version: 326,
            name: "0326_reindex_entity_annuaire_idx.sql",
            sql: include_str!("../migrations/0326_reindex_entity_annuaire_idx.sql"),
        },
        Migration {
            version: 327,
            name: "0327_reindex_entity_prefix_idx.sql",
            sql: include_str!("../migrations/0327_reindex_entity_prefix_idx.sql"),
        },
        Migration {
            version: 328,
            name: "0328_reindex_entity_prefix_contentieux_idx.sql",
            sql: include_str!("../migrations/0328_reindex_entity_prefix_contentieux_idx.sql"),
        },
        Migration {
            version: 329,
            name: "0329_reindex_entity_folded_all_idx.sql",
            sql: include_str!("../migrations/0329_reindex_entity_folded_all_idx.sql"),
        },
        Migration {
            version: 330,
            name: "0330_assertion_rule_and_coverage_rule.sql",
            sql: include_str!("../migrations/0330_assertion_rule_and_coverage_rule.sql"),
        },
        Migration {
            version: 331,
            name: "0331_drop_users_browser_locale.sql",
            sql: include_str!("../migrations/0331_drop_users_browser_locale.sql"),
        },
        Migration {
            version: 332,
            name: "0332_pending_docket_idx_fastupdate_off.sql",
            sql: include_str!("../migrations/0332_pending_docket_idx_fastupdate_off.sql"),
        },
        Migration {
            version: 333,
            name: "0333_decision_text.sql",
            sql: include_str!("../migrations/0333_decision_text.sql"),
        },
        Migration {
            version: 334,
            name: "0334_decision_text_bm25.sql",
            sql: include_str!("../migrations/0334_decision_text_bm25.sql"),
        },
        Migration {
            version: 335,
            name: "0335_decision_text_writers.sql",
            sql: include_str!("../migrations/0335_decision_text_writers.sql"),
        },
        Migration {
            version: 336,
            name: "0336_drop_decisions_bm25.sql",
            sql: include_str!("../migrations/0336_drop_decisions_bm25.sql"),
        },
        Migration {
            version: 337,
            name: "0337_drop_decisions_text_columns.sql",
            sql: include_str!("../migrations/0337_drop_decisions_text_columns.sql"),
        },
        Migration {
            version: 338,
            name: "0338_decision_text_per_provenance.sql",
            sql: include_str!("../migrations/0338_decision_text_per_provenance.sql"),
        },
    ]
}

/// Clé du verrou consultatif sérialisant les runs de migration concurrents.
/// Forme à **deux int4** (`pg_advisory_lock(int4, int4)`) → espace de verrous
/// disjoint de celui des `pg_advisory_xact_lock(decision_id)` (bigint) du
/// repository. `classid` = `'LJ'`, `objid` = `'mig'`.
const MIGRATION_LOCK_CLASS: i32 = 0x4C4A;
const MIGRATION_LOCK_OBJ: i32 = 0x6D6967;

/// Prend le verrou consultatif de **session** sérialisant les migrations. Le
/// `SELECT pg_advisory_lock` peut bloquer bien au-delà des 30 s de
/// `statement_timeout` posées par le pool ; on le scoppe donc dans une
/// transaction avec `SET LOCAL statement_timeout = 0`. Le verrou de session
/// survit au COMMIT et reste tenu jusqu'à `pg_advisory_unlock` ou la fin de
/// session.
async fn acquire_migration_lock(conn: &Connection) -> Result<()> {
    tracing::info!("migrations: acquiring the advisory lock");
    let waited = std::time::Instant::now();
    conn.batch_execute("BEGIN").await?;
    let acquired = async {
        conn.batch_execute("SET LOCAL statement_timeout = 0")
            .await?;
        conn.execute(
            "SELECT pg_advisory_lock($1, $2)",
            &[&MIGRATION_LOCK_CLASS, &MIGRATION_LOCK_OBJ],
        )
        .await?;
        Ok::<(), StoreError>(())
    }
    .await;
    match acquired {
        Ok(()) => {
            conn.batch_execute("COMMIT").await?;
            tracing::info!(
                waited_ms = waited.elapsed().as_millis() as u64,
                "migrations: advisory lock acquired"
            );
            Ok(())
        }
        Err(err) => {
            let _ = conn.batch_execute("ROLLBACK").await;
            Err(err)
        }
    }
}

/// Table de suivi du produit.
pub const SCHEMA_MIGRATIONS: &str = "schema_migrations";

/// Crée la table de suivi si absente, et son schéma quand le nom est qualifié :
/// la table précède la première migration, qui ne peut donc pas le créer.
///
/// L'existence est testée avant le `CREATE` : `IF NOT EXISTS` vérifie d'abord
/// le droit `CREATE` sur la base, que le propriétaire du schéma n'a pas besoin
/// d'avoir. Sûr sous le verrou de migration, tenu par l'appelant.
async fn ensure_schema_migrations(conn: &Connection, table: &str) -> Result<()> {
    if let Some((schema, _)) = table.split_once('.') {
        let existe: bool = conn
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1)",
                &[&schema],
            )
            .await?
            .get(0);
        if !existe {
            conn.batch_execute(&format!("CREATE SCHEMA {schema}"))
                .await?;
        }
    }
    conn.batch_execute(&format!(
        "CREATE TABLE IF NOT EXISTS {table} (\
           version INTEGER PRIMARY KEY,\
           applied_at TIMESTAMPTZ NOT NULL\
         )"
    ))
    .await?;
    Ok(())
}

/// Versions déjà appliquées.
async fn applied_versions(conn: &Connection, table: &str) -> Result<Vec<i32>> {
    let rows = conn
        .query(&format!("SELECT version FROM {table}"), &[])
        .await?;
    Ok(rows.iter().map(|row| row.get::<_, i32>(0)).collect())
}

/// Applique les migrations non-vues. Renvoie les versions effectivement
/// appliquées.
///
/// Sérialise les runners concurrents via un verrou consultatif de session
/// (cf. doc de module), puis délègue à [`apply_migrations_locked`]. Le verrou est
/// **toujours relâché** au retour, succès comme erreur.
pub async fn apply_migrations(conn: &Connection, intent: MigrationIntent) -> Result<Vec<i32>> {
    match intent {
        MigrationIntent::Apply => {
            apply_migrations_with(conn, embedded_migrations(), SCHEMA_MIGRATIONS).await
        }
        MigrationIntent::Verify => {
            verify_schema_current(conn).await?;
            Ok(Vec::new())
        }
    }
}

/// What a caller allows itself on the schema. Every command states it, because
/// the implicit default was `Apply`: on 2026-09-10 migration 0296 rode in on a
/// `quantum-extract` and took 547 s of ACCESS EXCLUSIVE on `legal_article`,
/// which its operator had not asked for.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MigrationIntent {
    /// Refuses a schema that lags, and writes nothing.
    Verify,
    /// Applies what is pending. Reserved for `lj-ingest migrate`.
    Apply,
}

/// The check of [`apply_migrations`] without its writing: raises when the base
/// holds a version the binary lacks, then when the binary holds one the base
/// lacks. It does not create `schema_migrations`, a check writing nothing.
async fn verify_schema_current(conn: &Connection) -> Result<()> {
    let table: Option<String> = conn
        .query_one(
            &format!("SELECT to_regclass('{SCHEMA_MIGRATIONS}')::text"),
            &[],
        )
        .await?
        .get(0);
    let available: Vec<i32> = embedded_migrations().iter().map(|m| m.version).collect();
    let applied = match table {
        Some(_) => applied_versions(conn, SCHEMA_MIGRATIONS).await?,
        None => Vec::new(),
    };
    let missing: Vec<i32> = applied
        .iter()
        .copied()
        .filter(|v| !available.contains(v))
        .collect();
    if let Some(&version) = missing.first() {
        return Err(StoreError::UnknownMigrations { version, missing });
    }
    let pending: Vec<i32> = available
        .into_iter()
        .filter(|v| !applied.contains(v))
        .collect();
    if pending.is_empty() {
        Ok(())
    } else {
        Err(StoreError::PendingMigrations { pending })
    }
}

/// Applique une **autre** série, suivie dans sa propre table : deux séries qui
/// partageraient la table se refuseraient l'une l'autre, chacune levant sur une
/// version absente de son disque. Le verrou reste commun, les deux touchant la
/// même base. `table` est un nom d'objet SQL, d'où le `&'static`.
pub async fn apply_migrations_with(
    conn: &Connection,
    migrations: Vec<Migration>,
    table: &'static str,
) -> Result<Vec<i32>> {
    acquire_migration_lock(conn).await?;
    let result = apply_migrations_locked(conn, migrations, table).await;
    // Relâche le verrou de session sur tous les chemins.
    let _ = conn
        .execute(
            "SELECT pg_advisory_unlock($1, $2)",
            &[&MIGRATION_LOCK_CLASS, &MIGRATION_LOCK_OBJ],
        )
        .await;
    result
}

/// Corps de [`apply_migrations`] exécuté **sous verrou** :
///
/// 1. crée `schema_migrations` ;
/// 2. liste les versions appliquées, relues *après* le verrou : le gagnant a
///    déjà committé, donc le perdant voit ses migrations ;
/// 3. lève [`StoreError::UnknownMigrations`] si une version en base n'a pas de
///    fichier embarqué correspondant (binaire plus ancien que la base) ;
/// 4. applique chaque migration en attente, chacune dans sa propre transaction
///    (`BEGIN`/`COMMIT`, `ROLLBACK` sur erreur), puis insère la ligne
///    `schema_migrations`.
async fn apply_migrations_locked(
    conn: &Connection,
    available: Vec<Migration>,
    table: &str,
) -> Result<Vec<i32>> {
    ensure_schema_migrations(conn, table).await?;

    // `available` est déjà trié par version croissante (cf. embedded_migrations).
    let available_versions: Vec<i32> = available.iter().map(|m| m.version).collect();

    let applied = applied_versions(conn, table).await?;

    let missing: Vec<i32> = applied
        .iter()
        .copied()
        .filter(|v| !available_versions.contains(v))
        .collect();
    if !missing.is_empty() {
        let mut missing_sorted = missing;
        missing_sorted.sort_unstable();
        let max = *missing_sorted.last().expect("non-empty");
        return Err(StoreError::UnknownMigrations {
            version: max,
            missing: missing_sorted,
        });
    }

    let pending: Vec<&Migration> = available
        .iter()
        .filter(|m| !applied.contains(&m.version))
        .collect();
    if pending.is_empty() {
        return Ok(Vec::new());
    }

    let mut newly_applied: Vec<i32> = Vec::with_capacity(pending.len());
    for migration in pending {
        tracing::info!(
            version = migration.version,
            name = migration.name,
            "applying migration"
        );
        let started = std::time::Instant::now();
        if let Err(err) = apply_one(conn, migration, table).await {
            // best-effort rollback avant de remonter l'erreur d'origine.
            let _ = conn.batch_execute("ROLLBACK").await;
            return Err(err);
        }
        tracing::info!(
            version = migration.version,
            name = migration.name,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "migration applied"
        );
        newly_applied.push(migration.version);
    }
    Ok(newly_applied)
}

/// Marqueur de **première ligne** d'une migration à jouer hors transaction.
/// Un seul besoin réel : `CREATE INDEX CONCURRENTLY`, que Postgres refuse dans
/// un bloc transactionnel — et c'est le seul moyen de changer le schéma d'un
/// index bm25 sans couper la recherche, ParadeDB n'autorisant pas deux index
/// bm25 sur une table par le chemin ordinaire.
const SANS_TRANSACTION: &str = "-- lj:sans-transaction";

/// Applique une migration dans une transaction explicite. `applied_at` est posé
/// par `now()` côté Postgres, sans dépendre de l'horloge du client.
async fn apply_one(conn: &Connection, migration: &Migration, table: &str) -> Result<()> {
    if migration.sql.starts_with(SANS_TRANSACTION) {
        return apply_one_sans_transaction(conn, migration, table).await;
    }
    conn.batch_execute("BEGIN").await?;
    // Les migrations sont du SQL de maintenance de confiance ; certaines
    // reconstruisent des index de plusieurs Go, bien au-delà des 30 s que
    // `build_pool` arme sur chaque connexion. `SET LOCAL` : scoppé à la
    // transaction de la migration → la connexion repart propre dans le pool.
    conn.batch_execute("SET LOCAL statement_timeout = 0")
        .await?;
    // `lock_timeout` borne l'ATTENTE d'un verrou, pas la durée du travail — une
    // reconstruction d'index de 20 min passe, une DDL qui ne décroche pas son
    // ACCESS EXCLUSIVE en 5 s échoue. La migration tourne serveur debout : une DDL
    // en attente derrière une requête longue fait la queue devant elle toutes les
    // requêtes suivantes sur la table.
    conn.batch_execute("SET LOCAL lock_timeout = '5s'").await?;
    conn.batch_execute(migration.sql).await?;
    conn.execute(
        &format!("INSERT INTO {table} (version, applied_at) VALUES ($1, now())"),
        &[&migration.version],
    )
    .await?;
    conn.batch_execute("COMMIT").await?;
    Ok(())
}

/// Applique une migration marquée [`SANS_TRANSACTION`] : la DDL puis la ligne
/// de `schema_migrations`, chacune en autocommit.
///
/// Trois conséquences, toutes assumées :
///
/// - **Un seul statement par fichier** : un simple-query qui en porte plusieurs
///   est lui aussi un bloc transactionnel, où `CONCURRENTLY` est rejeté. Une
///   bascule d'index se découpe donc en deux migrations.
/// - **Pas de rollback** : un `CREATE INDEX CONCURRENTLY` interrompu laisse un
///   index INVALIDE et la version non enregistrée, donc le run suivant échoue
///   sur « already exists ». La reprise est un `DROP INDEX` de l'opérateur.
/// - **Pas de `lock_timeout`** : `CONCURRENTLY` ne prend qu'un `SHARE UPDATE
///   EXCLUSIVE`, il ne fait la queue devant personne.
///
/// `SET LOCAL` n'a pas de portée hors transaction : le `statement_timeout` est
/// posé en session puis `RESET`, ce qui restaure celui du pool et non le défaut
/// serveur.
async fn apply_one_sans_transaction(
    conn: &Connection,
    migration: &Migration,
    table: &str,
) -> Result<()> {
    conn.batch_execute("SET statement_timeout = 0").await?;
    let ddl = conn.batch_execute(migration.sql).await;
    conn.batch_execute("RESET statement_timeout").await?;
    ddl?;
    conn.execute(
        &format!("INSERT INTO {table} (version, applied_at) VALUES ($1, now())"),
        &[&migration.version],
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_migrations_are_contiguous() {
        let migs = embedded_migrations();
        assert!(!migs.is_empty());
        // versions contiguës 1..=N, strictement croissantes (pas de trou ni
        // de doublon possible quand on ajoute une migration).
        for (i, m) in migs.iter().enumerate() {
            assert_eq!(m.version, (i + 1) as i32, "version contiguë");
        }
    }

    /// Un `.sql` déposé dans `migrations/` sans sa ligne dans
    /// `embedded_migrations` ne casse aucune compilation et aucun autre test :
    /// le migrateur l'ignore en silence, et le code qui en dépend part en prod
    /// sans son schéma. C'est ce qui a mis `lj_docket_keys` en 500 le
    /// 14/08/2026 — les 0211 à 0214 étaient sur disque, absentes de la liste.
    #[test]
    fn every_sql_file_is_embedded() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let mut sur_disque: Vec<String> = std::fs::read_dir(&dir)
            .expect("dossier migrations lisible")
            .map(|e| {
                e.expect("entrée lisible")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|n| n.ends_with(".sql"))
            .collect();
        sur_disque.sort();

        let mut embarquees: Vec<String> = embedded_migrations()
            .iter()
            .map(|m| m.name.to_string())
            .collect();
        embarquees.sort();

        assert_eq!(
            sur_disque, embarquees,
            "migrations/ et embedded_migrations() divergent — ajouter la ligne manquante"
        );
    }

    #[test]
    fn migration_names_match_versions() {
        for m in embedded_migrations() {
            let prefix: String = m.name.chars().take(4).collect();
            let parsed: i32 = prefix.parse().expect("prefix NNNN numérique");
            assert_eq!(parsed, m.version, "nom {} vs version {}", m.name, m.version);
            assert!(m.name.ends_with(".sql"));
        }
    }

    #[test]
    fn sans_transaction_migrations_carry_a_single_statement() {
        for m in embedded_migrations()
            .iter()
            .filter(|m| m.sql.starts_with(SANS_TRANSACTION))
        {
            let corps: String = m
                .sql
                .lines()
                .filter(|l| !l.trim_start().starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(
                corps.matches(';').count(),
                1,
                "{} : un seul statement attendu hors transaction",
                m.name
            );
            assert!(
                corps.trim_end().ends_with(';'),
                "{} : le statement doit clore le fichier",
                m.name
            );
        }
    }

    #[test]
    fn first_migration_sql_is_non_empty() {
        let migs = embedded_migrations();
        let init = &migs[0];
        assert_eq!(init.name, "0001_init.sql");
        assert!(init.sql.contains("schema_migrations") || init.sql.contains("decisions"));
        assert!(!init.sql.trim().is_empty());
    }
}
