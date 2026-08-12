//! Cible lib de `lj-ingest`. N'expose que le pont d'extraction (`extract`) :
//! c'est LA fonction d'extraction du système — le banc (`lj-bench`) l'importe
//! pour scorer exactement l'artefact que l'ingest persiste. Le reste (pipelines,
//! CLI, cron) demeure privé au binaire.

pub mod extract;
