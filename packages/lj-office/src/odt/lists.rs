//! List styles, their levels, and the labels their counters produce.

use std::sync::Arc;

use super::model::*;
use super::styles::{apply_text, Scope, Styles};
use super::units;
use super::xml::{Element, Ns, Xml};

#[derive(Clone, Debug)]
pub enum Kind {
    Number {
        format: NumFormat,
        display_levels: u8,
    },
    Bullet(String),
    Image(String),
}

#[derive(Clone, Debug)]
pub struct Level {
    pub kind: Kind,
    pub prefix: String,
    pub suffix: String,
    /// `loext:num-list-format`, `%1%.%2%.`: the label with each level's number
    /// in place, which LibreOffice prefers to prefix, suffix and display levels.
    pub template: Option<String>,
    pub start: u32,
    pub letter_sync: bool,
    pub char_style: Option<String>,
    /// Properties of the label itself, a bullet's font among them.
    pub text: Option<usize>,
    pub bullet_scale: Option<f32>,
    pub placement: LabelPlacement,
    /// `label-alignment` indents, which a list paragraph takes over its own.
    pub margin_left: Option<Pt>,
    pub text_indent: Option<Pt>,
}

pub struct ListStyle<'a> {
    pub levels: Vec<Level>,
    /// Every level counts in one sequence.
    pub consecutive: bool,
    pub text_props: Vec<&'a Element>,
}

pub fn num_format(value: Option<&str>) -> NumFormat {
    let Some(v) = value else {
        return NumFormat::Decimal;
    };
    let first = v.split(',').next().unwrap_or_default().trim();
    match first {
        "" => NumFormat::None,
        "a" => NumFormat::LowerAlpha,
        "A" => NumFormat::UpperAlpha,
        "i" => NumFormat::LowerRoman,
        "I" => NumFormat::UpperRoman,
        "\u{4e00}" => NumFormat::Chinese,
        "\u{7532}" => NumFormat::Stems,
        "\u{2460}" => NumFormat::Circled,
        "\u{391}" => NumFormat::Greek { upper: true },
        "\u{3b1}" => NumFormat::Greek { upper: false },
        "\u{410}" => NumFormat::Cyrillic { upper: true },
        "\u{430}" => NumFormat::Cyrillic { upper: false },
        "\u{5d0}" => NumFormat::Hebrew,
        f if f.len() > 1
            && f.starts_with('0')
            && f.ends_with('1')
            && f.bytes().all(|b| b == b'0' || b == b'1') =>
        {
            NumFormat::Padded(f.len().min(9) as u8)
        }
        _ => NumFormat::Decimal,
    }
}

/// Whether a format's own name asks for synced letters, `аа, бб`.
pub fn format_syncs(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        let mut letters = v.split(',').filter_map(|s| {
            let s = s.trim();
            (s.chars().count() == 2 && s.chars().all(char::is_alphabetic)).then_some(s)
        });
        letters.nth(1).is_some_and(|s| {
            let mut c = s.chars().flat_map(char::to_lowercase);
            c.next() == c.next()
        })
    })
}

const GREEK: &str = "\u{3b1}\u{3b2}\u{3b3}\u{3b4}\u{3b5}\u{3b6}\u{3b7}\u{3b8}\u{3b9}\u{3ba}\u{3bb}\u{3bc}\u{3bd}\u{3be}\u{3bf}\u{3c0}\u{3c1}\u{3c3}\u{3c4}\u{3c5}\u{3c6}\u{3c7}\u{3c8}\u{3c9}";
const CYRILLIC: &str = "\u{430}\u{431}\u{432}\u{433}\u{434}\u{435}\u{436}\u{437}\u{438}\u{43a}\u{43b}\u{43c}\u{43d}\u{43e}\u{43f}\u{440}\u{441}\u{442}\u{443}\u{444}\u{445}\u{446}\u{447}\u{448}\u{449}\u{44d}\u{44e}\u{44f}";
const STEMS: &str =
    "\u{7532}\u{4e59}\u{4e19}\u{4e01}\u{620a}\u{5df1}\u{5e9a}\u{8f9b}\u{58ec}\u{7678}";
const CHINESE_DIGITS: [char; 10] = [
    '\u{96f6}', '\u{4e00}', '\u{4e8c}', '\u{4e09}', '\u{56db}', '\u{4e94}', '\u{516d}', '\u{4e03}',
    '\u{516b}', '\u{4e5d}',
];

