//! Baseline JPEG as quantised coefficients, read from its Huffman stream and
//! written back, so that blocks can be replaced while every other one keeps
//! its exact bits.

use crate::MAX_PIXELS;

pub struct Component {
    pub id: u8,
    pub h: usize,
    pub v: usize,
    pub quant: usize,
    /// Blocks per row and rows of blocks, padded to whole MCUs.
    pub blocks_wide: usize,
    pub blocks_high: usize,
    /// Each block's 64 coefficients in zigzag order.
    pub blocks: Vec<[i16; 64]>,
    /// Blocks a scan of this component alone covers.
    own: (usize, usize),
}

pub struct Jpeg {
    pub width: usize,
    pub height: usize,
    pub components: Vec<Component>,
    /// The quantisation tables, zigzag order.
    pub quant: [[u16; 64]; 4],
    /// The transform byte of an Adobe APP14 segment.
    pub adobe: Option<u8>,
    pub jfif: bool,
    /// APPn and COM segments as read, markers included.
    segments: Vec<u8>,
}

/// Canonical Huffman decoding, ITU T.81 F.2.2.3.
#[derive(Clone)]
struct Huffman {
    max_code: [i32; 17],
    offset: [i32; 17],
    values: Vec<u8>,
}

impl Huffman {
    fn new(counts: &[u8], values: &[u8]) -> Option<Huffman> {
        let mut table = Huffman {
            max_code: [-1; 17],
            offset: [0; 17],
            values: values.to_vec(),
        };
        let (mut code, mut k) = (0i32, 0i32);
        for length in 1..=16 {
            let count = i32::from(counts[length - 1]);
            if code + count > 1 << length {
                return None;
            }
            table.offset[length] = k - code;
            code += count;
            k += count;
            if count > 0 {
                table.max_code[length] = code - 1;
            }
            code <<= 1;
        }
        Some(table)
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    bits: u32,
    marker: bool,
    /// Zero bytes fed past the end of the scan.
    padding: usize,
}

impl Reader<'_> {
    fn fill(&mut self) {
        while self.bits <= 56 {
            let mut byte = 0;
            if self.marker || self.pos >= self.data.len() {
                self.padding += 1;
            } else if self.data[self.pos] != 0xFF {
                byte = self.data[self.pos];
                self.pos += 1;
            } else if self.data.get(self.pos + 1) == Some(&0) {
                byte = 0xFF;
                self.pos += 2;
            } else {
                self.marker = true;
                self.padding += 1;
            }
            self.acc |= u64::from(byte) << (56 - self.bits);
            self.bits += 8;
        }
    }

    fn get(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        self.fill();
        let value = (self.acc >> (64 - n)) as u32;
        self.acc <<= n;
        self.bits -= n;
        value
    }

    fn decode(&mut self, table: &Huffman) -> Option<u8> {
        self.fill();
        for length in 1..=16 {
            let code = (self.acc >> (64 - length)) as i32;
            if code <= table.max_code[length] {
                self.acc <<= length;
                self.bits -= length as u32;
                return table
                    .values
                    .get((code + table.offset[length]) as usize)
                    .copied();
            }
        }
        None
    }

    fn extend(&mut self, size: u8) -> Option<i32> {
        if size > 16 {
            return None;
        }
        let size = u32::from(size);
        let value = self.get(size) as i32;
        Some(match size > 0 && value < 1 << (size - 1) {
            true => value - (1 << size) + 1,
            false => value,
        })
    }

    /// Drop the bits left and step past the restart marker, which must follow.
    fn restart(&mut self) -> Option<()> {
        (self.acc, self.bits, self.marker, self.padding) = (0, 0, false, 0);
        while self.data.get(self.pos) == Some(&0xFF) && self.data.get(self.pos + 1) == Some(&0xFF) {
            self.pos += 1;
        }
        let marker = *self.data.get(self.pos + 1)?;
        (self.data[self.pos] == 0xFF && (0xD0..=0xD7).contains(&marker)).then(|| self.pos += 2)
    }
}

