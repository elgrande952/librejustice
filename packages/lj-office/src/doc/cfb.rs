//! Compound File Binary container: a lenient reader, and a writer that lays
//! every stream out afresh, so no free sector keeps what a stream once held.
//!
//! Lenient because Word and LibreOffice open files whose FAT outruns a
//! truncated tail, or whose directory tree points past its end: a missing
//! sector reads as zeros, a chain stops at its first loop, and a stream the
//! tree cannot reach is still found by name.

use anyhow::{bail, Result};

use super::bytes::{u16_at, u32_at};

const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const DIFSECT: u32 = 0xFFFF_FFFC;
const FATSECT: u32 = 0xFFFF_FFFD;
const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
const FREESECT: u32 = 0xFFFF_FFFF;
const NOSTREAM: u32 = 0xFFFF_FFFF;
const MINI_CUTOFF: usize = 4096;
const MINI: usize = 64;
const SECTOR: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Storage,
    Stream,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub name: String,
    pub kind: Kind,
    pub clsid: [u8; 16],
    pub state_bits: u32,
    pub data: Vec<u8>,
    pub children: Vec<usize>,
}

/// A container read into memory: `nodes[0]` is the root storage.
#[derive(Clone, Debug)]
pub struct Cfb {
    pub nodes: Vec<Node>,
}

struct RawEntry {
    name: String,
    kind: u8,
    left: u32,
    right: u32,
    child: u32,
    clsid: [u8; 16],
    state_bits: u32,
    start: u32,
    size: u64,
}

fn chain(fat: &[u32], start: u32, sectors: usize) -> Vec<u32> {
    let mut out = Vec::new();
    let mut seen = vec![false; sectors];
    let mut id = start;
    while (id as usize) < sectors && !seen[id as usize] {
        seen[id as usize] = true;
        out.push(id);
        match fat.get(id as usize) {
            Some(next) => id = *next,
            None => break,
        }
    }
    out
}

impl Cfb {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < SECTOR || bytes[..8] != SIGNATURE {
            bail!("not a compound file");
        }
        let major = u16_at(bytes, 0x1A).unwrap_or(3);
        let sector = match u16_at(bytes, 0x1E) {
            Some(9) => 512,
            Some(12) => 4096,
            _ if major == 4 => 4096,
            _ => 512,
        };
        let mut file = bytes.to_vec();
        let body = file.len().saturating_sub(sector);
        let sectors = body.div_ceil(sector);
        file.resize(sector + sectors * sector, 0);
        let sector_at = |id: u32| &file[(id as usize + 1) * sector..(id as usize + 2) * sector];
        let per = sector / 4;

        let n_fat = u32_at(bytes, 0x2C).unwrap_or(0) as usize;
        let mut fat_ids: Vec<u32> = Vec::new();
        for i in 0..109 {
            let id = u32_at(bytes, 0x4C + 4 * i).unwrap_or(FREESECT);
            if (id as usize) < sectors && fat_ids.len() < n_fat.max(1) {
                fat_ids.push(id);
            }
        }
        let mut difat = u32_at(bytes, 0x44).unwrap_or(ENDOFCHAIN);
        let mut seen_difat = vec![false; sectors];
        while (difat as usize) < sectors && !seen_difat[difat as usize] && fat_ids.len() < n_fat {
            seen_difat[difat as usize] = true;
            let s = sector_at(difat);
            for i in 0..per - 1 {
                let id = u32_at(s, 4 * i).unwrap_or(FREESECT);
                if (id as usize) < sectors && fat_ids.len() < n_fat {
                    fat_ids.push(id);
                }
            }
            difat = u32_at(s, 4 * (per - 1)).unwrap_or(ENDOFCHAIN);
        }
        let mut fat: Vec<u32> = Vec::with_capacity(fat_ids.len() * per);
        for id in &fat_ids {
            let s = sector_at(*id);
            fat.extend((0..per).map(|i| u32_at(s, 4 * i).unwrap_or(FREESECT)));
        }

