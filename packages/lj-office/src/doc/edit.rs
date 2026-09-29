//! Replacing runs of characters of a Word 6 to 2003 document by texts of any
//! length. A replaced character is zeroed and no piece maps it any more; the
//! new text goes over it when it fits there, else at the end of the stream.

use std::ops::Range;

use anyhow::{bail, ensure, Context, Result};

use super::bytes::{u16_at, u32_at};
use super::cfb::Cfb;
use super::codepage::Codepage;
use super::fib::{lcb, Fib, Generation};
use super::fkp::{self, PAGE};
use super::sprm::{code, sprms};
use super::text::Story;
use super::{word6, Doc};

/// Characters `cps` of one story replaced by `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement {
    pub cps: Range<u32>,
    pub text: String,
}

/// Where the positions of one CP space go once its edits are applied.
#[derive(Clone, Debug, Default)]
pub struct Shift {
    /// Each removed run with the length of its new text, sorted, disjoint.
    edits: Vec<(Range<u32>, u32)>,
    /// Change in length up to and including each edit.
    change: Vec<i64>,
}

impl Shift {
    pub fn new(mut edits: Vec<(Range<u32>, u32)>) -> Self {
        edits.sort_by_key(|(r, _)| r.start);
        let mut total = 0i64;
        let change = edits
            .iter()
            .map(|(r, n)| {
                total += i64::from(*n) - i64::from(r.end - r.start);
                total
            })
            .collect();
        Self { edits, change }
    }

    /// The edits inside `range`, counted from its start.
    pub fn within(&self, range: Range<u32>) -> Self {
        Self::new(
            self.edits
                .iter()
                .filter(|(r, _)| range.start <= r.start && r.end <= range.end)
                .map(|(r, n)| (r.start - range.start..r.end - range.start, *n))
                .collect(),
        )
    }

    pub fn change(&self) -> i64 {
        self.change.last().copied().unwrap_or(0)
    }

    /// The new place of the boundary before character `p`: unmoved before
    /// an edit, moved by its change past it, and inside a removed run as far
    /// into the new text, at most to its end.
    pub fn map(&self, p: u32) -> u32 {
        let k = self.edits.partition_point(|(r, _)| r.end <= p);
        let before = if k == 0 { 0 } else { self.change[k - 1] };
        let moved = match self.edits.get(k) {
            Some((r, n)) if r.start < p => {
                i64::from(r.start) + before + i64::from((p - r.start).min(*n))
            }
            _ => i64::from(p) + before,
        };
        moved.clamp(0, i64::from(u32::MAX)) as u32
    }
}

/// The space a table's positions count in: every story end to end, or one
/// story from its first character.
#[derive(Clone, Copy, Debug)]
enum Space {
    All,
    In(Story),
}

/// The PLCs of FibRgFcLcb97 and later whose positions are CPs: their pair,
/// the size of their data elements, and the space they count in.
const PLCS: &[(usize, usize, Space)] = {
    use Space::{All, In};
    use Story::{Comments, Endnotes, Footnotes, HeaderTextboxes, Headers, Main, Textboxes};
    &[
        (2, 2, In(Main)),              // PlcffndRef
        (3, 0, In(Footnotes)),         // PlcffndTxt
        (4, 30, In(Main)),             // PlcfandRef
        (5, 0, In(Comments)),          // PlcfandTxt
        (6, 12, In(Main)),             // PlcfSed
        (10, 0, In(Main)),             // PlcfGlsy
        (11, 0, In(Headers)),          // PlcfHdd
        (16, 2, In(Main)),             // PlcfFldMom
        (17, 2, In(Headers)),          // PlcfFldHdr
        (18, 2, In(Footnotes)),        // PlcfFldFtn
        (19, 2, In(Comments)),         // PlcfFldAtn
        (22, 4, All),                  // PlcfBkf
        (23, 0, All),                  // PlcfBkl
        (40, 26, In(Main)),            // PlcSpaMom
        (41, 26, In(Headers)),         // PlcSpaHdr
        (42, 4, All),                  // PlcfAtnBkf
        (43, 0, All),                  // PlcfAtnBkl
        (46, 2, In(Main)),             // PlcfendRef
        (47, 0, In(Endnotes)),         // PlcfendTxt
        (48, 2, In(Endnotes)),         // PlcfFldEdn
        (54, 12, In(Main)),            // PlcfWkb
        (55, 2, All),                  // PlcfSpl
        (56, 22, In(Textboxes)),       // PlcftxbxTxt
        (57, 2, In(Textboxes)),        // PlcfFldTxbx
        (58, 22, In(HeaderTextboxes)), // PlcfHdrtxbxTxt
        (59, 2, In(HeaderTextboxes)),  // PlcffldHdrTxbx
        (75, 6, In(Textboxes)),        // PlcfTxbxBkd
        (76, 6, In(HeaderTextboxes)),  // PlcfTxbxHdrBkd
        (89, 4, All),                  // PlcfAsumy
        (90, 2, All),                  // PlcfGram
        (93, 4, In(Main)),             // PlcfTch
        (98, 2, All),                  // PlcfLad
        (101, 16, All),                // PlcfCookieOld
        (110, 20, All),                // Plcfuim
        (115, 6, All),                 // PlcfBkfFactoid
        (116, 10, All),                // Plcfcookie
        (117, 4, All),                 // PlcfBklFactoid
        (121, 6, All),                 // PlcfBkfFcc
        (122, 4, All),                 // PlcfBklFcc
        (124, 4, All),                 // PlcfbkfBPRepairs
        (125, 0, All),                 // PlcfbklBPRepairs
        (132, 2, All),                 // Plcffactoid
        (138, 10, All),                // PlcfBkfSdt
        (139, 8, All),                 // PlcfBklSdt
        (142, 6, All),                 // PlcfBkfProt
        (143, 0, All),                 // PlcfBklProt
    ]
};