/// The coefficients of a baseline, extended or progressive Huffman JPEG of
/// eight bit samples; `None` for anything else, or a stream that does not
/// decode to the end.
pub fn read(bytes: &[u8]) -> Option<Jpeg> {
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut jpeg: Option<Jpeg> = None;
    let (mut quant, mut adobe, mut jfif) = ([[0u16; 64]; 4], None, false);
    let mut segments = Vec::new();
    let mut tables = vec![None; 8];
    let (mut restart, mut scans) = (0, 0);
    let mut pos = 2;
    loop {
        while pos < bytes.len() && bytes[pos] != 0xFF {
            pos += 1;
        }
        while pos < bytes.len() && bytes[pos] == 0xFF {
            pos += 1;
        }
        let Some(&marker) = bytes.get(pos) else {
            break;
        };
        pos += 1;
        match marker {
            0xD9 => break,
            0xD0..=0xD7 | 0x01 => continue,
            _ => {}
        }
        let length = usize::from(u16::from_be_bytes([*bytes.get(pos)?, *bytes.get(pos + 1)?]));
        let segment = bytes.get(pos + 2..pos + length)?;
        let start = pos - 2;
        pos += length;
        match marker {
            0xC0..=0xC2 if jpeg.is_none() => jpeg = Some(frame(segment)?),
            0xC0..=0xCF if marker != 0xC4 && marker != 0xCC => return None,
            0xC4 => {
                let mut rest = segment;
                while !rest.is_empty() {
                    let slot = usize::from(rest[0] >> 4) * 4 + usize::from(rest[0] & 15);
                    let counts = rest.get(1..17)?;
                    let total = counts.iter().map(|&c| usize::from(c)).sum::<usize>();
                    *tables.get_mut(slot)? = Some(Huffman::new(counts, rest.get(17..17 + total)?)?);
                    rest = &rest[17 + total..];
                }
            }
            0xDB => {
                let mut rest = segment;
                while !rest.is_empty() {
                    let wide = rest[0] >> 4 != 0;
                    let size = if wide { 128 } else { 64 };
                    let values = rest.get(1..1 + size)?;
                    for (k, entry) in quant
                        .get_mut(usize::from(rest[0] & 15))?
                        .iter_mut()
                        .enumerate()
                    {
                        *entry = match wide {
                            true => u16::from_be_bytes([values[2 * k], values[2 * k + 1]]),
                            false => u16::from(values[k]),
                        };
                    }
                    rest = &rest[1 + size..];
                }
            }
            0xDD => {
                restart = usize::from(u16::from_be_bytes([*segment.first()?, *segment.get(1)?]))
            }
            0xDA => {
                scans += 1;
                if scans > 32 {
                    return None;
                }
                pos = decode_scan(jpeg.as_mut()?, segment, bytes, pos, restart, &tables)?;
            }
            0xE0..=0xEF | 0xFE => {
                jfif |= marker == 0xE0 && segment.starts_with(b"JFIF\0");
                if marker == 0xEE && segment.starts_with(b"Adobe") {
                    adobe = Some(*segment.get(11)?);
                }
                segments.extend_from_slice(&bytes[start..pos]);
            }
            _ => {}
        }
    }
    let mut jpeg = jpeg?;
    (jpeg.quant, jpeg.adobe, jpeg.jfif, jpeg.segments) = (quant, adobe, jfif, segments);
    let zero_step = jpeg
        .components
        .iter()
        .any(|c| jpeg.quant[c.quant].contains(&0));
    (scans > 0 && !zero_step).then_some(jpeg)
}

