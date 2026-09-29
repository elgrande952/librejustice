INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('procedure:CHAMBRE_DU_CONSEIL', 'procedure', 'Chambre du conseil',   14),
    ('matiere:REFERES',              'matiere',   'Chambre des référés',  25),
    ('matiere:PRESSE',               'matiere',   'Chambre de la presse', 26);

DELETE FROM facet_value WHERE uid = 'matiere:CONSEIL';