        let read_chain = |start: u32| -> Vec<u8> {
            let mut out = Vec::new();
            for id in chain(&fat, start, sectors) {
                out.extend_from_slice(sector_at(id));
            }
            out
        };

        let dir = read_chain(u32_at(bytes, 0x30).unwrap_or(ENDOFCHAIN));
        let mut raw: Vec<RawEntry> = Vec::new();
        for e in dir.as_chunks::<128>().0 {
            let name_len = (u16_at(e, 0x40).unwrap_or(0) as usize).min(64);
            let units: Vec<u16> = e[..name_len.saturating_sub(2)]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .take_while(|u| *u != 0)
                .collect();
            let mut clsid = [0u8; 16];
            clsid.copy_from_slice(&e[0x50..0x60]);
            let low = u32_at(e, 0x78).unwrap_or(0) as u64;
            let high = u32_at(e, 0x7C).unwrap_or(0) as u64;
            raw.push(RawEntry {
                name: String::from_utf16_lossy(&units),
                kind: e[0x42],
                left: u32_at(e, 0x44).unwrap_or(NOSTREAM),
                right: u32_at(e, 0x48).unwrap_or(NOSTREAM),
                child: u32_at(e, 0x4C).unwrap_or(NOSTREAM),
                clsid,
                state_bits: u32_at(e, 0x60).unwrap_or(0),
                start: u32_at(e, 0x74).unwrap_or(ENDOFCHAIN),
                size: if sector == 4096 {
                    low | (high << 32)
                } else {
                    low
                },
            });
        }
        let Some(root) = raw.first() else {
            bail!("compound file without a directory");
        };
        let mini_stream = {
            let mut m = read_chain(root.start);
            m.truncate(root.size as usize);
            m
        };
        let minifat: Vec<u32> = read_chain(u32_at(bytes, 0x3C).unwrap_or(ENDOFCHAIN))
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect();
        let mini_sectors = mini_stream.len() / MINI;

        let stream_data = |e: &RawEntry| -> Vec<u8> {
            let size = e.size as usize;
            let mut out = Vec::new();
            if size < MINI_CUTOFF {
                for id in chain(&minifat, e.start, mini_sectors) {
                    let at = id as usize * MINI;
                    out.extend_from_slice(&mini_stream[at..at + MINI]);
                    if out.len() >= size {
                        break;
                    }
                }
            } else {
                for id in chain(&fat, e.start, sectors) {
                    out.extend_from_slice(sector_at(id));
                    if out.len() >= size {
                        break;
                    }
                }
            }
            out.truncate(size);
            out
        };

