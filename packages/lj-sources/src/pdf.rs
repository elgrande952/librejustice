//! Extraction texte PDF born-digital au bord I/O.
//!
//! Le décodage de format vit dans `lj-sources` : ce module rend le texte plat
//! pour que le parser pur de `lj-core` reçoive du texte, pas des octets PDF.

use crate::error::{Result, SourceError};
use std::io::Write;
use std::process::{Command, Stdio};

/// A scanned PDF succeeds and yields an all but empty text: falling back to OCR
/// is the caller's call.
pub fn pdftotext_extract(bytes: &[u8]) -> Result<String> {
    poppler("pdftotext", &["-q", "-enc", "UTF-8", "-", "-"], bytes)
}

/// Renders the PDF to HTML, for the one thing flat text loses: the bold that
/// tells a form's label from its value. `-stdout` demands an output name all
/// the same, which gives rise to no file.
pub fn pdftohtml_extract(bytes: &[u8]) -> Result<String> {
    poppler(
        "pdftohtml",
        &["-q", "-i", "-noframes", "-stdout", "-", "sortie"],
        bytes,
    )
}

/// PDF on stdin, output read on stdout. A PDF weighing several MB deadlocks
/// against the stdout pipe unless stdin is written from its own thread.
fn poppler(outil: &str, args: &[&str], bytes: &[u8]) -> Result<String> {
    let mut child = Command::new(outil)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            SourceError::Invalid(format!("{outil} introuvable (poppler-utils requis): {e}"))
        })?;
    let mut stdin = child
        .stdin
        .take()
        .expect("stdin piped above is always present");
    let payload = bytes.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&payload));
    let output = child
        .wait_with_output()
        .map_err(|e| SourceError::Invalid(format!("{outil}: {e}")))?;
    writer
        .join()
        .map_err(|_| SourceError::Invalid(format!("{outil}: thread stdin paniqué")))?
        .map_err(|e| SourceError::Invalid(format!("{outil} stdin: {e}")))?;
    if !output.status.success() {
        return Err(SourceError::Invalid(format!(
            "{outil} code {:?}",
            output.status.code()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
