-- Le libellé d'un palier de publication ne dit pas ce qu'il vaut : « Publié aux
-- lettres de chambre » ne se lit que si on sait ce qu'est une lettre de chambre.
-- Le hint le dit, et le code source reste en fin de phrase pour qui remonte.

UPDATE facet_value SET hint = 'Retenue dans le rapport annuel de la Cour de cassation, la plus étroite de ses sélections. Code source r.'
WHERE uid = 'publication:PUBLIE_RAPPORT';

UPDATE facet_value SET hint = 'Retenue au Bulletin des arrêts, où la Cour de cassation publie chaque mois la jurisprudence qu''elle veut faire connaître. Code source b.'
WHERE uid = 'publication:PUBLIE_BULLETIN';

UPDATE facet_value SET hint = 'Signalée dans la lettre que sa chambre publie pour ses propres lecteurs, un palier sous le Bulletin. Code source l.'
WHERE uid = 'publication:LETTRE_CHAMBRE';

UPDATE facet_value SET hint = 'Accompagnée d''un communiqué de la Cour de cassation, qui en explique la portée. Code source c.'
WHERE uid = 'publication:COMMUNIQUE';

UPDATE facet_value SET hint = 'Rendue par la Cour de cassation sans être retenue au Bulletin. Code source n.'
WHERE uid = 'publication:INEDIT_BULLETIN';

UPDATE facet_value SET hint = 'Retenue au recueil Lebon, la publication de référence de la jurisprudence administrative. Code source A.'
WHERE uid = 'publication:PUBLIE_LEBON';

UPDATE facet_value SET hint = 'Citée aux tables du recueil Lebon, un palier sous la publication au recueil. Couvre les codes source B, C+ et R.'
WHERE uid = 'publication:MENTIONNE_LEBON';

UPDATE facet_value SET hint = 'Rendue par une juridiction administrative sans être retenue au recueil Lebon. Couvre les codes source C, D et Z.'
WHERE uid = 'publication:INEDIT_LEBON';