fn frame(segment: &[u8]) -> Option<Jpeg> {
    let header = segment.get(..6)?;
    let height = usize::from(u16::from_be_bytes([header[1], header[2]]));
    let width = usize::from(u16::from_be_bytes([header[3], header[4]]));
    let count = usize::from(header[5]);
    if header[0] != 8 || width == 0 || height == 0 || !(1..=4).contains(&count) {
        return None;
    }
    let mut specs = Vec::with_capacity(count);
    for spec in segment.get(6..6 + 3 * count)?.chunks(3) {
        let (h, v) = (usize::from(spec[1] >> 4), usize::from(spec[1] & 15));
        if !(1..=4).contains(&h) || !(1..=4).contains(&v) || spec[2] > 3 {
            return None;
        }
        specs.push((spec[0], h, v, usize::from(spec[2])));
    }
    let h_max = specs.iter().map(|c| c.1).max()?;
    let v_max = specs.iter().map(|c| c.2).max()?;
    let (mcus_wide, mcus_high) = (width.div_ceil(8 * h_max), height.div_ceil(8 * v_max));
    let blocks: usize = specs
        .iter()
        .map(|c| mcus_wide * c.1 * mcus_high * c.2)
        .sum();
    if (width * height) as u64 > MAX_PIXELS || blocks as u64 * 64 > 2 * MAX_PIXELS {
        return None;
    }
    let components = specs
        .into_iter()
        .map(|(id, h, v, quant)| Component {
            id,
            h,
            v,
            quant,
            blocks_wide: mcus_wide * h,
            blocks_high: mcus_high * v,
            blocks: vec![[0; 64]; mcus_wide * h * mcus_high * v],
            own: match count {
                1 => (width.div_ceil(8), height.div_ceil(8)),
                _ => (
                    (width * h).div_ceil(h_max).div_ceil(8),
                    (height * v).div_ceil(v_max).div_ceil(8),
                ),
            },
        })
        .collect();
    Some(Jpeg {
        width,
        height,
        components,
        quant: [[0; 64]; 4],
        adobe: None,
        jfif: false,
        segments: Vec::new(),
    })
}

/// Decode the scan whose header is `segment` from `pos`, returning where the
/// marker after it starts.
fn decode_scan(
    jpeg: &mut Jpeg,
    segment: &[u8],
    data: &[u8],
    pos: usize,
    restart: usize,
    tables: &[Option<Huffman>],
) -> Option<usize> {
    let count = usize::from(*segment.first()?);
    let &[start, end, approx] = segment.get(1 + 2 * count..4 + 2 * count)? else {
        return None;
    };
    let (start, end, high, low) = (
        usize::from(start),
        usize::from(end),
        approx >> 4,
        approx & 15,
    );
    // Spectral selection and successive approximation, ITU T.81 G.1.
    let valid = match start {
        0 => end == 0 || (end == 63 && approx == 0),
        _ => count == 1 && start <= end && end <= 63,
    };
    if !valid || high > 13 || low > 13 {
        return None;
    }
    let mut scan = Vec::with_capacity(count);
    for spec in segment.get(1..1 + 2 * count)?.chunks(2) {
        let c = jpeg.components.iter().position(|c| c.id == spec[0])?;
        let dc = tables[usize::from(spec[1] >> 4) & 3].as_ref();
        let ac = tables[4 + (usize::from(spec[1] & 15) & 3)].as_ref();
        scan.push((c, dc, ac));
    }
    let (plus, minus) = (1i16 << low, -1i16 << low);
    let components: Vec<usize> = scan.iter().map(|s| s.0).collect();
    let mut reader = Reader {
        data,
        pos,
        acc: 0,
        bits: 0,
        marker: false,
        padding: 0,
    };
    let mut predictors = [0i32; 4];
    let mut eob_run = 0u32;
    let mut blocks = Vec::with_capacity(10);
    for unit in 0..units(jpeg, &components) {
        // The look-ahead reads 8 bytes past the end; more is a scan cut short.
        if reader.padding > 16 {
            return None;
        }
        if restart > 0 && unit > 0 && unit % restart == 0 {
            reader.restart()?;
            (predictors, eob_run) = ([0; 4], 0);
        }
        unit_blocks(jpeg, &components, unit, &mut blocks);
        for &(i, block) in &blocks {
            let (c, dc, ac) = scan[i];
            let coefficients = &mut jpeg.components[c].blocks[block];
            if start == 0 && high > 0 {
                if reader.get(1) == 1 {
                    coefficients[0] |= plus;
                }
            } else if start == 0 {
                let size = reader.decode(dc?)?;
                predictors[c] += reader.extend(size)?;
                coefficients[0] = i16::try_from(predictors[c] << low).ok()?;
            }
            if end == 0 {
                continue;
            }
            let ac = ac?;
            let mut k = start.max(1);
            if high > 0 {
                refine(
                    &mut reader,
                    ac,
                    coefficients,
                    k,
                    end,
                    &mut eob_run,
                    (plus, minus),
                )?;
                continue;
            }
            if eob_run > 0 {
                eob_run -= 1;
                continue;
            }
            while k <= end {
                let symbol = reader.decode(ac)?;
                let (run, size) = (usize::from(symbol >> 4), symbol & 15);
                if size == 0 && run != 15 {
                    eob_run = (1 << run) - 1 + reader.get(run as u32);
                    break;
                }
                k += run;
                if size > 0 {
                    *coefficients.get_mut(k)? = i16::try_from(reader.extend(size)? << low).ok()?;
                }
                k += 1;
            }
        }
    }
    let mut pos = reader.pos;
    while pos + 1 < data.len()
        && !(data[pos] == 0xFF && !matches!(data[pos + 1], 0 | 0xFF | 0xD0..=0xD7))
    {
        pos += 1;
    }
    Some(pos)
}

