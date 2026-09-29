ALTER TABLE decision_semantic_keywords
ADD COLUMN keyword_scores REAL[] NOT NULL DEFAULT ARRAY[]::REAL[];

ALTER TABLE decision_semantic_keywords
ADD COLUMN expression_scores REAL[] NOT NULL DEFAULT ARRAY[]::REAL[];