/// Layout, undo and paragraph caches whose records hold CPs, which readers
/// are told to ignore: dropped rather than left stale.
const CACHES: &[Range<usize>] = &[
    7..9,     // PlcPad, PlcfPhe
    34..35,   // PlcfPgdFtn
    63..69,   // Pgd, Bkd OldOld
    77..83,   // undo
    88..89,   // PlcfLvcPre10
    102..108, // Pgd, Bkd Old
    119..120, // DocUndo
    128..131, // Plcfpmi XP
    133..136, // Plcflvc XP
    146..165, // Plcfpmi, Plcflvc, Pgd, Bkd, Afd, Plcfmthd
];

/// `PLCS` for Word 6, whose FIB numbers its pairs as FibRgFcLcb97 does.
const PLCS6: &[(usize, usize, Space)] = {
    use Space::{All, In};
    use Story::{Comments, Endnotes, Footnotes, HeaderTextboxes, Headers, Macro, Main, Textboxes};
    &[
        (2, 2, In(Main)),             // PlcffndRef
        (3, 0, In(Footnotes)),        // PlcffndTxt
        (4, 20, In(Main)),            // PlcfandRef
        (5, 0, In(Comments)),         // PlcfandTxt
        (6, 12, In(Main)),            // PlcfSed
        (10, 0, In(Main)),            // PlcfGlsy
        (11, 0, In(Headers)),         // PlcfHdd
        (16, 2, In(Main)),            // PlcfFldMom
        (17, 2, In(Headers)),         // PlcfFldHdr
        (18, 2, In(Footnotes)),       // PlcfFldFtn
        (19, 2, In(Comments)),        // PlcfFldAtn
        (20, 2, In(Macro)),           // PlcfFldMcr
        (22, 4, All),                 // PlcfBkf
        (23, 0, All),                 // PlcfBkl
        (38, 6, In(Main)),            // PlcfdoaMom
        (39, 6, In(Headers)),         // PlcfdoaHdr
        (42, 4, All),                 // PlcfAtnBkf
        (43, 0, All),                 // PlcfAtnBkl
        (46, 2, In(Main)),            // PlcfendRef
        (47, 0, In(Endnotes)),        // PlcfendTxt
        (48, 2, In(Endnotes)),        // PlcfFldEdn
        (56, 0, In(Textboxes)),       // PlcftxbxTxt
        (57, 2, In(Textboxes)),       // PlcfFldTxbx
        (58, 0, In(HeaderTextboxes)), // PlcfHdrtxbxTxt
        (59, 2, In(HeaderTextboxes)), // PlcffldHdrTxbx
    ]
};

/// `CACHES` for Word 6: PlcPad, PlcfPhe, PlcfPgdFtn, PlcfPgdEdn, PlcfSpl.
const CACHES6: &[usize] = &[7, 8, 34, 49, 55];

const PLCFBTELVC: usize = 86;
/// ccpText and the seven story lengths after it, in FibRgLw97.
const CCP_AT: usize = 0x4C;
const CB_MAC_AT: usize = 0x40;
const CCP_AT6: usize = 0x34;
const CB_MAC_AT6: usize = 0x20;
const FLAGS_AT: usize = 0x0A;
const COMPLEX: u16 = 0x0004;
/// Word 6: pnChpFirst and pnPapFirst, then cpnBteChp and cpnBtePap.
const PN_FIRST6: usize = 0x18A;
const CPN_BTE6: usize = 0x18E;
/// Dop2002 cpMinRMText and the six story CPs after it.
const DOP_CP_MIN_RM: usize = 562;
/// fNoParaLast: the piece holds no paragraph mark.
const NO_PARA_LAST: u16 = 1;

#[derive(Clone, Copy, Debug)]
struct Pcd {
    cp: u32,
    cp_end: u32,
    flags: u16,
    fc: u32,
    compressed: bool,
    prm: u16,
}

impl Pcd {
    fn fc_of(&self, cp: u32) -> u32 {
        self.fc + (cp - self.cp) * if self.compressed { 1 } else { 2 }
    }

    /// Whether the bytes of the piece end within 4 GiB, which every FC this
    /// module computes from it assumes.
    fn addressable(&self) -> bool {
        let width = if self.compressed { 1 } else { 2 };
        u64::from(self.fc) + u64::from(self.cp_end.saturating_sub(self.cp)) * width
            <= u64::from(u32::MAX)
    }
}

/// A piece of the new table, by length.
struct Piece {
    len: u32,
    flags: u16,
    fc: u32,
    compressed: bool,
    prm: u16,
}

impl Piece {
    fn bytes(&self) -> Range<u32> {
        self.fc..self.fc + self.len * if self.compressed { 1 } else { 2 }
    }
}

