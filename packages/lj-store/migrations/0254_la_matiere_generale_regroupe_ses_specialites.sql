-- Une matière spécialisée est membre de la matière générale qui la couvre :
-- filtrer « Chambre civile » retient aussi les baux et la copropriété.
UPDATE facet_value SET parent_uid = 'matiere:CIVILE' WHERE uid IN (
    'matiere:BAUX',
    'matiere:CIVI',
    'matiere:CONSTRUCTION',
    'matiere:COPROPRIETE',
    'matiere:EXPROPRIATION',
    'matiere:FAMILLE',
    'matiere:NATIONALITE',
    'matiere:SURENDETTEMENT',
    'matiere:TERRES'
);

UPDATE facet_value SET parent_uid = 'matiere:SOCIALE' WHERE uid IN (
    'matiere:PROTECTION_SOCIALE',
    'matiere:PRUD_HOMALE'
);

UPDATE facet_value SET parent_uid = 'matiere:COMMERCIALE'
WHERE uid = 'matiere:PROCEDURES_COLLECTIVES';

UPDATE facet_value SET parent_uid = 'matiere:CRIMINELLE' WHERE uid IN (
    'matiere:CORRECTIONNELLE',
    'matiere:INSTRUCTION',
    'matiere:MINEURS'
);