/// `n` (1-based) in an alphabet: `a`..`z`, then `aa`, `ab`.. or, synced,
/// `aa`, `bb`...
fn letters(n: u32, alphabet: &[char], sync: bool) -> Vec<char> {
    if n == 0 {
        return Vec::new();
    }
    let size = alphabet.len() as u32;
    if sync {
        let letter = alphabet[((n - 1) % size) as usize];
        return std::iter::repeat_n(letter, ((n - 1) / size + 1).min(1000) as usize).collect();
    }
    let mut n = n;
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push(alphabet[(n % size) as usize]);
        n /= size;
    }
    out.reverse();
    out
}

fn chinese(n: u32) -> String {
    if n == 0 || n >= 10_000 {
        return n.to_string();
    }
    let digits = [n / 1000, n / 100 % 10, n / 10 % 10, n % 10];
    let units = ['\u{5343}', '\u{767e}', '\u{5341}'];
    let mut out = String::new();
    let mut zero = false;
    for (i, &d) in digits.iter().enumerate() {
        if d == 0 {
            zero = !out.is_empty();
            continue;
        }
        if zero {
            out.push(CHINESE_DIGITS[0]);
            zero = false;
        }
        if !(i == 2 && d == 1 && out.is_empty()) {
            out.push(CHINESE_DIGITS[d as usize]);
        }
        if i < 3 {
            out.push(units[i]);
        }
    }
    out
}

fn hebrew(n: u32) -> String {
    const ONES: [char; 9] = ['א', 'ב', 'ג', 'ד', 'ה', 'ו', 'ז', 'ח', 'ט'];
    const TENS: [char; 9] = ['י', 'כ', 'ל', 'מ', 'נ', 'ס', 'ע', 'פ', 'צ'];
    const HUNDREDS: [char; 4] = ['ק', 'ר', 'ש', 'ת'];
    let mut out = String::new();
    let mut hundreds = n / 100;
    while hundreds >= 4 {
        out.push('ת');
        hundreds -= 4;
    }
    if hundreds > 0 {
        out.push(HUNDREDS[hundreds as usize - 1]);
    }
    match n % 100 {
        15 => out.push_str("טו"),
        16 => out.push_str("טז"),
        rest => {
            if rest >= 10 {
                out.push(TENS[(rest / 10) as usize - 1]);
            }
            if rest % 10 > 0 {
                out.push(ONES[(rest % 10) as usize - 1]);
            }
        }
    }
    out
}

fn circled(n: u32) -> String {
    let c = match n {
        1..=20 => char::from_u32(0x2460 + n - 1),
        21..=35 => char::from_u32(0x3251 + n - 21),
        36..=50 => char::from_u32(0x32b1 + n - 36),
        _ => None,
    };
    c.map_or_else(|| n.to_string(), String::from)
}