/// Apply `replacements` to a document: disjoint runs, each inside one story
/// and holding no character a table anchors to, replaced by texts without
/// control characters. Returns the container with its WordDocument and
/// table streams rewritten.
pub fn replace(doc: &Doc, replacements: &[Replacement]) -> Result<Cfb> {
    let mut edits: Vec<&Replacement> = replacements.iter().collect();
    edits.sort_by_key(|e| e.cps.start);
    validate(doc, &edits)?;
    let fib = &doc.fib;
    let w6 = fib.generation == Generation::Word6;
    let src = doc.word_document();
    let source_table = doc.table_stream();
    let (prc, pcds) = if w6 && !fib.complex {
        let whole = Pcd {
            cp: 0,
            cp_end: fib.ccp_all(),
            flags: 0,
            fc: fib.fc_min,
            compressed: !fib.ext_char,
            prm: 0,
        };
        ensure!(whole.addressable(), "the text lies past 4 GiB");
        (Vec::new(), vec![whole])
    } else {
        read_clx(fib, fib.table(source_table, lcb::CLX))?
    };
    let Some(total) = pcds.last().map(|p| p.cp_end) else {
        bail!("empty piece table");
    };

    let mut pieces: Vec<Piece> = Vec::new();
    let mut texts: Vec<(usize, Vec<u16>, Pcd, Range<u32>)> = Vec::new();
    let mut cursor = 0;
    let keep = |pieces: &mut Vec<Piece>, range: Range<u32>| {
        for p in pcds
            .iter()
            .filter(|p| p.cp < range.end && range.start < p.cp_end)
        {
            let start = p.cp.max(range.start);
            pieces.push(Piece {
                len: p.cp_end.min(range.end) - start,
                flags: p.flags,
                fc: p.fc_of(start),
                compressed: p.compressed,
                prm: p.prm,
            });
        }
    };
    for e in &edits {
        keep(&mut pieces, cursor..e.cps.start);
        let base = *pcds
            .iter()
            .find(|p| p.cp <= e.cps.start && e.cps.start < p.cp_end)
            .context("replacement outside the piece table")?;
        let units: Vec<u16> = e.text.encode_utf16().collect();
        if !units.is_empty() {
            texts.push((pieces.len(), units, base, e.cps.clone()));
            pieces.push(Piece {
                len: 0,
                flags: base.flags | NO_PARA_LAST,
                fc: 0,
                compressed: base.compressed,
                prm: base.prm,
            });
        }
        cursor = e.cps.end;
    }
    keep(&mut pieces, cursor..total);
    let placed: Vec<usize> = texts.iter().map(|t| t.0).collect();
    let kept = merged(
        pieces
            .iter()
            .enumerate()
            .filter(|(i, _)| !placed.contains(i))
            .map(|(_, p)| p.bytes())
            .collect(),
    );

    let mut wd = src.to_vec();
    for e in &edits {
        for p in pcds
            .iter()
            .filter(|p| p.cp < e.cps.end && e.cps.start < p.cp_end)
        {
            let from = p.fc_of(p.cp.max(e.cps.start));
            let to = p.fc_of(p.cp_end.min(e.cps.end));
            for at in (from..to).filter(|at| !inside(&kept, *at)) {
                if let Some(b) = wd.get_mut(at as usize) {
                    *b = 0;
                }
            }
        }
    }

    let pn_len = if w6 { 2 } else { 4 };
    let mut chpx_bte = bin_table(doc, lcb::PLCFBTECHPX, 0);
    let mut papx_bte = bin_table(doc, lcb::PLCFBTEPAPX, 1);
    let lvc_bte = fib.table(source_table, PLCFBTELVC);
    let tail_at = (wd.len() as u32)
        .max(bte_end(&chpx_bte, pn_len))
        .max(bte_end(&papx_bte, pn_len).saturating_add(1))
        .max(bte_end(lvc_bte, 4).saturating_add(1))
        .next_multiple_of(2);
    // An FC past the stream maps nothing: one this far past it is a corrupt bin table.
    ensure!(
        tail_at as usize <= wd.len() + (16 << 20),
        "the bin tables reach byte {tail_at}, past the {} bytes of the WordDocument stream",
        wd.len()
    );
    let raw6 = w6.then(|| fkp::chpx_pages(fkp::pages(src, &chpx_bte, 2)));
    let chpx = raw6.as_deref().unwrap_or(&doc.chpx);
    let mut tail: Vec<u8> = Vec::new();
    let mut runs: Vec<(u32, Option<Vec<u8>>)> = Vec::new();
    for (i, units, base, cps) in &texts {
        let at = base.fc_of(cps.start);
        let (bytes, compressed, charset) = encode(doc, units, base, at)?;
        let room = base.fc_of(cps.end.min(base.cp_end)) - at;
        let len = bytes.len() as u32;
        let fc = if charset.is_none()
            && compressed == base.compressed
            && len <= room
            && !overlaps(&kept, at..at + len)
            && (at + len) as usize <= wd.len()
        {
            wd[at as usize..(at + len) as usize].copy_from_slice(&bytes);
            at
        } else {
            tail.resize(tail.len().next_multiple_of(2), 0);
            let fc = tail_at + tail.len() as u32;
            tail.extend_from_slice(&bytes);
            let grpprl = doc
                .text
                .bytes_of(cps.start)
                .and_then(|b| fkp::find(chpx, b.start as u32))
                .map(|r| match w6 {
                    true => word6::plain(&chpx[r].grpprl, charset),
                    false => plain(&chpx[r].grpprl),
                })
                .unwrap_or_default();
            ensure!(
                grpprl.len() < 256,
                "character properties past one FKP entry"
            );
            runs.push((fc, Some(grpprl)));
            fc
        };
        pieces[*i].len = units.len() as u32;
        pieces[*i].fc = fc;
        pieces[*i].compressed = compressed;
    }

    let mut btes: Vec<usize> = Vec::new();
    if !tail.is_empty() {
        wd.resize(tail_at as usize, 0);
        wd.extend_from_slice(&tail);
        let end = wd.len() as u32;
        let chpx_end = bte_end(&chpx_bte, pn_len);
        if bte_count(&chpx_bte, pn_len) > 0 && chpx_end < runs[0].0 {
            runs.insert(0, (chpx_end, None));
        }
        wd.resize(wd.len().next_multiple_of(PAGE), 0);
        let mut pages = Vec::new();
        for (first, page) in chpx_pages(&runs, end) {
            pages.push((first, (wd.len() / PAGE) as u32));
            wd.extend_from_slice(&page);
        }
        chpx_bte = extend_bte(&chpx_bte, pn_len, &pages, end)?;
        let first = match bte_count(&papx_bte, pn_len) {
            0 => tail_at,
            _ => bte_end(&papx_bte, pn_len),
        };
        let pn = (wd.len() / PAGE) as u32;
        wd.extend_from_slice(&papx_page(first, end + 1, w6));
        papx_bte = extend_bte(&papx_bte, pn_len, &[(first, pn)], end + 1)?;
        btes = vec![lcb::PLCFBTECHPX, lcb::PLCFBTEPAPX];
    }

    let mut table = match w6 {
        true => std::mem::take(&mut wd),
        false => source_table.to_vec(),
    };
    let mut pairs: Vec<(usize, u32, u32)> = Vec::new();
    for &index in &btes {
        let bte = match index {
            lcb::PLCFBTECHPX => &chpx_bte,
            _ => &papx_bte,
        };
        pairs.push((index, table.len() as u32, bte.len() as u32));
        table.extend_from_slice(bte);
    }
    let mut clx = prc;
    let mut plc = Vec::new();
    let mut cp = 0u32;
    for p in &pieces {
        plc.extend_from_slice(&cp.to_le_bytes());
        cp += p.len;
    }
    plc.extend_from_slice(&cp.to_le_bytes());
    for p in &pieces {
        let fc = if p.compressed && !w6 {
            ensure!(p.fc < 1 << 29, "text past the reach of an 8-bit piece");
            (p.fc * 2) | 0x4000_0000
        } else {
            ensure!(p.fc < 1 << 30, "text past the reach of a piece");
            p.fc
        };
        plc.extend_from_slice(&p.flags.to_le_bytes());
        plc.extend_from_slice(&fc.to_le_bytes());
        plc.extend_from_slice(&p.prm.to_le_bytes());
    }
    clx.push(2);
    clx.extend_from_slice(&(plc.len() as u32).to_le_bytes());
    clx.extend_from_slice(&plc);
    pairs.push((lcb::CLX, table.len() as u32, clx.len() as u32));
    table.extend_from_slice(&clx);

    let global = Shift::new(
        edits
            .iter()
            .map(|e| (e.cps.clone(), e.text.encode_utf16().count() as u32))
            .collect(),
    );
    let mut stories: [Range<u32>; 8] = Default::default();
    let mut at = 0u32;
    for (s, len) in stories.iter_mut().zip(fib.ccp) {
        *s = at..at.saturating_add(len);
        at = at.saturating_add(len);
    }
    let shift_of = |space: Space| match space {
        Space::All => global.clone(),
        Space::In(story) => global.within(stories[story.index() as usize].clone()),
    };
    for &(index, cb, space) in if w6 { PLCS6 } else { PLCS } {
        let (fc, len) = pair(doc, index);
        if len > 0 {
            shift_plc(
                &mut table,
                source_table,
                fc as usize..(fc as usize).saturating_add(len as usize),
                cb,
                &shift_of(space),
            );
        }
    }
    let (dop, dop_len) = fib.pair(lcb::DOP);
    if dop_len as usize >= DOP_CP_MIN_RM + 32 {
        use Story::{Comments, Endnotes, Footnotes, HeaderTextboxes, Headers, Main, Textboxes};
        let order = [
            Main,
            Footnotes,
            Headers,
            Comments,
            Endnotes,
            Textboxes,
            HeaderTextboxes,
        ];
        for (k, story) in order.into_iter().enumerate() {
            let at = (dop as usize).saturating_add(DOP_CP_MIN_RM + 4 * k);
            match u32_at(source_table, at) {
                Some(v) if v > 0 && v < 0x8000_0000 => {
                    let moved = shift_of(Space::In(story)).map(v);
                    table[at..at + 4].copy_from_slice(&moved.to_le_bytes());
                }
                _ => {}
            }
        }
    }
    if w6 {
        wd = std::mem::take(&mut table);
    }

    let caches: Vec<usize> = match w6 {
        true => CACHES6.to_vec(),
        false => CACHES.iter().flat_map(|r| r.clone()).collect(),
    };
    for index in caches {
        let (fc, len) = pair(doc, index);
        if len > 0 {
            pairs.push((index, fc, 0));
        }
    }
    for (index, fc, len) in pairs {
        let at = pair_at(doc, index).context("pair outside the FIB")?;
        wd[at..at + 4].copy_from_slice(&fc.to_le_bytes());
        wd[at + 4..at + 8].copy_from_slice(&len.to_le_bytes());
    }
    let (ccp_at, cb_mac_at) = match w6 {
        true => (CCP_AT6, CB_MAC_AT6),
        false => (CCP_AT, CB_MAC_AT),
    };
    for (i, s) in stories.iter().enumerate() {
        let change = global.within(s.clone()).change();
        if change != 0 {
            let ccp = u32::try_from(i64::from(fib.ccp[i]) + change).context("story length")?;
            let at = ccp_at + 4 * i;
            wd[at..at + 4].copy_from_slice(&ccp.to_le_bytes());
        }
    }
    if w6 {
        let flags = u16_at(&wd, FLAGS_AT).unwrap_or(0) | COMPLEX;
        wd[FLAGS_AT..FLAGS_AT + 2].copy_from_slice(&flags.to_le_bytes());
        for (k, bte) in [&chpx_bte, &papx_bte].into_iter().enumerate() {
            let count = u16::try_from(bte_count(bte, 2)).context("bin table size")?;
            let at = CPN_BTE6 + 2 * k;
            wd[at..at + 2].copy_from_slice(&count.to_le_bytes());
        }
    }
    let len = wd.len() as u32;
    wd[cb_mac_at..cb_mac_at + 4].copy_from_slice(&len.to_le_bytes());

    let live: Vec<Range<u32>> = pieces.iter().map(Piece::bytes).collect();
    let tables: &[u8] = if w6 { &wd } else { &table };
    let kept = protected(doc, tables, [&chpx_bte, &papx_bte, lvc_bte], live);
    erase_leftovers(doc, &mut wd, &kept, &edits);

    let mut cfb = doc.cfb.clone();
    *cfb.stream_mut("WordDocument")
        .context("no WordDocument stream")? = wd;
    if !w6 {
        *cfb.stream_mut(doc.table_name).context("no table stream")? = table;
    }
    Ok(cfb)
}

