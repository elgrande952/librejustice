ALTER TABLE legal_article
    ADD COLUMN kind text NOT NULL DEFAULT 'article';

COMMENT ON COLUMN legal_article.kind IS
  'Nature de la ligne : ''article'' (norme ou publication) | ''edit'' (dispositif consommé, LEGI n''en garde que la mention de sa cible). Posée par refresh_article_denorm depuis le corps.';