/// A successive approximation refinement of coefficients `k..=end`, as
/// libjpeg's `decode_mcu_AC_refine` reads it.
fn refine(
    reader: &mut Reader,
    ac: &Huffman,
    coefficients: &mut [i16; 64],
    mut k: usize,
    end: usize,
    eob_run: &mut u32,
    (plus, minus): (i16, i16),
) -> Option<()> {
    let correct = |reader: &mut Reader, coefficient: &mut i16| {
        if reader.get(1) == 1 && *coefficient & plus == 0 {
            *coefficient += if *coefficient >= 0 { plus } else { minus };
        }
    };
    if *eob_run == 0 {
        while k <= end {
            let symbol = reader.decode(ac)?;
            let (mut run, size) = (i32::from(symbol >> 4), symbol & 15);
            let mut value = 0;
            if size != 0 {
                value = if reader.get(1) == 1 { plus } else { minus };
            } else if run != 15 {
                *eob_run = (1 << run) + reader.get(run as u32);
                break;
            }
            while k <= end {
                if coefficients[k] != 0 {
                    correct(reader, &mut coefficients[k]);
                } else {
                    run -= 1;
                    if run < 0 {
                        break;
                    }
                }
                k += 1;
            }
            if value != 0 {
                *coefficients.get_mut(k)? = value;
            }
            k += 1;
        }
    }
    if *eob_run > 0 {
        for coefficient in &mut coefficients[k.min(end + 1)..=end] {
            if *coefficient != 0 {
                correct(reader, coefficient);
            }
        }
        *eob_run -= 1;
    }
    Some(())
}

/// How many coding units a scan over `components` has: MCUs when it
/// interleaves, single blocks otherwise.
fn units(jpeg: &Jpeg, components: &[usize]) -> usize {
    let first = &jpeg.components[components[0]];
    match components.len() {
        1 => first.own.0 * first.own.1,
        _ => first.blocks_wide / first.h * (first.blocks_high / first.v),
    }
}

/// (index in the scan, block index) of each block unit `unit` carries.
fn unit_blocks(jpeg: &Jpeg, components: &[usize], unit: usize, out: &mut Vec<(usize, usize)>) {
    out.clear();
    let first = &jpeg.components[components[0]];
    if components.len() == 1 {
        let (x, y) = (unit % first.own.0, unit / first.own.0);
        out.push((0, y * first.blocks_wide + x));
        return;
    }
    let mcus_wide = first.blocks_wide / first.h;
    let (x, y) = (unit % mcus_wide, unit / mcus_wide);
    for (i, &c) in components.iter().enumerate() {
        let component = &jpeg.components[c];
        for v in 0..component.v {
            for h in 0..component.h {
                let row = y * component.v + v;
                out.push((i, row * component.blocks_wide + x * component.h + h));
            }
        }
    }
}

