-- lj:sans-transaction
-- Migration 0219 — legal_text_body_bm25 reconstruit avec le tokenizer qui garde
-- un numéro entier.
--
-- 13 805 titres portent un numéro à barre, et pour un instrument européen ce
-- numéro EST l'identité du texte : « directive 2018/1808 » ne désigne rien
-- d'autre. Mesuré sur 600 instruments tirés du corpus, P@1 0,2967 → 0,3417 et
-- MRR 0,3123 → 0,3608, aucune strate en recul.
--
-- Le pattern est celui de `lj_core::body_tok::TOKEN_PATTERN`, recopié ici parce
-- que le SQL ne peut pas l'importer. Champs repris à l'identique de l'index en
-- place, seul le pattern change.
--
-- Hors transaction, donc sans rollback : une interruption laisse un index
-- INVALIDE que le run suivant refusera de recréer. Reprise = DROP INDEX
-- legal_text_body_bm25_v2, puis rejouer. ~2 min pour 1,3 M de textes.

CREATE INDEX CONCURRENTLY legal_text_body_bm25_v2 ON legal_text
USING bm25 (
  id,
  title,
  body,
  ((text_uid)::pdb.literal),
  ((nature)::pdb.literal),
  ((jurisdiction)::pdb.literal),
  ((slug)::pdb.literal),
  ((role)::pdb.literal),
  ((COALESCE(status, 'VIGUEUR'::text))::pdb.literal('alias=status_eff')),
  ((upper(nature))::pdb.literal('alias=nature_up')),
  ((lower(nature))::pdb.literal('alias=source')),
  (((body IS NOT NULL))::pdb.alias('alias=has_body'))
) WITH (key_field=id, text_fields='{
    "title": {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"},
    "body":  {"tokenizer": {"type": "regex", "pattern": "\\p{N}+(?:[./-]\\p{N}+)+|[\\p{L}\\p{N}-]+", "ascii_folding": true, "stopwords_language": "French", "stopwords": ["a", "à"]}, "record": "position"}
  }');
