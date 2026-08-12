-- 0168 : revendication des textes portés par un dataset curé.
--
-- `curated = true` : le texte (fiche, articles, arêtes TOC) appartient au
-- loader legal-corpus ; les fonds bulk (LEGI/JORF/KALI/BOFIP) n'écrivent
-- plus dessus — gardes dans le repository lj-store.

ALTER TABLE legal_text ADD COLUMN curated boolean NOT NULL DEFAULT false;

-- Backfill : signature du loader corpus (`source_uid` = `text_uid#num_key`
-- ou `text_uid#num_key@date`). BOFIP construit le même motif mais reste un
-- fond bulk — exclu par son namespace `BOI-`.
UPDATE legal_text t
SET curated = true
WHERE t.text_uid NOT LIKE 'BOI-%'
  AND EXISTS (
    SELECT 1 FROM legal_article a
    WHERE a.text_uid = t.text_uid
      AND a.source_uid LIKE t.text_uid || '#%');