impl Jpeg {
    /// Replace by a flat block of `samples`, one stored sample per
    /// component, every block that meets the pixel rectangle `(x, y, w, h)`,
    /// rounded out to whole MCUs when components interleave. A subsampled
    /// component is flattened one of its samples wider, so that a decoder
    /// smoothing it across blocks does not tint the rectangle.
    pub fn flatten(&mut self, rect: (usize, usize, usize, usize), samples: &[u8]) {
        let (mut x, mut y, w, h) = rect;
        let (mut right, mut bottom) = (x + w, y + h);
        let h_max = self.components.iter().map(|c| c.h).max().unwrap_or(1);
        let v_max = self.components.iter().map(|c| c.v).max().unwrap_or(1);
        let single = self.components.len() == 1;
        if self.components.iter().any(|c| c.h != h_max || c.v != v_max) {
            (x, y) = (x.saturating_sub(h_max), y.saturating_sub(v_max));
            (right, bottom) = (right + h_max, bottom + v_max);
        }
        let (unit_w, unit_h) = if single {
            (8, 8)
        } else {
            (8 * h_max, 8 * v_max)
        };
        let columns = (x / unit_w, right.min(self.width).div_ceil(unit_w));
        let rows = (y / unit_h, bottom.min(self.height).div_ceil(unit_h));
        for (component, &sample) in self.components.iter_mut().zip(samples) {
            // The DC a flat block of `sample` takes; black and white round
            // past the end so that the decoder clamps them exactly.
            let q = i32::from(self.quant[component.quant][0]);
            let mut flat = [0i16; 64];
            flat[0] = match sample {
                0 => -(1024 + q - 1) / q,
                255 => (1016 + q - 1) / q,
                s => ((i32::from(s) - 128) * 8 + q / 2).div_euclid(q),
            } as i16;
            let (scale_w, scale_h) = if single {
                (1, 1)
            } else {
                (component.h, component.v)
            };
            for by in rows.0 * scale_h..(rows.1 * scale_h).min(component.blocks_high) {
                for bx in columns.0 * scale_w..(columns.1 * scale_w).min(component.blocks_wide) {
                    component.blocks[by * component.blocks_wide + bx] = flat;
                }
            }
        }
    }

    /// The coefficients as a baseline JPEG under Huffman tables built for
    /// them, luma and chroma apart; `None` when a coefficient passes what
    /// baseline codes.
    pub fn write(&self) -> Option<Vec<u8>> {
        let n = self.components.len();
        let scans: Vec<Vec<usize>> =
            match n > 1 && self.components.iter().map(|c| c.h * c.v).sum::<usize>() <= 10 {
                true => vec![(0..n).collect()],
                false => (0..n).map(|c| vec![c]).collect(),
            };
        let mut frequencies = vec![[0u32; 257]; 4];
        for scan in &scans {
            self.entropy(scan, &mut |table, symbol, _, _| {
                frequencies[table][usize::from(symbol)] += 1
            })?;
        }
        let codes: Vec<Option<Code>> = frequencies
            .iter()
            .map(|f| f.iter().any(|&n| n > 0).then(|| Code::optimal(f)))
            .collect();

        let mut out = vec![0xFF, 0xD8];
        out.extend_from_slice(&self.segments);
        let mut used: Vec<usize> = self.components.iter().map(|c| c.quant).collect();
        used.sort_unstable();
        used.dedup();
        let wide = used.iter().any(|&q| self.quant[q].iter().any(|&v| v > 255));
        for &q in &used {
            let mut segment = vec![(u8::from(wide) << 4) | q as u8];
            for &value in &self.quant[q] {
                match wide {
                    true => segment.extend_from_slice(&value.to_be_bytes()),
                    false => segment.push(value as u8),
                }
            }
            put_segment(&mut out, 0xDB, &segment);
        }
        let mut sof = vec![8];
        sof.extend_from_slice(&(self.height as u16).to_be_bytes());
        sof.extend_from_slice(&(self.width as u16).to_be_bytes());
        sof.push(n as u8);
        for c in &self.components {
            sof.extend_from_slice(&[c.id, ((c.h << 4) | c.v) as u8, c.quant as u8]);
        }
        put_segment(&mut out, if wide { 0xC1 } else { 0xC0 }, &sof);
        for (slot, code) in codes.iter().enumerate() {
            if let Some(code) = code {
                let mut segment = vec![(((slot % 2) << 4) | (slot / 2)) as u8];
                segment.extend_from_slice(&code.counts);
                segment.extend_from_slice(&code.values);
                put_segment(&mut out, 0xC4, &segment);
            }
        }
        for scan in &scans {
            let mut sos = vec![scan.len() as u8];
            for &c in scan {
                let group = u8::from(c > 0);
                sos.extend_from_slice(&[self.components[c].id, (group << 4) | group]);
            }
            sos.extend_from_slice(&[0, 63, 0]);
            put_segment(&mut out, 0xDA, &sos);
            let mut writer = Writer {
                out: &mut out,
                acc: 0,
                bits: 0,
            };
            self.entropy(scan, &mut |table, symbol, value, size| {
                let (code, length) =
                    codes[table].as_ref().expect("counted").codes[usize::from(symbol)];
                writer.put(u32::from(code), u32::from(length));
                let bits = if value < 0 { value - 1 } else { value };
                writer.put(bits as u32 & ((1 << size) - 1), u32::from(size));
            })?;
            let pad = (8 - writer.bits % 8) % 8;
            writer.put((1 << pad) - 1, pad);
        }
        out.extend_from_slice(&[0xFF, 0xD9]);
        Some(out)
    }