/// The bytes of `units` in a piece like `base`, 8-bit when it is. Readers
/// disagree on the code page of a Word 6 run, so 8-bit text beyond ASCII
/// comes with the character set the run must state.
fn encode(doc: &Doc, units: &[u16], base: &Pcd, fc: u32) -> Result<(Vec<u8>, bool, Option<u8>)> {
    let narrow =
        |cp: Codepage| -> Option<Vec<u8>> { units.iter().map(|u| cp.encode(*u)).collect() };
    let wide = || units.iter().flat_map(|u| u.to_le_bytes()).collect();
    if !base.compressed {
        return Ok((wide(), false, None));
    }
    if doc.fib.generation == Generation::Word97 {
        return Ok(match narrow(Codepage::windows(1252)) {
            Some(bytes) => (bytes, true, None),
            None => (wide(), false, None),
        });
    }
    if units.iter().all(|u| *u < 0x80) {
        return Ok((units.iter().map(|u| *u as u8).collect(), true, None));
    }
    let run = word6::charset_at(fc, &doc.chpx, &doc.papx, &doc.styles, &doc.fonts);
    for charset in [Some(0), run].into_iter().flatten() {
        let Some(number) = word6::charset_codepage(charset) else {
            continue;
        };
        if let Some(bytes) = narrow(Codepage::windows(number)) {
            return Ok((bytes, true, Some(charset)));
        }
    }
    bail!("no character set writes the text replacing FC {fc}")
}

