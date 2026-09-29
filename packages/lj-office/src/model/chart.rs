//! A chart, drawn from the values its part caches.

use super::{Block, Inline, Paint, RunProps, Stroke, Text};
use crate::Anchor;

pub struct Chart {
    /// Empty without a title.
    pub title: Vec<Block>,
    pub plots: Vec<Plot>,
    /// The labels of the categories every plot shares.
    pub categories: Vec<ChartText>,
    /// `None` when the axis is deleted.
    pub category_axis: Option<Axis>,
    pub value_axis: Option<Axis>,
    /// On the other side of the plot, for the plots marked secondary.
    pub secondary_axis: Option<Axis>,
    pub legend: Option<Legend>,
    pub fill: Option<Paint>,
    pub stroke: Option<Stroke>,
    /// Drawn in perspective, which LibreOffice scales without a margin.
    pub three_d: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plot {
    pub kind: PlotKind,
    pub series: Vec<Series>,
    pub secondary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlotKind {
    /// `gap` and `overlap` are shares of a bar's width.
    Bar {
        horizontal: bool,
        grouping: Grouping,
        gap: f64,
        overlap: f64,
    },
    Line {
        grouping: Grouping,
    },
    Area {
        grouping: Grouping,
    },
    /// `hole` is the share of the radius a doughnut leaves empty; the legend
    /// of a pie names its categories.
    Pie {
        hole: f64,
        first_angle: f64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grouping {
    Standard,
    Stacked,
    Percent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub name: ChartText,
    pub values: Vec<Option<f64>>,
    pub paint: Paint,
    pub line_width: f64,
    /// The colour of each slice of a pie.
    pub points: Vec<Paint>,
}

/// A label, anchored when the part holds it in one text node.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartText {
    pub text: String,
    /// One per character of `text`, or none for what the reader wrote.
    pub anchors: Vec<Anchor>,
}

impl ChartText {
    pub fn inlines(&self, props: &RunProps) -> Vec<Inline> {
        if self.anchors.is_empty() {
            return self
                .text
                .chars()
                .map(|c| Inline::Char(c, props.clone()))
                .collect();
        }
        vec![Inline::Text(Text {
            text: self.text.clone(),
            props: props.clone(),
            anchors: self.anchors.clone(),
        })]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Axis {
    /// `None` when the axis shows no labels.
    pub labels: Option<RunProps>,
    pub format: NumberFormat,
    pub gridlines: bool,
    pub ticks_out: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub major: Option<f64>,
    pub line: Option<Stroke>,
    /// Categories on the ticks rather than between them.
    pub on_ticks: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Legend {
    pub side: Side,
    pub props: RunProps,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Right,
    Left,
    Top,
    Bottom,
}

/// A spreadsheet number format code.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct NumberFormat {
    pub code: String,
    /// Serial dates count from 1904 rather than 1900.
    pub date1904: bool,
}

impl NumberFormat {
    pub fn format(&self, value: f64) -> String {
        let sections = split_sections(&self.code);
        let (section, value, sign) = match sections.get(1) {
            Some(negative) if value < 0.0 => (negative.as_str(), -value, ""),
            _ => (
                sections.first().map_or("", String::as_str),
                value.abs(),
                if value < 0.0 { "-" } else { "" },
            ),
        };
        if general(section) {
            return format!("{sign}{}", general_number(value));
        }
        let tokens = tokens(section);
        if tokens
            .iter()
            .any(|t| matches!(t, Token::Code(c) if "dmy".contains(c.to_ascii_lowercase())))
        {
            return date(&tokens, value, self.date1904);
        }
        number(&tokens, value, sign)
    }
}

fn general(section: &str) -> bool {
    let s = section.trim();
    s.is_empty() || s.eq_ignore_ascii_case("general")
}

/// The sections of a code, split on the semicolons outside quotes.
fn split_sections(code: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut quoted = false;
    let mut escaped = false;
    for c in code.chars() {
        let section = out.last_mut().expect("one section");
        if escaped {
            section.push(c);
            escaped = false;
            continue;
        }
        match c {
            '"' => quoted = !quoted,
            '\\' if !quoted => escaped = true,
            ';' if !quoted => {
                out.push(String::new());
                continue;
            }
            _ => {}
        }
        out.last_mut().expect("one section").push(c);
    }
    out
}

#[derive(Debug)]
enum Token {
    Literal(String),
    /// A digit placeholder, a separator or a date letter.
    Code(char),
}

fn tokens(section: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut chars = section.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let text: String = chars.by_ref().take_while(|&c| c != '"').collect();
                out.push(Token::Literal(text));
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push(Token::Literal(next.to_string()));
                }
            }
            '[' => {
                let inner: String = chars.by_ref().take_while(|&c| c != ']').collect();
                if let Some(currency) = inner.strip_prefix('$') {
                    let symbol = currency.split('-').next().unwrap_or("");
                    out.push(Token::Literal(symbol.to_string()));
                }
            }
            '_' | '*' => {
                chars.next();
            }
            '0' | '#' | '?' | '.' | ',' | '%' => out.push(Token::Code(c)),
            c if "dmyhsDMYHS".contains(c) => out.push(Token::Code(c)),
            c => out.push(Token::Literal(c.to_string())),
        }
    }
    out
}

/// A number as the General format shows it: ten significant digits at most,
/// no trailing zero.
fn general_number(value: f64) -> String {
    if value == value.trunc() && value.abs() < 1e15 {
        return format!("{value:.0}");
    }
    let magnitude = value.abs().log10().floor() as i32;
    let decimals = (9 - magnitude).clamp(0, 15) as usize;
    let mut s = format!("{value:.decimals$}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

fn number(tokens: &[Token], mut value: f64, sign: &str) -> String {
    let is_digit = |t: &Token| matches!(t, Token::Code('0' | '#' | '?'));
    let (Some(first), Some(last)) = (
        tokens.iter().position(is_digit),
        tokens.iter().rposition(is_digit),
    ) else {
        let mut out = sign.to_string();
        for t in tokens {
            match t {
                Token::Literal(s) => out.push_str(s),
                Token::Code(c) => out.push(*c),
            }
        }
        return out;
    };
    if tokens.iter().any(|t| matches!(t, Token::Code('%'))) {
        value *= 100.0;
    }
    let body = &tokens[first..=last];
    let point = body.iter().position(|t| matches!(t, Token::Code('.')));
    let whole = &body[..point.unwrap_or(body.len())];
    let decimals = point.map_or(0, |p| body[p..].iter().filter(|t| is_digit(t)).count());
    let grouped = whole.iter().any(|t| matches!(t, Token::Code(',')));
    let keeps_zero = whole.iter().any(|t| matches!(t, Token::Code('0')));
    let fixed = format!("{value:.decimals$}");
    let (integer, fraction) = fixed.split_once('.').unwrap_or((&fixed, ""));
    let mut integer = integer.to_string();
    if integer == "0" && !keeps_zero {
        integer.clear();
    }
    if grouped {
        let digits: Vec<char> = integer.chars().collect();
        integer = digits
            .iter()
            .enumerate()
            .flat_map(|(i, c)| {
                let separator = i > 0 && (digits.len() - i).is_multiple_of(3);
                separator.then_some(',').into_iter().chain(Some(*c))
            })
            .collect();
    }
    let mut out = sign.to_string();
    for t in &tokens[..first] {
        push_literal(t, &mut out);
    }
    out.push_str(&integer);
    if decimals > 0 {
        out.push('.');
        out.push_str(fraction);
    }
    for t in &tokens[last + 1..] {
        push_literal(t, &mut out);
    }
    out
}

fn push_literal(token: &Token, out: &mut String) {
    match token {
        Token::Literal(s) => out.push_str(s),
        Token::Code('%') => out.push('%'),
        Token::Code(_) => {}
    }
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A serial date through a format's day, month and year letters.
fn date(tokens: &[Token], serial: f64, date1904: bool) -> String {
    let days = serial.floor() as i64 + if date1904 { 1462 } else { 0 };
    let (year, month, day) = civil(days - 25569 + 719_468);
    let month_name = MONTHS[(month - 1) as usize];
    let mut out = String::new();
    let mut i = 0;
    while i < tokens.len() {
        let c = match &tokens[i] {
            Token::Literal(s) => {
                out.push_str(s);
                i += 1;
                continue;
            }
            Token::Code(c) => c.to_ascii_lowercase(),
        };
        let run = tokens[i..]
            .iter()
            .take_while(|t| matches!(t, Token::Code(k) if k.to_ascii_lowercase() == c))
            .count();
        match (c, run) {
            ('y', 1 | 2) => out.push_str(&format!("{:02}", year % 100)),
            ('y', _) => out.push_str(&year.to_string()),
            ('m', 1) => out.push_str(&month.to_string()),
            ('m', 2) => out.push_str(&format!("{month:02}")),
            ('m', 3) => out.push_str(&month_name[..3]),
            ('m', _) => out.push_str(month_name),
            ('d', 1) => out.push_str(&day.to_string()),
            ('d', _) => out.push_str(&format!("{day:02}")),
            ('.' | ',', _) => out.extend(std::iter::repeat_n(c, run)),
            _ => {}
        }
        i += run;
    }
    out
}

/// Year, month and day of a count of days from 0000-03-01.
fn civil(z: i64) -> (i64, i64, i64) {
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::NumberFormat;

    fn format(code: &str, value: f64) -> String {
        NumberFormat {
            code: code.into(),
            date1904: false,
        }
        .format(value)
    }

    #[test]
    fn a_value_is_shown_as_its_format_code_writes_it() {
        assert_eq!(format("General", 4.4 + 1e-15), "4.4");
        assert_eq!(format("General", 0.5), "0.5");
        assert_eq!(format("General", 10000.0), "10000");
        assert_eq!(format("0%", 0.3), "30%");
        assert_eq!(format("\"$\"#,##0", 12000.0), "$12,000");
        assert_eq!(format("[$€-40C]#,##0.00", 1234.5), "€1,234.50");
        assert_eq!(format("0.0;(0.0)", -2.3), "(2.3)");
        assert_eq!(format("m/d/yyyy", 37261.0), "1/5/2002");
        assert_eq!(format("mmm-yy", 45000.0), "Mar-23");
    }
}