        let mut nodes = vec![Node {
            name: "Root Entry".into(),
            kind: Kind::Storage,
            clsid: root.clsid,
            state_bits: root.state_bits,
            data: Vec::new(),
            children: Vec::new(),
        }];
        let mut visited = vec![false; raw.len()];
        visited[0] = true;
        // (storage node, first directory entry of its sibling tree)
        let mut storages: Vec<(usize, u32)> = vec![(0, root.child)];
        while let Some((parent, top)) = storages.pop() {
            let mut stack = vec![top];
            let mut members = Vec::new();
            while let Some(id) = stack.pop() {
                let i = id as usize;
                if i >= raw.len() || visited[i] {
                    continue;
                }
                visited[i] = true;
                members.push(i);
                stack.push(raw[i].left);
                stack.push(raw[i].right);
            }
            members.sort_unstable();
            for i in members {
                let e = &raw[i];
                let kind = match e.kind {
                    1 => Kind::Storage,
                    2 => Kind::Stream,
                    _ => continue,
                };
                let at = nodes.len();
                nodes.push(Node {
                    name: e.name.clone(),
                    kind,
                    clsid: e.clsid,
                    state_bits: e.state_bits,
                    data: if kind == Kind::Stream {
                        stream_data(e)
                    } else {
                        Vec::new()
                    },
                    children: Vec::new(),
                });
                nodes[parent].children.push(at);
                if kind == Kind::Storage {
                    storages.push((at, e.child));
                }
            }
        }
        // A stream the broken tree hides is still the document's if the root
        // has nothing of that name.
        for (i, e) in raw.iter().enumerate() {
            if visited[i] || e.kind != 2 || e.name.is_empty() {
                continue;
            }
            if nodes[0].children.iter().any(|c| nodes[*c].name == e.name) {
                continue;
            }
            let at = nodes.len();
            nodes.push(Node {
                name: e.name.clone(),
                kind: Kind::Stream,
                clsid: e.clsid,
                state_bits: e.state_bits,
                data: stream_data(e),
                children: Vec::new(),
            });
            nodes[0].children.push(at);
        }
        Ok(Self { nodes })
    }

    /// The node at `path` below the root, names separated by `/`.
    pub fn find(&self, path: &str) -> Option<usize> {
        let mut at = 0;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            at = *self.nodes[at]
                .children
                .iter()
                .find(|c| self.nodes[**c].name.eq_ignore_ascii_case(part))?;
        }
        Some(at)
    }

    pub fn stream(&self, path: &str) -> Option<&[u8]> {
        let at = self.find(path)?;
        (self.nodes[at].kind == Kind::Stream).then(|| self.nodes[at].data.as_slice())
    }

    pub fn stream_mut(&mut self, path: &str) -> Option<&mut Vec<u8>> {
        let at = self.find(path)?;
        (self.nodes[at].kind == Kind::Stream).then(|| &mut self.nodes[at].data)
    }

    /// Serialise as a version 3 file, streams laid out contiguously and every
    /// unused byte zero.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut order: Vec<usize> = Vec::new();
        let mut stack = vec![0usize];
        while let Some(n) = stack.pop() {
            order.push(n);
            for c in self.nodes[n].children.iter().rev() {
                stack.push(*c);
            }
        }
        let mut dir_index = vec![NOSTREAM; self.nodes.len()];
        for (i, n) in order.iter().enumerate() {
            dir_index[*n] = i as u32;
        }
        let mut left = vec![NOSTREAM; order.len()];
        let mut right = vec![NOSTREAM; order.len()];
        let mut child = vec![NOSTREAM; order.len()];
        let mut red = vec![false; order.len()];
        for n in &order {
            if self.nodes[*n].kind != Kind::Storage || self.nodes[*n].children.is_empty() {
                continue;
            }
            let mut kids: Vec<usize> = self.nodes[*n].children.clone();
            kids.sort_by(|a, b| name_order(&self.nodes[*a].name, &self.nodes[*b].name));
            kids.dedup_by(|a, b| name_order(&self.nodes[*a].name, &self.nodes[*b].name).is_eq());
            let ids: Vec<u32> = kids.iter().map(|k| dir_index[*k]).collect();
            let depth = usize::BITS - ids.len().leading_zeros() - 1;
            child[dir_index[*n] as usize] =
                balance(&ids, 0, depth, &mut left, &mut right, &mut red);
        }

        let mut big: Vec<(usize, u32)> = Vec::new();
        let mut mini_stream: Vec<u8> = Vec::new();
        let mut minifat: Vec<u32> = Vec::new();
        let mut start = vec![ENDOFCHAIN; order.len()];
        let mut next_sector = 0u32;
        for (i, n) in order.iter().enumerate() {
            let node = &self.nodes[*n];
            if node.kind != Kind::Stream || node.data.is_empty() {
                continue;
            }
            if node.data.len() < MINI_CUTOFF {
                let first = minifat.len() as u32;
                let count = node.data.len().div_ceil(MINI);
                for k in 0..count {
                    minifat.push(if k + 1 == count {
                        ENDOFCHAIN
                    } else {
                        first + k as u32 + 1
                    });
                }
                mini_stream.extend_from_slice(&node.data);
                mini_stream.resize(minifat.len() * MINI, 0);
                start[i] = first;
            } else {
                big.push((i, next_sector));
                start[i] = next_sector;
                next_sector += node.data.len().div_ceil(SECTOR) as u32;
            }
        }
        let mini_start = next_sector;
        let mini_count = mini_stream.len().div_ceil(SECTOR) as u32;
        let minifat_start = mini_start + mini_count;
        let minifat_count = (minifat.len() * 4).div_ceil(SECTOR) as u32;
        let dir_start = minifat_start + minifat_count;
        let dir_count = (order.len() * 128).div_ceil(SECTOR) as u32;
        let used = dir_start + dir_count;
        let (mut fat_count, mut difat_count) = (0u32, 0u32);
        loop {
            let total = used + fat_count + difat_count;
            let f = total.div_ceil(128);
            let d = if f > 109 { (f - 109).div_ceil(127) } else { 0 };
            if f == fat_count && d == difat_count {
                break;
            }
            (fat_count, difat_count) = (f, d);
        }
        let fat_start = used;
        let difat_start = fat_start + fat_count;
        let total = difat_start + difat_count;

        let mut fat = vec![FREESECT; (fat_count * 128) as usize];
        let run = |from: u32, count: u32, fat: &mut Vec<u32>| {
            for k in 0..count {
                fat[(from + k) as usize] = if k + 1 == count {
                    ENDOFCHAIN
                } else {
                    from + k + 1
                };
            }
        };
        for (i, s) in &big {
            let count = self.nodes[order[*i]].data.len().div_ceil(SECTOR) as u32;
            run(*s, count, &mut fat);
        }
        run(mini_start, mini_count, &mut fat);
        run(minifat_start, minifat_count, &mut fat);
        run(dir_start, dir_count, &mut fat);
        for k in 0..fat_count {
            fat[(fat_start + k) as usize] = FATSECT;
        }
        for k in 0..difat_count {
            fat[(difat_start + k) as usize] = DIFSECT;
        }

        let mut out = vec![0u8; SECTOR * (total as usize + 1)];
        let put32 = |out: &mut Vec<u8>, at: usize, v: u32| {
            out[at..at + 4].copy_from_slice(&v.to_le_bytes())
        };
        out[..8].copy_from_slice(&SIGNATURE);
        out[0x18..0x1A].copy_from_slice(&0x003Eu16.to_le_bytes());
        out[0x1A..0x1C].copy_from_slice(&3u16.to_le_bytes());
        out[0x1C..0x1E].copy_from_slice(&0xFFFEu16.to_le_bytes());
        out[0x1E..0x20].copy_from_slice(&9u16.to_le_bytes());
        out[0x20..0x22].copy_from_slice(&6u16.to_le_bytes());
        put32(&mut out, 0x2C, fat_count);
        put32(&mut out, 0x30, dir_start);
        put32(&mut out, 0x38, MINI_CUTOFF as u32);
        put32(
            &mut out,
            0x3C,
            if minifat_count > 0 {
                minifat_start
            } else {
                ENDOFCHAIN
            },
        );
        put32(&mut out, 0x40, minifat_count);
        put32(
            &mut out,
            0x44,
            if difat_count > 0 {
                difat_start
            } else {
                ENDOFCHAIN
            },
        );
        put32(&mut out, 0x48, difat_count);
        for i in 0..109u32 {
            let v = if i < fat_count {
                fat_start + i
            } else {
                FREESECT
            };
            put32(&mut out, 0x4C + 4 * i as usize, v);
        }
        let sector_off = |id: u32| (id as usize + 1) * SECTOR;
        for (i, s) in &big {
            let data = &self.nodes[order[*i]].data;
            let at = sector_off(*s);
            out[at..at + data.len()].copy_from_slice(data);
        }
        let at = sector_off(mini_start);
        out[at..at + mini_stream.len()].copy_from_slice(&mini_stream);
        let at = sector_off(minifat_start);
        for (k, v) in minifat.iter().enumerate() {
            put32(&mut out, at + 4 * k, *v);
        }
        for k in minifat.len()..(minifat_count as usize * 128) {
            put32(&mut out, at + 4 * k, FREESECT);
        }
        let at = sector_off(fat_start);
        for (k, v) in fat.iter().enumerate() {
            put32(&mut out, at + 4 * k, *v);
        }
        for d in 0..difat_count {
            let at = sector_off(difat_start + d);
            for k in 0..127u32 {
                let i = 109 + d * 127 + k;
                put32(
                    &mut out,
                    at + 4 * k as usize,
                    if i < fat_count {
                        fat_start + i
                    } else {
                        FREESECT
                    },
                );
            }
            let next = if d + 1 < difat_count {
                difat_start + d + 1
            } else {
                ENDOFCHAIN
            };
            put32(&mut out, at + 508, next);
        }
        let dir_at = sector_off(dir_start);
        for slot in 0..(dir_count as usize * 4) {
            let at = dir_at + slot * 128;
            put32(&mut out, at + 0x44, NOSTREAM);
            put32(&mut out, at + 0x48, NOSTREAM);
            put32(&mut out, at + 0x4C, NOSTREAM);
            let Some(n) = order.get(slot) else { continue };
            let node = &self.nodes[*n];
            let units: Vec<u16> = node.name.encode_utf16().take(31).collect();
            for (k, u) in units.iter().enumerate() {
                out[at + 2 * k..at + 2 * k + 2].copy_from_slice(&u.to_le_bytes());
            }
            out[at + 0x40..at + 0x42]
                .copy_from_slice(&((units.len() as u16 + 1) * 2).to_le_bytes());
            out[at + 0x42] = match (slot, node.kind) {
                (0, _) => 5,
                (_, Kind::Storage) => 1,
                (_, Kind::Stream) => 2,
            };
            out[at + 0x43] = if red[slot] { 0 } else { 1 };
            put32(&mut out, at + 0x44, left[slot]);
            put32(&mut out, at + 0x48, right[slot]);
            put32(&mut out, at + 0x4C, child[slot]);
            // A stream carries neither class nor state bits; some writers
            // leave garbage there, which strict readers refuse.
            if node.kind == Kind::Storage {
                out[at + 0x50..at + 0x60].copy_from_slice(&node.clsid);
                put32(&mut out, at + 0x60, node.state_bits);
            }
            let (first, size) = match (slot, node.kind) {
                (0, _) => (
                    if mini_count > 0 {
                        mini_start
                    } else {
                        ENDOFCHAIN
                    },
                    mini_stream.len(),
                ),
                (_, Kind::Storage) => (0, 0),
                (_, Kind::Stream) => (start[slot], node.data.len()),
            };
            put32(&mut out, at + 0x74, first);
            put32(&mut out, at + 0x78, size as u32);
        }
        out
    }
}

