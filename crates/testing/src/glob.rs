// SPDX-License-Identifier: GPL-3.0-or-later

/// Match a path against a glob: `*` within one path segment, `**` across
/// segments (`src/**` is everything under `src`), `?` one character.
pub fn matches(glob: &str, path: &str) -> bool {
    let g: Vec<&str> = glob.trim_start_matches("./").split('/').collect();
    let p: Vec<&str> = path.trim_start_matches("./").split('/').collect();
    segs(&g, &p)
}

fn segs(g: &[&str], p: &[&str]) -> bool {
    match g.split_first() {
        None => p.is_empty(),
        Some((&"**", rest)) => {
            // `**` matches zero or more whole segments.
            (0..=p.len()).any(|n| segs(rest, &p[n..]))
        }
        Some((first, rest)) => match p.split_first() {
            Some((seg, prest)) => seg_match(first, seg) && segs(rest, prest),
            None => false,
        },
    }
}

fn seg_match(g: &str, s: &str) -> bool {
    let (g, s): (Vec<char>, Vec<char>) = (g.chars().collect(), s.chars().collect());
    fn go(g: &[char], s: &[char]) -> bool {
        match g.split_first() {
            None => s.is_empty(),
            Some(('*', rest)) => (0..=s.len()).any(|n| go(rest, &s[n..])),
            Some(('?', rest)) => !s.is_empty() && go(rest, &s[1..]),
            Some((c, rest)) => s.first() == Some(c) && go(rest, &s[1..]),
        }
    }
    go(&g, &s)
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn globs() {
        for (g, p, want) in [
            ("src/**", "src/a/b.rs", true),
            ("src/**", "src", true),
            ("src/**", "docs/a.md", false),
            ("**/*.md", "a/b/c.md", true),
            ("**/*.md", "c.md", true),
            ("*.md", "a/c.md", false),
            ("src/*.rs", "src/a.rs", true),
            ("src/*.rs", "src/a/b.rs", false),
            ("a?c", "abc", true),
            ("a?c", "ac", false),
            ("./src/**", "src/x", true),
            ("Makefile", "Makefile", true),
            ("**", "anything/at/all", true),
        ] {
            assert_eq!(matches(g, p), want, "{g} vs {p}");
        }
    }
}