fn validate(doc: &Doc, edits: &[&Replacement]) -> Result<()> {
    let chars = &doc.text.chars;
    let mut end = 0;
    for e in edits {
        let Range { start, end: stop } = e.cps;
        ensure!(start < stop, "empty replacement at CP {start}");
        ensure!(start >= end, "replacements overlap at CP {start}");
        end = stop;
        let Some((_, story)) = doc
            .text
            .stories
            .iter()
            .find(|(_, r)| r.start <= start && start < r.end)
        else {
            bail!("CP {start} is in no story");
        };
        ensure!(
            stop <= story.end,
            "replacement at CP {start} leaves its story"
        );
        for cp in start..stop {
            let unit = chars[cp as usize];
            ensure!(
                unit >= 0x20 || matches!(unit, 0x09 | 0x0B | 0x1E | 0x1F),
                "CP {cp} holds {unit:#06x}, which a table anchors to"
            );
        }
        ensure!(
            !(0xDC00..0xE000).contains(&chars[start as usize])
                && !(0xD800..0xDC00).contains(&chars[stop as usize - 1]),
            "replacement at CP {start} splits a surrogate pair"
        );
        ensure!(
            e.text.chars().all(|c| c >= ' '),
            "replacement text at CP {start} holds a control character"
        );
    }
    Ok(())
}

/// The Prc entries of a CLX, verbatim, and its pieces. A Word 6 piece is
/// 8-bit unless the whole document is 16-bit.
fn read_clx(fib: &Fib, clx: &[u8]) -> Result<(Vec<u8>, Vec<Pcd>)> {
    let mut at = 0usize;
    let plc = loop {
        match clx.get(at) {
            Some(1) => at += 3 + u16_at(clx, at + 1).context("truncated Prc")? as usize,
            Some(2) => {
                let len = u32_at(clx, at + 1).context("truncated Pcdt")? as usize;
                break (at + 5)
                    .checked_add(len)
                    .and_then(|end| clx.get(at + 5..end))
                    .context("truncated piece table")?;
            }
            _ => bail!("CLX without a piece table"),
        }
    };
    let prc = clx[..at].to_vec();
    ensure!(
        plc.len() >= 16 && (plc.len() - 4) % 12 == 0,
        "piece table of {} bytes",
        plc.len()
    );
    let n = (plc.len() - 4) / 12;
    let mut pcds: Vec<Pcd> = Vec::with_capacity(n);
    for i in 0..n {
        let cp = u32_at(plc, 4 * i).unwrap_or(0);
        let cp_end = u32_at(plc, 4 * (i + 1)).unwrap_or(0);
        let at = 4 * (n + 1) + 8 * i;
        let raw = u32_at(plc, at + 2).unwrap_or(0);
        let flagged = raw & 0x4000_0000 != 0;
        pcds.push(Pcd {
            cp,
            cp_end,
            flags: u16_at(plc, at).unwrap_or(0),
            fc: if flagged {
                (raw & 0x3FFF_FFFF) / 2
            } else {
                raw
            },
            compressed: match fib.generation {
                Generation::Word97 => flagged,
                Generation::Word6 => !fib.ext_char,
            },
            prm: u16_at(plc, at + 6).unwrap_or(0),
        });
    }
    if let Some(i) = pcds.iter().position(|p| !p.addressable()) {
        bail!("piece {i} lies past 4 GiB");
    }
    let pcds = laid_out(pcds);
    let mut cp = 0;
    for p in &pcds {
        ensure!(p.cp == cp, "no piece maps CP {cp}");
        cp = p.cp_end;
    }
    Ok((prc, pcds))
}

/// Pieces as the reader lays them out: one that ends before it starts maps
/// nothing, and a later one overwrites what an earlier one maps.
fn laid_out(pcds: Vec<Pcd>) -> Vec<Pcd> {
    let mut pcds: Vec<Pcd> = pcds.into_iter().filter(|p| p.cp < p.cp_end).collect();
    if pcds.windows(2).all(|w| w[0].cp_end <= w[1].cp) {
        return pcds;
    }
    let mut out: Vec<Pcd> = Vec::with_capacity(pcds.len());
    for p in pcds.drain(..) {
        let mut next = Vec::with_capacity(out.len() + 2);
        for q in out {
            if q.cp_end <= p.cp || p.cp_end <= q.cp {
                next.push(q);
                continue;
            }
            if q.cp < p.cp {
                next.push(Pcd { cp_end: p.cp, ..q });
            }
            if p.cp_end < q.cp_end {
                next.push(Pcd {
                    cp: p.cp_end,
                    fc: q.fc_of(p.cp_end),
                    ..q
                });
            }
        }
        next.push(p);
        next.sort_by_key(|q| q.cp);
        out = next;
    }
    out
}

fn bte_count(bte: &[u8], pn_len: usize) -> usize {
    bte.len().saturating_sub(4) / (4 + pn_len)
}

