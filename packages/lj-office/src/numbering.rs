//! List labels as LibreOffice counts and writes them, whatever file the list
//! comes from: a counter per level, the levels below a paragraph reset by it,
//! and placeholders each showing their level's value in that level's format.

use std::collections::HashMap;
use std::hash::Hash;

/// Levels a list has.
pub(crate) const LEVELS: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NumFormat {
    /// Arabic digits, zero-padded to `width`.
    Decimal {
        width: u8,
    },
    UpperRoman,
    LowerRoman,
    /// A to Z, then AA to ZZ, and so on.
    UpperLetter,
    LowerLetter,
    /// Circled numbers up to twenty, then arabic.
    Circled,
    FullWidth,
    /// Asterisk, dagger, double dagger, section, each doubled on the next round.
    Chicago,
    /// No number: the level text alone.
    None,
}

impl NumFormat {
    pub const DECIMAL: NumFormat = NumFormat::Decimal { width: 1 };
}

fn roman(mut value: u32, upper: bool) -> String {
    const DIGITS: [(u32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    if value == 0 {
        return "0".into();
    }
    let mut out = String::new();
    for (step, digits) in DIGITS {
        while value >= step {
            out.push_str(digits);
            value -= step;
        }
    }
    if upper {
        out.make_ascii_uppercase();
    }
    out
}

fn letters(value: u32, first: u8) -> String {
    if value == 0 {
        return String::new();
    }
    let letter = char::from(first + ((value - 1) % 26) as u8);
    std::iter::repeat_n(letter, ((value - 1) / 26 + 1) as usize).collect()
}

/// Largest value a letter, roman or Chicago label writes: they grow with the
/// value, and LibreOffice keeps a list start in 16 bits.
const MAX_REPEATED: u32 = u16::MAX as u32;

/// `value` written in `format`.
pub(crate) fn number(value: u32, format: NumFormat) -> String {
    let value = match format {
        NumFormat::Decimal { .. } | NumFormat::Circled | NumFormat::FullWidth => value,
        _ => value.min(MAX_REPEATED),
    };
    match format {
        NumFormat::Decimal { width } => format!("{value:0width$}", width = usize::from(width)),
        NumFormat::UpperRoman => roman(value, true),
        NumFormat::LowerRoman => roman(value, false),
        NumFormat::UpperLetter => letters(value, b'A'),
        NumFormat::LowerLetter => letters(value, b'a'),
        NumFormat::Circled => match value {
            1..=20 => char::from_u32(0x2460 + value - 1).map_or_else(String::new, String::from),
            _ => value.to_string(),
        },
        NumFormat::FullWidth => value
            .to_string()
            .chars()
            .filter_map(|d| char::from_u32(0xFF10 + d.to_digit(10)?))
            .collect(),
        NumFormat::Chicago => {
            if value == 0 {
                return String::new();
            }
            let mark = ['*', '\u{2020}', '\u{2021}', '\u{A7}'][((value - 1) % 4) as usize];
            std::iter::repeat_n(mark, ((value - 1) / 4 + 1) as usize).collect()
        }
        NumFormat::None => String::new(),
    }
}

/// A level text: literal text, and placeholders naming a level from 0.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Part {
    Text(String),
    Level(usize),
}

/// Parse a level text where `%1` to `%9` name the levels.
pub(crate) fn parts(text: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut literal = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let level = chars
            .peek()
            .and_then(|d| d.to_digit(10))
            .filter(|d| (1..=LEVELS as u32).contains(d));
        match (c, level) {
            ('%', Some(d)) => {
                chars.next();
                if !literal.is_empty() {
                    parts.push(Part::Text(std::mem::take(&mut literal)));
                }
                parts.push(Part::Level(d as usize - 1));
            }
            _ => literal.push(c),
        }
    }
    if !literal.is_empty() {
        parts.push(Part::Text(literal));
    }
    parts
}

/// What a level writes, beside the paragraph formatting it brings.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Level {
    pub start: u32,
    pub format: NumFormat,
    pub text: Vec<Part>,
    /// Every placeholder is written in arabic digits.
    pub legal: bool,
}

