-- A « chambre du conseil » is a hearing held away from the public, not a room:
-- it leaves the matters for the procedures. The référés and the press do name a
-- room and enter as matters, mirroring SPECIALTY_LABELS (lj-core).
--
-- Applied BEFORE the re-extraction: the FK on `decisions.procedure_uid` refuses
-- the first decision written without it. No column references a matter, so the
-- delete holds at any moment.

INSERT INTO facet_value (uid, facet, label, sort) VALUES
    ('procedure:CHAMBRE_DU_CONSEIL', 'procedure', 'Chambre du conseil',   14),
    ('matiere:REFERES',              'matiere',   'Chambre des référés',  25),
    ('matiere:PRESSE',               'matiere',   'Chambre de la presse', 26);

DELETE FROM facet_value WHERE uid = 'matiere:CONSEIL';
