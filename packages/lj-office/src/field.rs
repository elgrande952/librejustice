//! The syntax of a Word field instruction, `HYPERLINK "mailto:x" \o "y"`.

/// Positions in `instruction` of its markup: the keyword, each switch, and the
/// picture a formatting switch (`\*`, `\#`, `\@`) takes. An argument, quoted or
/// not, is text the file carries: a link target, a bookmark, a field name.
pub(crate) fn markup(instruction: &[char]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut at = 0;
    let mut keyword = true;
    let mut picture = false;
    while at < instruction.len() {
        if instruction[at].is_whitespace() {
            at += 1;
            continue;
        }
        let start = at;
        if instruction[at] == '"' {
            at += 1;
            while at < instruction.len() && instruction[at] != '"' {
                at += 1 + usize::from(instruction[at] == '\\');
            }
            at = (at + 1).min(instruction.len());
        } else {
            while at < instruction.len()
                && !instruction[at].is_whitespace()
                && instruction[at] != '"'
            {
                at += 1;
            }
        }
        let switch = instruction[start] == '\\';
        if keyword || switch || picture {
            out.extend(start..at);
        }
        picture = switch && at - start == 2 && matches!(instruction[start + 1], '*' | '#' | '@');
        keyword = false;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(instruction: &str) -> String {
        let chars: Vec<char> = instruction.chars().collect();
        let gone = markup(&chars);
        chars
            .iter()
            .enumerate()
            .map(|(i, c)| if gone.contains(&i) { ' ' } else { *c })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn a_field_keeps_its_arguments_and_loses_its_keyword_its_switches_and_its_pictures() {
        for (instruction, arguments) in [
            (
                r#" HYPERLINK "mailto:jean.dupont@exemple.fr" \o "Écrire à Me Dupont" "#,
                r#""mailto:jean.dupont@exemple.fr" "Écrire à Me Dupont""#,
            ),
            (r#"HYPERLINK \l "_Toc12""#, r#""_Toc12""#),
            (r" PAGEREF _Toc123456 \h ", "_Toc123456"),
            (r#" TOC \o "1-3" \h \z \u "#, r#""1-3""#),
            (r" MERGEFIELD Nom_client \* MERGEFORMAT ", "Nom_client"),
            (r#" DATE \@ "d MMMM yyyy" \*MERGEFORMAT"#, ""),
            (
                r#"INCLUDEPICTURE "C:\\Users\\jdupont\\a \"b\".png" \* MERGEFORMATINET"#,
                r#""C:\\Users\\jdupont\\a \"b\".png""#,
            ),
        ] {
            assert_eq!(kept(instruction), arguments, "{instruction}");
        }
    }
}