impl Default for Level {
    fn default() -> Self {
        Self {
            start: 0,
            format: NumFormat::DECIMAL,
            text: Vec::new(),
            legal: false,
        }
    }
}

/// The label of a paragraph at `level`, `values` holding the counter of each
/// level up to it and `format_of` the format of each.
pub(crate) fn label(
    level: &Level,
    values: &[u32; LEVELS],
    format_of: impl Fn(usize) -> NumFormat,
) -> String {
    let mut out = String::new();
    for part in &level.text {
        match part {
            Part::Text(text) => out.push_str(text),
            Part::Level(k) => {
                let format = match format_of(*k) {
                    NumFormat::None => NumFormat::None,
                    _ if level.legal => NumFormat::DECIMAL,
                    format => format,
                };
                out.push_str(&number(values[*k], format));
            }
        }
    }
    out
}

/// The counters of every list of a story, keyed by the list they count.
pub(crate) struct Counters<K> {
    lists: HashMap<K, [Option<u32>; LEVELS]>,
}

impl<K> Default for Counters<K> {
    fn default() -> Self {
        Self {
            lists: HashMap::new(),
        }
    }
}

impl<K: Hash + Eq> Counters<K> {
    /// Count a paragraph at `level` of `list`, set to `restart` when it
    /// restarts the list. A level above not counted since its reset counts
    /// once, as LibreOffice numbers a list entered below its first level.
    pub fn count(
        &mut self,
        list: K,
        level: usize,
        starts: [u32; LEVELS],
        restart: Option<u32>,
    ) -> [u32; LEVELS] {
        let level = level.min(LEVELS - 1);
        let counters = self.lists.entry(list).or_insert([None; LEVELS]);
        for k in 0..level {
            counters[k].get_or_insert(starts[k]);
        }
        counters[level] = Some(match (restart, counters[level]) {
            (Some(value), _) => value,
            (None, Some(value)) => value.saturating_add(1),
            (None, None) => starts[level],
        });
        for counter in &mut counters[level + 1..] {
            *counter = None;
        }
        std::array::from_fn(|k| counters[k].unwrap_or(starts[k]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_written_as_libreoffice_writes_them() {
        assert_eq!(number(1994, NumFormat::UpperRoman), "MCMXCIV");
        assert_eq!(number(4, NumFormat::LowerRoman), "iv");
        assert_eq!(number(28, NumFormat::LowerLetter), "bb");
        assert_eq!(number(26, NumFormat::UpperLetter), "Z");
        assert_eq!(number(7, NumFormat::Decimal { width: 2 }), "07");
        assert_eq!(number(3, NumFormat::Circled), "\u{2462}");
        assert_eq!(number(6, NumFormat::Chicago), "\u{2020}\u{2020}");
    }

    /// The sequence LibreOffice renders for a list entered at its second
    /// level, whose third level names no start, then restarted at seven.
    #[test]
    fn a_list_counts_as_libreoffice_renders_it() {
        let levels = [
            Level {
                start: 1,
                text: parts("%1."),
                ..Level::default()
            },
            Level {
                start: 1,
                format: NumFormat::LowerLetter,
                text: parts("%1.%2)"),
                ..Level::default()
            },
            Level {
                start: 0,
                format: NumFormat::UpperLetter,
                text: parts("%1.%2.%3"),
                legal: true,
            },
        ];
        let starts = std::array::from_fn(|k| levels.get(k).map_or(0, |l| l.start));
        let mut counters = Counters::default();
        let mut run = |level: usize, restart: Option<u32>| {
            let values = counters.count(0, level, starts, restart);
            label(&levels[level], &values, |k| levels[k].format)
        };
        let labels: Vec<String> = [(1, None), (0, None), (1, None), (2, None), (2, None)]
            .into_iter()
            .chain([(0, Some(7)), (0, None), (1, None)])
            .map(|(level, restart)| run(level, restart))
            .collect();
        assert_eq!(
            labels,
            ["1.a)", "2.", "2.a)", "2.1.0", "2.1.1", "7.", "8.", "8.a)"]
        );
    }
}
