CREATE INDEX entity_denomination_contentieux_idx
    ON entity (denomination_folded text_pattern_ops)
    WHERE decision_count > 0;