    /// Hand each symbol of `scan` to `emit` as (table, symbol, value, size):
    /// tables 0 and 1 code DC and AC of the first component, 2 and 3 of the
    /// others.
    fn entropy(&self, scan: &[usize], emit: &mut impl FnMut(usize, u8, i32, u8)) -> Option<()> {
        let mut predictors = [0i32; 4];
        let mut blocks = Vec::with_capacity(10);
        for unit in 0..units(self, scan) {
            unit_blocks(self, scan, unit, &mut blocks);
            for &(i, block) in &blocks {
                let c = scan[i];
                let table = 2 * usize::from(c > 0);
                let block = &self.components[c].blocks[block];
                let diff = i32::from(block[0]) - predictors[c];
                predictors[c] = i32::from(block[0]);
                let size = magnitude(diff);
                if size > 11 {
                    return None;
                }
                emit(table, size, diff, size);
                let mut run = 0;
                for &value in &block[1..] {
                    if value == 0 {
                        run += 1;
                        continue;
                    }
                    while run > 15 {
                        emit(table + 1, 0xF0, 0, 0);
                        run -= 16;
                    }
                    let size = magnitude(i32::from(value));
                    if size > 10 {
                        return None;
                    }
                    emit(table + 1, (run << 4) | size, i32::from(value), size);
                    run = 0;
                }
                if run > 0 {
                    emit(table + 1, 0, 0, 0);
                }
            }
        }
        Some(())
    }
}

fn magnitude(value: i32) -> u8 {
    (32 - value.unsigned_abs().leading_zeros()) as u8
}

fn put_segment(out: &mut Vec<u8>, marker: u8, segment: &[u8]) {
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&(segment.len() as u16 + 2).to_be_bytes());
    out.extend_from_slice(segment);
}

struct Writer<'a> {
    out: &'a mut Vec<u8>,
    acc: u32,
    bits: u32,
}

impl Writer<'_> {
    fn put(&mut self, code: u32, length: u32) {
        self.acc = (self.acc << length) | code;
        self.bits += length;
        while self.bits >= 8 {
            self.bits -= 8;
            let byte = (self.acc >> self.bits) as u8;
            self.out.push(byte);
            if byte == 0xFF {
                self.out.push(0);
            }
        }
        self.acc &= (1 << self.bits) - 1;
    }
}

struct Code {
    counts: [u8; 16],
    values: Vec<u8>,
    codes: [(u16, u8); 256],
}