/// The order a directory's sibling tree is sorted by: shorter names first,
/// then code unit by code unit, upper-cased.
fn name_order(a: &str, b: &str) -> std::cmp::Ordering {
    let ua: Vec<u16> = a.encode_utf16().collect();
    let ub: Vec<u16> = b.encode_utf16().collect();
    ua.len().cmp(&ub.len()).then_with(|| {
        let up = |u: &u16| {
            char::from_u32(*u as u32)
                .and_then(|c| {
                    let mut up = c.to_uppercase();
                    (up.len() == 1).then(|| up.next().unwrap() as u32 as u16)
                })
                .unwrap_or(*u)
        };
        ua.iter().map(up).cmp(ub.iter().map(up))
    })
}

/// Middle-split tree over sorted `ids`; the deepest level is red, which keeps
/// every path at the same black height.
fn balance(
    ids: &[u32],
    depth: u32,
    max: u32,
    left: &mut [u32],
    right: &mut [u32],
    red: &mut [bool],
) -> u32 {
    if ids.is_empty() {
        return NOSTREAM;
    }
    let mid = ids.len() / 2;
    let me = ids[mid];
    red[me as usize] = depth == max && max > 0;
    left[me as usize] = balance(&ids[..mid], depth + 1, max, left, right, red);
    right[me as usize] = balance(&ids[mid + 1..], depth + 1, max, left, right, red);
    me
}