/// The last FC of a bin table whose page numbers are `pn_len` bytes wide.
fn bte_end(bte: &[u8], pn_len: usize) -> u32 {
    u32_at(bte, 4 * bte_count(bte, pn_len)).unwrap_or(0)
}

/// A bin table with `pages`, as (first FC, page number), after its own
/// entries, the last page ending at `end`.
fn extend_bte(bte: &[u8], pn_len: usize, pages: &[(u32, u32)], end: u32) -> Result<Vec<u8>> {
    let n = bte_count(bte, pn_len);
    let mut out = bte[..4 * n].to_vec();
    for (first, _) in pages {
        out.extend_from_slice(&first.to_le_bytes());
    }
    out.extend_from_slice(&end.to_le_bytes());
    out.extend_from_slice(
        bte.get(4 * (n + 1)..4 * (n + 1) + pn_len * n)
            .unwrap_or(&[]),
    );
    let reach = if pn_len == 2 { 1 << 16 } else { 1 << 22 };
    for (_, pn) in pages {
        ensure!(*pn < reach, "page {pn} past the reach of a bin table");
        out.extend_from_slice(&pn.to_le_bytes()[..pn_len]);
    }
    Ok(out)
}

/// A paragraph FKP of one run from `first` to `lim` in the default style.
/// A reader that finds no paragraph run at the first character of a story
/// drops the paragraph properties of the whole story.
fn papx_page(first: u32, lim: u32, word6: bool) -> [u8; PAGE] {
    const PAPX_AT: usize = 500;
    let papx: &[u8] = if word6 { &[1, 0, 0] } else { &[0, 1, 0, 0] };
    let mut page = [0u8; PAGE];
    page[..4].copy_from_slice(&first.to_le_bytes());
    page[4..8].copy_from_slice(&lim.to_le_bytes());
    page[8] = (PAPX_AT / 2) as u8;
    page[PAPX_AT..PAPX_AT + papx.len()].copy_from_slice(papx);
    page[PAGE - 1] = 1;
    page
}

/// Bin table `index`, `k` 0 for characters and 1 for paragraphs. A Word 6
/// FIB may count more pages than its table holds: the table is then that
/// many pages from the first, one after the other.
fn bin_table(doc: &Doc, index: usize, k: usize) -> Vec<u8> {
    let stored = doc.fib.table(doc.table_stream(), index);
    let wd = doc.word_document();
    let first = u16_at(wd, PN_FIRST6 + 2 * k).unwrap_or(0) as usize;
    let count = u16_at(wd, CPN_BTE6 + 2 * k).unwrap_or(0) as usize;
    if doc.fib.generation == Generation::Word97 || bte_count(stored, 2) >= count {
        return stored.to_vec();
    }
    let mut out = Vec::new();
    for pn in first..first + count {
        out.extend_from_slice(&u32_at(wd, pn * PAGE).unwrap_or(0).to_le_bytes());
    }
    let last = (first + count - 1) * PAGE;
    let crun = wd.get(last + PAGE - 1).copied().unwrap_or(0) as usize;
    out.extend_from_slice(&u32_at(wd, last + 4 * crun).unwrap_or(0).to_le_bytes());
    for pn in first..first + count {
        out.extend_from_slice(&(pn as u16).to_le_bytes());
    }
    out
}

/// Where pair `index` sits in the FIB. Word 6 has the pairs of FibRgFcLcb97
/// up to the 62nd, with ten bytes between the 38th and the 39th.
fn pair_at(doc: &Doc, index: usize) -> Option<usize> {
    match doc.fib.generation {
        Generation::Word97 => doc.fib.pair_offset(doc.word_document(), index),
        Generation::Word6 if index < 38 => Some(0x58 + 8 * index),
        Generation::Word6 if index < 62 => Some(0x192 + 8 * (index - 38)),
        Generation::Word6 => None,
    }
}

/// The `(fc, lcb)` of pair `index` in the source FIB.
fn pair(doc: &Doc, index: usize) -> (u32, u32) {
    let wd = doc.word_document();
    pair_at(doc, index)
        .and_then(|at| Some((u32_at(wd, at)?, u32_at(wd, at + 4)?)))
        .unwrap_or((0, 0))
}

/// A character run's sprms, less those that make a character stand for an
/// object: the new text is plain text.
fn plain(grpprl: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for (sprm, operand) in sprms(grpprl) {
        if !matches!(
            sprm,
            code::C_SPEC
                | code::C_PIC_LOCATION
                | code::C_DATA
                | code::C_OLE2
                | code::C_OBJ
                | code::C_SYMBOL
                | 0x680E
        ) {
            out.extend_from_slice(&sprm.to_le_bytes());
            out.extend_from_slice(operand);
        }
    }
    out
}

/// Character FKPs over contiguous `runs`, each from its start to the next
/// one's, the last to `end`, as (first FC, page).
fn chpx_pages(runs: &[(u32, Option<Vec<u8>>)], end: u32) -> Vec<(u32, [u8; PAGE])> {
    let mut pages = Vec::new();
    let mut i = 0;
    while i < runs.len() {
        let mut distinct: Vec<&[u8]> = Vec::new();
        let mut room = 0usize;
        let mut j = i;
        while j < runs.len() && j - i < 0x65 {
            let k = j - i + 1;
            let fresh = runs[j].1.as_deref().filter(|g| !distinct.contains(g));
            let extra = fresh.map_or(0, |g| (2 + g.len()) & !1);
            if 5 * k + 4 + room + extra + 1 > PAGE - 1 {
                break;
            }
            distinct.extend(fresh);
            room += extra;
            j += 1;
        }
        let crun = j - i;
        let mut page = [0u8; PAGE];
        for (k, (fc, _)) in runs[i..j].iter().enumerate() {
            page[4 * k..4 * k + 4].copy_from_slice(&fc.to_le_bytes());
        }
        let lim = runs.get(j).map_or(end, |r| r.0);
        page[4 * crun..4 * crun + 4].copy_from_slice(&lim.to_le_bytes());
        let mut top = PAGE - 1;
        let mut placed: Vec<(&[u8], u8)> = Vec::new();
        for (k, (_, grpprl)) in runs[i..j].iter().enumerate() {
            let offset = match grpprl.as_deref() {
                None => 0,
                Some(g) => match placed.iter().find(|(p, _)| *p == g) {
                    Some((_, o)) => *o,
                    None => {
                        top = (top - 1 - g.len()) & !1;
                        page[top] = g.len() as u8;
                        page[top + 1..top + 1 + g.len()].copy_from_slice(g);
                        placed.push((g, (top / 2) as u8));
                        (top / 2) as u8
                    }
                },
            };
            page[4 * (crun + 1) + k] = offset;
        }
        page[PAGE - 1] = crun as u8;
        pages.push((runs[i].0, page));
        i = j;
    }
    pages
}