impl Code {
    /// The length-limited optimal code of ITU T.81 K.2, one code point kept
    /// back so that no code is all ones.
    fn optimal(frequencies: &[u32; 257]) -> Code {
        let mut freq: Vec<u64> = frequencies.iter().map(|&f| u64::from(f)).collect();
        freq[256] = 1;
        let (mut size, mut others) = ([0usize; 257], [usize::MAX; 257]);
        let smallest = |skip: usize, freq: &[u64]| {
            (0..257)
                .filter(|&i| freq[i] > 0 && i != skip)
                .reduce(|b, i| if freq[i] <= freq[b] { i } else { b })
        };
        while let Some(mut c1) = smallest(usize::MAX, &freq) {
            let Some(mut c2) = smallest(c1, &freq) else {
                break;
            };
            freq[c1] += freq[c2];
            freq[c2] = 0;
            size[c1] += 1;
            while others[c1] != usize::MAX {
                c1 = others[c1];
                size[c1] += 1;
            }
            others[c1] = c2;
            size[c2] += 1;
            while others[c2] != usize::MAX {
                c2 = others[c2];
                size[c2] += 1;
            }
        }
        let mut bits = [0u32; 33];
        for &s in size.iter().filter(|&&s| s > 0) {
            bits[s.min(32)] += 1;
        }
        for i in (17..=32).rev() {
            while bits[i] > 0 {
                let mut j = i - 2;
                while bits[j] == 0 {
                    j -= 1;
                }
                bits[i] -= 2;
                bits[i - 1] += 1;
                bits[j + 1] += 2;
                bits[j] -= 1;
            }
        }
        let last = (1..=16)
            .rev()
            .find(|&i| bits[i] > 0)
            .expect("a code point at least");
        bits[last] -= 1;
        let mut values = Vec::new();
        for length in 1..=32 {
            values.extend((0..256).filter(|&s| size[s] == length).map(|s| s as u8));
        }
        let counts: [u8; 16] = std::array::from_fn(|i| bits[i + 1] as u8);
        let mut codes = [(0u16, 0u8); 256];
        let (mut code, mut k) = (0u16, 0);
        for (i, &count) in counts.iter().enumerate() {
            for _ in 0..count {
                codes[usize::from(values[k])] = (code, i as u8 + 1);
                code += 1;
                k += 1;
            }
            code <<= 1;
        }
        Code {
            counts,
            values,
            codes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jpeg_encoder::{ColorType, Encoder, SamplingFactor};

    const WIDTH: usize = 61;
    const HEIGHT: usize = 45;

    /// An RGB JPEG subsampled 4:2:0 with a restart marker every two MCUs.
    fn baseline_420_with_restarts() -> Vec<u8> {
        let mut rgb = Vec::with_capacity(WIDTH * HEIGHT * 3);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let wave = ((x * 7 + y * 13) % 29) as u8 * 8;
                rgb.extend_from_slice(&[wave, (x * 4) as u8, (255 - y * 5) as u8]);
            }
        }
        let mut out = Vec::new();
        let mut encoder = Encoder::new(&mut out, 80);
        encoder.set_sampling_factor(SamplingFactor::R_4_2_0);
        encoder.set_restart_interval(2);
        encoder
            .encode(&rgb, WIDTH as u16, HEIGHT as u16, ColorType::Rgb)
            .expect("encodes");
        assert!(out.windows(2).any(|w| w == [0xFF, 0xD1]));
        out
    }

    fn pixels(bytes: &[u8]) -> Vec<u8> {
        jpeg_decoder::Decoder::new(bytes).decode().expect("decodes")
    }

    #[test]
    fn written_back_the_coefficients_decode_to_the_same_pixels() {
        let original = baseline_420_with_restarts();
        let written = read(&original).expect("reads").write().expect("writes");
        assert_eq!(pixels(&original), pixels(&written));
    }

    #[test]
    fn a_flattened_rectangle_is_black_and_every_other_mcu_keeps_its_bits() {
        let original = baseline_420_with_restarts();
        let before = read(&original).unwrap();
        let mut after = read(&original).unwrap();
        after.flatten((20, 10, 5, 5), &[0, 128, 128]);
        let written = after.write().unwrap();
        let reread = read(&written).unwrap();
        // Grown by one chroma sample, (18, 8) to (27, 17) meets MCU column 1
        // and rows 0 and 1, of 16 pixels each.
        for (a, b) in before.components.iter().zip(&reread.components) {
            for (index, (block_a, block_b)) in a.blocks.iter().zip(&b.blocks).enumerate() {
                let (mx, my) = (index % a.blocks_wide / a.h, index / a.blocks_wide / a.v);
                match mx == 1 && my <= 1 {
                    true => assert!(block_b[1..].iter().all(|&c| c == 0)),
                    false => assert_eq!(block_a, block_b, "block {index}"),
                }
            }
        }
        let decoded = pixels(&written);
        for y in 10..15 {
            for x in 20..25 {
                let at = (y * WIDTH + x) * 3;
                assert_eq!(decoded[at..at + 3], [0, 0, 0], "({x}, {y})");
            }
        }
    }
}
