-- Jambe « code nommé » de la recherche d'articles : la clause usage scopée
-- filtre `u.text_uid = $code` dans le scan ParadeDB — le champ doit être un
-- literal de l'index pour que le filtre soit poussé (TopK sans heap filter,
-- même mécanique que text_uid dans legal_article_bm25).
DROP INDEX legal_article_usage_bm25;
CREATE INDEX legal_article_usage_bm25 ON legal_article_usage USING bm25 (
  id, terms, ((text_uid)::pdb.literal)
) WITH (key_field=id, text_fields='{
    "terms": {"tokenizer": {"type": "regex", "pattern": "[\\p{L}\\p{N}_-]+", "ascii_folding": true}, "record": "freq"}
  }');