/// Move the positions of the PLC at `span`, read from `source`. Positions
/// from 2^31 up are not CPs and stay.
fn shift_plc(table: &mut [u8], source: &[u8], span: Range<usize>, cb: usize, shift: &Shift) {
    if span.len() < 4 || span.end > source.len() {
        return;
    }
    let n = (span.len() - 4) / (4 + cb);
    for i in 0..=n {
        let at = span.start + 4 * i;
        let p = u32_at(source, at).unwrap_or(u32::MAX);
        if p < 0x8000_0000 {
            table[at..at + 4].copy_from_slice(&shift.map(p).to_le_bytes());
        }
    }
}

/// The bytes of the WordDocument stream a reader reads besides the text:
/// the FIB, the property pages of `btes`, the section properties, and for
/// Word 6, whose tables share the stream, every table.
fn protected(
    doc: &Doc,
    tables: &[u8],
    btes: [&[u8]; 3],
    mut kept: Vec<Range<u32>>,
) -> Vec<Range<u32>> {
    let fib = &doc.fib;
    let w6 = fib.generation == Generation::Word6;
    let wd = doc.word_document();
    kept.push(0..if w6 { fib.fc_min } else { fib_len(wd) });
    for (k, bte) in btes.into_iter().enumerate() {
        let pn_len = if w6 && k < 2 { 2 } else { 4 };
        let n = bte_count(bte, pn_len);
        for i in 0..n {
            let at = 4 * (n + 1) + pn_len * i;
            let pn = match pn_len {
                2 => u32::from(u16_at(bte, at).unwrap_or(0)),
                _ => u32_at(bte, at).unwrap_or(0) & 0x3F_FFFF,
            };
            kept.push(pn * PAGE as u32..(pn + 1) * PAGE as u32);
        }
    }
    let sed = fib.table(doc.table_stream(), lcb::PLCFSED);
    let n = sed.len().saturating_sub(4) / 16;
    for i in 0..n {
        let fc = u32_at(sed, 4 * (n + 1) + 12 * i + 2).unwrap_or(u32::MAX);
        if let Some(cb) = u16_at(wd, fc as usize) {
            kept.push(fc..fc + 2 + u32::from(cb));
        }
    }
    if w6 {
        for at in (0..62).filter_map(|index| pair_at(doc, index)) {
            let fc = u32_at(tables, at).unwrap_or(0);
            let len = u32_at(tables, at + 4).unwrap_or(0);
            kept.push(fc..fc.saturating_add(len));
        }
    }
    merged(kept)
}

/// Zero, outside `kept`, the copies of each replaced text a fast save left
/// behind, 8-bit or UTF-16, in any case.
fn erase_leftovers(doc: &Doc, wd: &mut [u8], kept: &[Range<u32>], edits: &[&Replacement]) {
    let mut needles: Vec<Vec<char>> = Vec::new();
    for e in edits {
        let old = &doc.text.chars[e.cps.start as usize..e.cps.end as usize];
        let folded: Vec<char> = char::decode_utf16(old.iter().copied())
            .map(|c| fold(c.unwrap_or('\u{FFFD}')))
            .collect();
        if folded.len() >= 3 && !needles.contains(&folded) {
            needles.push(folded);
        }
    }
    let codepage = match doc.fib.generation {
        Generation::Word97 => Codepage::windows(1252),
        Generation::Word6 => doc.codepage,
    };
    let narrow: Vec<char> = (0..=255u8)
        .map(|b| fold(char::from_u32(u32::from(codepage.decode(b))).unwrap_or('\0')))
        .collect();
    let wide: Vec<char> = (0..=u16::MAX)
        .map(|u| char::from_u32(u32::from(u)).map_or('\0', fold))
        .collect();
    for needle in &needles {
        let n = needle.len();
        let mut hits: Vec<Range<u32>> = Vec::new();
        let firsts: Vec<u8> = (0..=255u8)
            .filter(|b| narrow[*b as usize] == needle[0])
            .collect();
        let mut from = 0;
        while let Some(at) = next_start(wd, from, &firsts) {
            if at + n > wd.len() {
                break;
            }
            if (1..n).all(|k| narrow[wd[at + k] as usize] == needle[k]) {
                hits.push(at as u32..(at + n) as u32);
            }
            from = at + 1;
        }
        let mut lows: Vec<u8> = (0..=u16::MAX)
            .filter(|u| wide[*u as usize] == needle[0])
            .map(|u| u.to_le_bytes()[0])
            .collect();
        lows.sort_unstable();
        lows.dedup();
        let unit = |at: usize| wide[usize::from(u16::from_le_bytes([wd[at], wd[at + 1]]))];
        let mut from = 0;
        while let Some(at) = next_start(wd, from, &lows) {
            if at + 2 * n > wd.len() {
                break;
            }
            if (0..n).all(|k| unit(at + 2 * k) == needle[k]) {
                hits.push(at as u32..(at + 2 * n) as u32);
            }
            from = at + 1;
        }
        for hit in hits.into_iter().filter(|h| !overlaps(kept, h.clone())) {
            wd[hit.start as usize..hit.end as usize].fill(0);
        }
    }
}

