// SPDX-License-Identifier: GPL-3.0-or-later

//! One design-token file (`clients/tokens.toml`), generated for both clients.

use toml_edit::{DocumentMut, Item};

fn camel(path: &[&str]) -> String {
    let mut out = String::new();
    for (i, part) in path.iter().enumerate() {
        for (j, word) in part.split('_').enumerate() {
            let mut cs = word.chars();
            if let Some(f) = cs.next() {
                if i == 0 && j == 0 {
                    out.extend(f.to_lowercase());
                } else {
                    out.extend(f.to_uppercase());
                }
                out.push_str(cs.as_str());
            }
        }
    }
    out
}

enum Value {
    Text(String),
    Number(i64),
}

fn flatten(doc: &DocumentMut) -> Result<Vec<(String, Value)>, String> {
    fn walk(
        item: &Item,
        path: &mut Vec<String>,
        out: &mut Vec<(String, Value)>,
    ) -> Result<(), String> {
        if let Some(t) = item.as_table_like() {
            for (k, v) in t.iter() {
                path.push(k.to_string());
                walk(v, path, out)?;
                path.pop();
            }
            return Ok(());
        }
        let parts: Vec<&str> = path.iter().map(String::as_str).collect();
        let name = camel(&parts);
        if let Some(s) = item.as_str() {
            out.push((name, Value::Text(s.to_string())));
        } else if let Some(n) = item.as_integer() {
            out.push((name, Value::Number(n)));
        } else {
            return Err(format!(
                "{}: only strings and whole numbers are tokens",
                path.join(".")
            ));
        }
        Ok(())
    }
    let mut out = vec![];
    for (k, v) in doc.iter() {
        walk(v, &mut vec![k.to_string()], &mut out)?;
    }
    Ok(out)
}

const HEADER: &str =
    "// Generated from clients/tokens.toml by `conformance tokens --write`. Do not edit.\n";

pub fn swift(text: &str) -> Result<String, String> {
    let doc: DocumentMut = text.parse().map_err(|e| format!("not valid TOML: {e}"))?;
    let mut s = format!(
        "// SPDX-License-Identifier: GPL-3.0-or-later\n{HEADER}\npublic enum MasonTokens {{\n"
    );
    for (name, v) in flatten(&doc)? {
        match v {
            Value::Text(t) => s.push_str(&format!("    public static let {name} = \"{t}\"\n")),
            Value::Number(n) => s.push_str(&format!("    public static let {name} = {n}\n")),
        }
    }
    s.push_str("}\n");
    Ok(s)
}

pub fn kotlin(text: &str) -> Result<String, String> {
    let doc: DocumentMut = text.parse().map_err(|e| format!("not valid TOML: {e}"))?;
    let mut s = format!("// SPDX-License-Identifier: GPL-3.0-or-later\n{HEADER}\npackage mason\n\nobject MasonTokens {{\n");
    for (name, v) in flatten(&doc)? {
        match v {
            Value::Text(t) => s.push_str(&format!("    const val {name} = \"{t}\"\n")),
            Value::Number(n) => s.push_str(&format!("    const val {name} = {n}\n")),
        }
    }
    s.push_str("}\n");
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "[color.light]\nattention_tint = \"#FDECEC\"\n[size]\ntile_min_width = 64\n";

    #[test]
    fn both_languages_get_the_same_names_and_values() {
        let sw = swift(T).unwrap();
        let kt = kotlin(T).unwrap();
        assert!(
            sw.contains("public static let colorLightAttentionTint = \"#FDECEC\""),
            "{sw}"
        );
        assert!(
            sw.contains("public static let sizeTileMinWidth = 64"),
            "{sw}"
        );
        assert!(
            kt.contains("const val colorLightAttentionTint = \"#FDECEC\""),
            "{kt}"
        );
        assert!(kt.contains("const val sizeTileMinWidth = 64"), "{kt}");
        assert!(kt.contains("package mason"));
    }

    #[test]
    fn only_strings_and_whole_numbers_are_tokens() {
        assert!(swift("[a]\nb = 1.5\n").is_err());
        assert!(kotlin("[a]\nb = [1]\n").is_err());
        assert!(swift("not toml [").is_err());
    }
}
