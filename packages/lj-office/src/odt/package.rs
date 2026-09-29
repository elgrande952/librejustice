//! The ZIP container, entries held in memory in their original order.

use std::io::{Cursor, Read, Write};

use crate::ooxml::{MAX_ENTRIES, MAX_UNPACKED};
use anyhow::{anyhow, bail, Context, Result};
use zip::CompressionMethod;

pub struct Entry {
    pub name: String,
    pub data: Vec<u8>,
    pub deflated: bool,
}

pub struct Package {
    pub entries: Vec<Entry>,
}

pub fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

impl Package {
    /// Read every entry, refused past `left` bytes unpacked. The data is
    /// inflated here rather than by the archive reader, which refuses an entry
    /// whose CRC is wrong where LibreOffice reads it.
    pub fn open(bytes: &[u8], left: &mut u64) -> Result<Self> {
        let mut archive =
            zip::ZipArchive::new(Cursor::new(bytes)).context("reading the package")?;
        if archive.len() > MAX_ENTRIES {
            bail!(
                "the package holds {} entries, more than {MAX_ENTRIES}",
                archive.len()
            );
        }
        let mut entries = Vec::with_capacity(archive.len());
        for i in 0..archive.len() {
            let mut file = archive.by_index_raw(i).context("reading a package entry")?;
            let name = file.name().to_string();
            let method = file.compression();
            let mut raw =
                Vec::with_capacity(file.compressed_size().min(bytes.len() as u64) as usize);
            file.read_to_end(&mut raw)
                .with_context(|| format!("reading {name}"))?;
            let data = match method {
                CompressionMethod::Stored => raw,
                CompressionMethod::Deflated => {
                    let mut out = Vec::with_capacity(file.size().min(*left) as usize);
                    flate2::read::DeflateDecoder::new(raw.as_slice())
                        .take(left.saturating_add(1))
                        .read_to_end(&mut out)
                        .with_context(|| format!("inflating {name}"))?;
                    out
                }
                other => bail!("{name} is compressed with {other:?}"),
            };
            if data.len() as u64 > *left {
                bail!(
                    "the package unpacks to more than {} MiB",
                    MAX_UNPACKED >> 20
                );
            }
            *left -= data.len() as u64;
            entries.push(Entry {
                name,
                data,
                deflated: method == CompressionMethod::Deflated,
            });
        }
        Ok(Self { entries })
    }

    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.data.as_slice())
    }

    pub fn position(&self, name: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.name == name)
    }

    pub fn save(&self) -> Result<Vec<u8>> {
        let entries: Vec<(&Entry, Option<Vec<u8>>)> =
            self.entries.iter().map(|e| (e, None)).collect();
        write(&entries)
    }
}

/// Write entries as a package, each with its replacement data if it has one.
/// `mimetype` goes first and stored, as ODF requires for a reader that sniffs
/// the format from the first bytes.
pub fn write(entries: &[(&Entry, Option<Vec<u8>>)]) -> Result<Vec<u8>> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated =
        zip::write::SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mimetype = entries.iter().position(|(e, _)| e.name == "mimetype");
    let order = mimetype
        .into_iter()
        .chain((0..entries.len()).filter(|i| Some(*i) != mimetype));
    for i in order {
        let (entry, replaced) = &entries[i];
        let data = replaced.as_deref().unwrap_or(&entry.data);
        if entry.name.ends_with('/') && data.is_empty() {
            writer
                .add_directory(entry.name.as_str(), stored)
                .context("writing a directory entry")?;
            continue;
        }
        let options = if entry.deflated && Some(i) != mimetype {
            deflated
        } else {
            stored
        };
        writer
            .start_file(entry.name.as_str(), options)
            .with_context(|| format!("writing {}", entry.name))?;
        writer
            .write_all(data)
            .with_context(|| format!("writing {}", entry.name))?;
    }
    Ok(writer
        .finish()
        .map_err(|e| anyhow!("closing the package: {e}"))?
        .into_inner())
}