fn roman(mut n: u32) -> String {
    const TABLE: [(u32, &str); 13] = [
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
    let mut out = String::new();
    for (value, digits) in TABLE {
        while n >= value {
            out.push_str(digits);
            n -= value;
        }
    }
    out
}

/// A counter as its level writes it.
pub fn format_number(n: u32, format: NumFormat, letter_sync: bool) -> String {
    let alphabet = |set: &str| set.chars().collect::<Vec<char>>();
    match format {
        NumFormat::Decimal => n.to_string(),
        NumFormat::Padded(width) => format!("{n:0width$}", width = width as usize),
        NumFormat::None => String::new(),
        NumFormat::LowerRoman | NumFormat::UpperRoman => {
            let r = if n == 0 {
                String::new()
            } else {
                roman(n.min(100_000))
            };
            if format == NumFormat::UpperRoman {
                r.to_uppercase()
            } else {
                r
            }
        }
        NumFormat::LowerAlpha => letters(n, &alphabet("abcdefghijklmnopqrstuvwxyz"), letter_sync)
            .into_iter()
            .collect(),
        NumFormat::UpperAlpha => letters(n, &alphabet("ABCDEFGHIJKLMNOPQRSTUVWXYZ"), letter_sync)
            .into_iter()
            .collect(),
        NumFormat::Chinese => chinese(n),
        NumFormat::Stems => letters(n, &alphabet(STEMS), letter_sync)
            .into_iter()
            .collect(),
        NumFormat::Circled => circled(n),
        NumFormat::Hebrew => hebrew(n.min(999)),
        NumFormat::Greek { upper } => {
            let s: String = letters(n, &alphabet(GREEK), letter_sync)
                .into_iter()
                .collect();
            if upper {
                s.to_uppercase()
            } else {
                s
            }
        }
        NumFormat::Cyrillic { upper } => {
            let s = letters(n, &alphabet(CYRILLIC), letter_sync);
            s.iter()
                .enumerate()
                .flat_map(|(i, c)| {
                    let up = upper && i == 0;
                    let v: Vec<char> = if up {
                        c.to_uppercase().collect()
                    } else {
                        vec![*c]
                    };
                    v
                })
                .collect()
        }
    }
}

/// `text:start-value`, which Writer drops past a signed 16-bit counter.
pub fn start_value(e: &Element) -> Option<u32> {
    e.attr(Ns::Text, "start-value")
        .and_then(|s| s.trim().parse().ok())
        .filter(|&v| v <= i16::MAX as u32)
}

fn length(e: &Element, ns: Ns, local: &str) -> Option<Pt> {
    e.attr(ns, local).and_then(units::length)
}

pub fn parse<'a>(xml: &'a Xml, style: &'a Element) -> ListStyle<'a> {
    let mut levels: Vec<Option<Level>> = vec![None; 10];
    let mut text_props = Vec::new();
    for level in xml.children(style) {
        let n: usize = level
            .attr(Ns::Text, "level")
            .and_then(|l| l.parse().ok())
            .unwrap_or(1);
        if !(1..=10).contains(&n) {
            continue;
        }
        let kind = match level.local.as_str() {
            "list-level-style-number" | "outline-level-style" => Kind::Number {
                format: num_format(level.attr(Ns::Style, "num-format")),
                display_levels: level
                    .attr(Ns::Text, "display-levels")
                    .and_then(|d| d.parse().ok())
                    .unwrap_or(1),
            },
            "list-level-style-bullet" => Kind::Bullet(
                level
                    .attr(Ns::Text, "bullet-char")
                    .unwrap_or("\u{2022}")
                    .to_string(),
            ),
            "list-level-style-image" => Kind::Image(
                level
                    .attr(Ns::Xlink, "href")
                    .unwrap_or_default()
                    .to_string(),
            ),
            _ => continue,
        };
        let props = xml.child(level, Ns::Style, "list-level-properties");
        let mode = props.and_then(|p| p.attr(Ns::Text, "list-level-position-and-space-mode"));
        let aligned = props.and_then(|p| xml.child(p, Ns::Style, "list-level-label-alignment"));
        let (placement, margin_left, text_indent) = match (mode, aligned) {
            (Some("label-alignment"), Some(a)) => {
                let follow = match a.attr(Ns::Text, "label-followed-by") {
                    Some("space") => LabelFollow::Space,
                    Some("nothing") => LabelFollow::Nothing,
                    _ => LabelFollow::Tab(length(a, Ns::Text, "list-tab-stop-position")),
                };
                (
                    LabelPlacement::Aligned { follow },
                    Some(length(a, Ns::Fo, "margin-left").unwrap_or(0.0)),
                    Some(length(a, Ns::Fo, "text-indent").unwrap_or(0.0)),
                )
            }
            _ => {
                let p = props;
                let get = |local| p.and_then(|p| length(p, Ns::Text, local)).unwrap_or(0.0);
                let align = match p.and_then(|p| p.attr(Ns::Fo, "text-align")) {
                    Some("center") => Align::Center,
                    Some("end") | Some("right") => Align::Right,
                    _ => Align::Left,
                };
                (
                    LabelPlacement::Positioned {
                        space_before: get("space-before"),
                        min_width: get("min-label-width"),
                        min_distance: get("min-label-distance"),
                        align,
                    },
                    None,
                    None,
                )
            }
        };
        let text = xml.child(level, Ns::Style, "text-properties").map(|t| {
            text_props.push(t);
            text_props.len() - 1
        });
        levels[n - 1] = Some(Level {
            kind,
            prefix: level
                .attr(Ns::Style, "num-prefix")
                .unwrap_or_default()
                .to_string(),
            template: level.attr(Ns::Loext, "num-list-format").map(str::to_string),
            suffix: level
                .attr(Ns::Style, "num-suffix")
                .unwrap_or_default()
                .to_string(),
            start: start_value(level).unwrap_or(1),
            letter_sync: level.attr(Ns::Style, "num-letter-sync") == Some("true")
                || format_syncs(level.attr(Ns::Style, "num-format")),
            char_style: level.attr(Ns::Text, "style-name").map(str::to_string),
            text,
            bullet_scale: level
                .attr(Ns::Text, "bullet-relative-size")
                .and_then(units::percent)
                .map(|p| p / 100.0),
            placement,
            margin_left,
            text_indent,
        });
    }
    // A level the style leaves undefined is `1.`, `2.`... in a list, bare in the outline.
    let fallback = if style.local == "outline-style" {
        Level {
            suffix: String::new(),
            ..plain(Kind::Number {
                format: NumFormat::None,
                display_levels: 1,
            })
        }
    } else {
        plain(Kind::Number {
            format: NumFormat::Decimal,
            display_levels: 1,
        })
    };
    ListStyle {
        levels: levels
            .into_iter()
            .map(|l| l.unwrap_or_else(|| fallback.clone()))
            .collect(),
        consecutive: style.attr(Ns::Text, "consecutive-numbering") == Some("true"),
        text_props,
    }
}

fn plain(kind: Kind) -> Level {
    let suffix = match kind {
        Kind::Number { .. } => ".".to_string(),
        _ => String::new(),
    };
    Level {
        kind,
        prefix: String::new(),
        suffix,
        template: None,
        start: 1,
        letter_sync: false,
        char_style: None,
        text: None,
        bullet_scale: None,
        placement: LabelPlacement::Positioned {
            space_before: 0.0,
            min_width: 0.0,
            min_distance: 0.0,
            align: Align::Left,
        },
        margin_left: None,
        text_indent: None,
    }
}

/// What a list whose style is missing shows: a bullet on every level.
pub fn default_bullets<'a>() -> ListStyle<'a> {
    ListStyle {
        levels: vec![plain(Kind::Bullet("\u{2022}".to_string())); 10],
        consecutive: false,
        text_props: Vec::new(),
    }
}

/// Counters of one list, shared by every `text:list` that continues it.
#[derive(Clone, Debug, Default)]
pub struct Counters {
    values: [Option<u32>; 10],
    /// One sequence for every level, the first slot holding it.
    pub consecutive: bool,
}

impl Counters {
    /// Count a paragraph at `level` (1-based) and return the counters it shows.
    /// When the paragraph shows a number (`numbered`), a level above with no
    /// paragraph counted yet takes its start value, as Writer's phantom nodes do.
    pub fn bump(
        &mut self,
        level: usize,
        start: &dyn Fn(usize) -> u32,
        restart: Option<u32>,
        numbered: bool,
    ) -> [u32; 10] {
        let i = level.clamp(1, 10) - 1;
        if self.consecutive {
            let v = restart.unwrap_or_else(|| {
                self.values[0].map_or_else(|| start(i), |v| v.saturating_add(1))
            });
            self.values[0] = Some(v);
            return [v; 10];
        }
        if numbered {
            for k in 0..i {
                self.values[k].get_or_insert_with(|| start(k));
            }
        }
        self.values[i] = Some(match (restart, self.values[i]) {
            (Some(r), _) => r,
            (None, Some(v)) => v.saturating_add(1),
            (None, None) => start(i),
        });
        for deeper in self.values.iter_mut().skip(i + 1) {
            *deeper = None;
        }
        let mut out = [0; 10];
        for (k, v) in self.values.iter().enumerate() {
            out[k] = v.unwrap_or_else(|| start(k));
        }
        out
    }
}

/// The label text of a paragraph at `level` whose counters read `values`.
pub fn label_text(style: &ListStyle, level: usize, values: &[u32; 10]) -> String {
    let i = level.clamp(1, 10) - 1;
    let l = &style.levels[i];
    let number = |k: usize| {
        let lk = &style.levels[k];
        match lk.kind {
            Kind::Number { format, .. } => format_number(values[k], format, lk.letter_sync),
            _ => String::new(),
        }
    };
    if let (
        Kind::Number {
            format: NumFormat::None,
            ..
        },
        Some(template),
    ) = (&l.kind, &l.template)
    {
        // No number on this level: the template keeps only what lies outside its placeholders.
        return match (template.find('%'), template.rfind('%')) {
            (Some(first), Some(last)) => format!("{}{}", &template[..first], &template[last + 1..]),
            _ => template.clone(),
        };
    }
    if let (Kind::Number { .. }, Some(template), false) = (&l.kind, &l.template, style.consecutive)
    {
        // A level above that shows no number takes the text after it away too.
        let mut out = String::new();
        let mut rest = template.as_str();
        let mut skip_text = false;
        while let Some(at) = rest.find('%') {
            if !skip_text {
                out.push_str(&rest[..at]);
            }
            let tail = &rest[at + 1..];
            let digits = tail.find('%').map(|end| &tail[..end]);
            match digits
                .and_then(|d| d.parse::<usize>().ok())
                .filter(|n| (1..=10).contains(n))
            {
                Some(n) => {
                    let shown = number(n - 1);
                    skip_text = shown.is_empty();
                    out.push_str(&shown);
                    rest = &tail[digits.map_or(0, str::len) + 1..];
                }
                None => {
                    out.push('%');
                    rest = tail;
                }
            }
        }
        if !skip_text {
            out.push_str(rest);
        }
        return out;
    }
    match &l.kind {
        Kind::Bullet(c) => c.to_string(),
        Kind::Image(_) => String::new(),
        Kind::Number {
            format: NumFormat::None,
            ..
        } => format!("{}{}", l.prefix, l.suffix),
        Kind::Number {
            format,
            display_levels,
        } => {
            let shown = if style.consecutive {
                1
            } else {
                (*display_levels).max(1) as usize
            };
            let from = (i + 1).saturating_sub(shown);
            let parts: Vec<String> = (from..=i)
                .map(|k| match style.levels[k].kind {
                    Kind::Number { .. } => number(k),
                    _ => format_number(values[k], *format, style.levels[k].letter_sync),
                })
                .filter(|s| !s.is_empty())
                .collect();
            format!("{}{}{}", l.prefix, parts.join("."), l.suffix)
        }
    }
}

/// The label's character properties: the paragraph's, then the level's
/// character style, then the level's own text properties.
pub fn label_props(
    styles: &Styles,
    scope: Scope,
    style: &ListStyle,
    level: usize,
    base: &Arc<TextProps>,
) -> Arc<TextProps> {
    let l = &style.levels[level.clamp(1, 10) - 1];
    let mut props = styles.text(scope, base, l.char_style.as_deref());
    if let Some(i) = l.text {
        let mut t = (*props).clone();
        apply_text(&mut t, style.text_props[i], &styles.fonts);
        props = Arc::new(t);
    }
    if let Some(scale) = l.bullet_scale {
        let mut t = (*props).clone();
        t.latin.size *= scale;
        props = Arc::new(t);
    }
    props
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphabetic_numbering_carries_like_a_spreadsheet_column_unless_letters_sync() {
        assert_eq!(format_number(1, NumFormat::LowerAlpha, false), "a");
        assert_eq!(format_number(26, NumFormat::LowerAlpha, false), "z");
        assert_eq!(format_number(27, NumFormat::LowerAlpha, false), "aa");
        assert_eq!(format_number(28, NumFormat::UpperAlpha, false), "AB");
        assert_eq!(format_number(28, NumFormat::UpperAlpha, true), "BB");
        assert_eq!(format_number(1994, NumFormat::UpperRoman, false), "MCMXCIV");
        assert_eq!(
            format_number(30, NumFormat::Cyrillic { upper: true }, false),
            "Аб"
        );
        assert!(format_syncs(Some("А, Б, .., Аа, Бб, ... (ru)")));
        assert!(!format_syncs(Some("а, б, .., аа, аб, ... (ru)")));
    }

    #[test]
    fn chinese_numerals_drop_the_leading_one_of_ten_and_mark_inner_zeros() {
        assert_eq!(format_number(10, NumFormat::Chinese, false), "十");
        assert_eq!(format_number(12, NumFormat::Chinese, false), "十二");
        assert_eq!(format_number(20, NumFormat::Chinese, false), "二十");
        assert_eq!(format_number(105, NumFormat::Chinese, false), "一百零五");
        assert_eq!(format_number(110, NumFormat::Chinese, false), "一百一十");
    }
}