/// The first position from `at` on holding one of `bytes`: a WordDocument
/// stream can run to tens of megabytes, searched once per replaced text.
fn next_start(data: &[u8], at: usize, bytes: &[u8]) -> Option<usize> {
    let rest = data.get(at..)?;
    let found = match *bytes {
        [] => None,
        [a] => memchr::memchr(a, rest),
        [a, b] => memchr::memchr2(a, b, rest),
        [a, b, c] => memchr::memchr3(a, b, c, rest),
        _ => rest.iter().position(|x| bytes.contains(x)),
    };
    found.map(|i| at + i)
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Bytes of a Word 97 FIB, to the end of its last part.
fn fib_len(wd: &[u8]) -> u32 {
    let csw = u16_at(wd, 0x20).unwrap_or(0) as usize;
    let at = 0x22 + 2 * csw;
    let cslw = u16_at(wd, at).unwrap_or(0) as usize;
    let at = at + 2 + 4 * cslw;
    let pairs = u16_at(wd, at).unwrap_or(0) as usize;
    let at = at + 2 + 8 * pairs;
    let csw_new = u16_at(wd, at).unwrap_or(0) as usize;
    (at + 2 + 2 * csw_new) as u32
}

fn merged(mut ranges: Vec<Range<u32>>) -> Vec<Range<u32>> {
    ranges.retain(|r| r.start < r.end);
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<Range<u32>> = Vec::new();
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

fn inside(ranges: &[Range<u32>], at: u32) -> bool {
    overlaps(ranges, at..at + 1)
}

fn overlaps(ranges: &[Range<u32>], span: Range<u32>) -> bool {
    let i = ranges.partition_point(|r| r.end <= span.start);
    ranges.get(i).is_some_and(|r| r.start < span.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shift(edits: &[(u32, u32, u32)]) -> Shift {
        Shift::new(edits.iter().map(|&(a, b, n)| (a..b, n)).collect())
    }

    #[test]
    fn a_position_before_an_edit_stays_and_one_after_moves_by_its_change() {
        let s = shift(&[(10, 16, 3)]);
        assert_eq!(s.map(0), 0);
        assert_eq!(s.map(10), 10);
        assert_eq!(s.map(16), 13);
        assert_eq!(s.map(40), 37);
        let s = shift(&[(10, 16, 13)]);
        assert_eq!(s.map(10), 10);
        assert_eq!(s.map(16), 23);
        assert_eq!(s.map(40), 47);
    }

    #[test]
    fn a_position_inside_a_removed_run_keeps_its_offset_up_to_the_new_end() {
        let s = shift(&[(10, 16, 3)]);
        assert_eq!(s.map(11), 11);
        assert_eq!(s.map(13), 13);
        assert_eq!(s.map(14), 13);
        assert_eq!(s.map(15), 13);
        assert_eq!(shift(&[(10, 16, 13)]).map(15), 15);
        let s = shift(&[(10, 16, 0)]);
        assert_eq!(s.map(12), 10);
        assert_eq!(s.map(16), 10);
    }

    #[test]
    fn changes_add_up_in_order_whatever_order_the_edits_come_in() {
        let s = shift(&[(50, 57, 13), (10, 16, 3), (30, 35, 3)]);
        assert_eq!(s.map(20), 17);
        assert_eq!(s.map(35), 30);
        assert_eq!(s.map(57), 58);
        assert_eq!(s.map(100), 101);
        assert_eq!(s.change(), 1);
        let mapped: Vec<u32> = (0..120).map(|p| s.map(p)).collect();
        assert!(mapped.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn a_story_counts_only_its_own_edits_from_its_first_character() {
        // Main 0..100, footnotes 100..140, headers 140..200.
        let s = shift(&[(20, 25, 3), (110, 116, 13), (150, 155, 3)]);
        let footnotes = s.within(100..140);
        assert_eq!(footnotes.map(0), 0);
        assert_eq!(footnotes.map(16), 23);
        assert_eq!(footnotes.map(39), 46);
        assert_eq!(footnotes.map(42), 49);
        assert_eq!(footnotes.change(), 7);
        let headers = s.within(140..200);
        assert_eq!(headers.map(15), 13);
        assert_eq!(headers.change(), -2);
        assert_eq!(s.map(140), 145);
        assert_eq!(s.map(200), 203);
    }

    #[test]
    fn a_backward_piece_maps_nothing_and_the_next_ones_overwrite_what_came_before() {
        let pcd = |cp, cp_end, fc| Pcd {
            cp,
            cp_end,
            flags: 0,
            fc,
            compressed: true,
            prm: 0,
        };
        let laid = laid_out(vec![
            pcd(0, 10, 100),
            pcd(10, 40, 200),
            pcd(40, 15, 300),
            pcd(15, 20, 400),
            pcd(20, 25, 500),
        ]);
        let got: Vec<(u32, u32, u32)> = laid.iter().map(|p| (p.cp, p.cp_end, p.fc)).collect();
        assert_eq!(
            got,
            [
                (0, 10, 100),
                (10, 15, 200),
                (15, 20, 400),
                (20, 25, 500),
                (25, 40, 215)
            ]
        );
    }

    #[test]
    fn a_plc_moves_its_positions_and_leaves_its_data() {
        let mut plc = Vec::new();
        for p in [0u32, 12, 30, 0xFFFF_FFFF] {
            plc.extend_from_slice(&p.to_le_bytes());
        }
        plc.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        let mut out = plc.clone();
        shift_plc(&mut out, &plc, 0..plc.len(), 2, &shift(&[(5, 10, 1)]));
        let positions: Vec<u32> = (0..4).map(|i| u32_at(&out, 4 * i).unwrap()).collect();
        assert_eq!(positions, [0, 8, 26, 0xFFFF_FFFF]);
        assert_eq!(out[16..], plc[16..]);
    }
}
