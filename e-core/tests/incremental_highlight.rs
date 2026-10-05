//! The incremental highlighter must colour exactly like the full one, after
//! every edit. `highlight_lines` (tree-sitter-highlight) is the oracle.
//!
//! Wider check against real projects (opt-in):
//! `E_HL_CORPUS=~/code cargo test -p e-core --release --test incremental_highlight -- --ignored --nocapture`

use e_core::language::Language;
use e_core::syntax::{highlight_lines, LineSpan, SyntaxTree};

/// Per-line spans in a canonical order, so equal colourings compare equal.
fn canon(lines: &[Vec<LineSpan>]) -> Vec<Vec<(usize, usize, String)>> {
    lines
        .iter()
        .map(|l| {
            let mut v: Vec<_> = l
                .iter()
                .map(|s| (s.start, s.end, format!("{:?}", s.kind)))
                .collect();
            v.sort();
            v
        })
        .collect()
}

fn first_difference(
    got: &[Vec<(usize, usize, String)>],
    want: &[Vec<(usize, usize, String)>],
) -> String {
    if got.len() != want.len() {
        return format!("{} lines, expected {}", got.len(), want.len());
    }
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        if g != w {
            return format!("line {i}: got {g:?}\n  expected {w:?}");
        }
    }
    String::new()
}

fn assert_full_matches(lang: Language, text: &str, what: &str) {
    let (_, lines) = SyntaxTree::new(lang, text).expect("supported");
    let (got, want) = (canon(&lines), canon(&highlight_lines(lang, text)));
    assert!(got == want, "{what}: {}", first_difference(&got, &want));
}

/// A small deterministic PRNG (xorshift), so failures reproduce.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn boundary(text: &str, mut i: usize) -> usize {
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Random edits — the syntax-wrecking kind included (quotes, comment
/// openers, braces, heredoc and PHP tags). After each one the spans must be
/// exactly what highlighting the whole tree gives (nothing stale left behind),
/// and — whenever the text parses cleanly — what `highlight_lines` gives.
/// Finally the original text is put back in one edit, and from that broken
/// state the colours must converge to the full highlight again.
fn assert_edits_match(lang: Language, start: &str, seed: u64, edits: usize) {
    const TOKENS: &[&str] = &[
        "\"",
        "'",
        "/*",
        "*/",
        "{",
        "}",
        "(",
        ")",
        "\n",
        ";",
        "//",
        "#",
        "`",
        "x",
        "$a",
        "é",
        " ",
        "\t",
        "DB::select(\"SELECT 1\")",
        "->whereRaw('a = 1')",
        "<?php",
        "?>",
        "<<<EOT\n",
        "\nEOT;\n",
        "fn ",
        "\"\"\"",
        "0x1f",
        "@media",
        ":",
        ",",
    ];
    let mut rng = Rng(seed);
    let (mut tree, mut lines) = SyntaxTree::new(lang, start).expect("supported");
    let mut text = start.to_string();
    let check = |tree: &SyntaxTree, lines: &[Vec<LineSpan>], text: &str, step: &str| {
        let (got, own) = (canon(lines), canon(&tree.highlight_all()));
        assert!(
            got == own,
            "{lang:?} seed {seed} {step}: stale spans: {}\n--- text ---\n{text}",
            first_difference(&got, &own)
        );
        if !tree.has_errors() {
            let want = canon(&highlight_lines(lang, text));
            assert!(
                got == want,
                "{lang:?} seed {seed} {step}: differs from highlight_lines: {}\n--- text ---\n{text}",
                first_difference(&got, &want)
            );
        }
    };
    for step in 0..edits {
        let at = boundary(&text, rng.below(text.len() + 1));
        if rng.below(3) == 0 && !text.is_empty() {
            let end = boundary(&text, (at + 1 + rng.below(24)).min(text.len()));
            text.replace_range(at..end.max(at), "");
        } else {
            text.insert_str(at, TOKENS[rng.below(TOKENS.len())]);
        }
        tree.update(&text, &mut lines);
        check(&tree, &lines, &text, &format!("step {step}"));
    }
    tree.update(start, &mut lines);
    check(&tree, &lines, start, "restored");
    if !SyntaxTree::new(lang, start).unwrap().0.has_errors() {
        assert!(
            !tree.has_errors(),
            "{lang:?} seed {seed}: no convergence on restore"
        );
    }
}

/// Edits that mostly keep the code valid — renaming identifiers, duplicating
/// and deleting lines, adding blank space — so the region path (taken only
/// between two clean parses) is what gets exercised. Returns how many steps
/// went from clean to clean.
fn assert_clean_edits_match(lang: Language, start: &str, seed: u64, edits: usize) -> usize {
    let mut rng = Rng(seed);
    let (mut tree, mut lines) = SyntaxTree::new(lang, start).expect("supported");
    let mut text = start.to_string();
    let mut clean_steps = 0;
    for step in 0..edits {
        let was_clean = !tree.has_errors();
        let line_starts: Vec<usize> = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .filter(|&i| i < text.len())
            .collect();
        match rng.below(5) {
            // Grow or shrink a word.
            0 | 1 => {
                let words: Vec<usize> = (1..text.len())
                    .filter(|&i| {
                        let b = text.as_bytes();
                        b[i - 1].is_ascii_alphabetic() && b[i].is_ascii_alphabetic()
                    })
                    .collect();
                if let Some(&i) = words.get(rng.below(words.len())) {
                    if rng.below(2) == 0 {
                        text.insert(i, (b'a' + rng.below(26) as u8) as char);
                    } else {
                        text.remove(i);
                    }
                }
            }
            // Duplicate a line.
            2 => {
                if let Some(&ls) = line_starts.get(rng.below(line_starts.len())) {
                    let le = text[ls..].find('\n').map_or(text.len(), |i| ls + i + 1);
                    let line = text[ls..le].to_string();
                    text.insert_str(le, &line);
                }
            }
            // Delete a line.
            3 => {
                if let Some(&ls) = line_starts.get(rng.below(line_starts.len())) {
                    let le = text[ls..].find('\n').map_or(text.len(), |i| ls + i + 1);
                    text.replace_range(ls..le, "");
                }
            }
            // Blank space at a line start.
            _ => {
                if let Some(&ls) = line_starts.get(rng.below(line_starts.len())) {
                    text.insert_str(ls, if rng.below(2) == 0 { "\n" } else { "    " });
                }
            }
        }
        tree.update(&text, &mut lines);
        let (got, own) = (canon(&lines), canon(&tree.highlight_all()));
        assert!(
            got == own,
            "{lang:?} clean seed {seed} step {step}: stale spans: {}\n--- text ---\n{text}",
            first_difference(&got, &own)
        );
        if !tree.has_errors() {
            if was_clean {
                clean_steps += 1;
            }
            let want = canon(&highlight_lines(lang, &text));
            assert!(
                got == want,
                "{lang:?} clean seed {seed} step {step}: differs from highlight_lines: {}\n--- text ---\n{text}",
                first_difference(&got, &want)
            );
        }
    }
    clean_steps
}

fn samples() -> Vec<(Language, String)> {
    let mut out = vec![(
        Language::Php,
        include_str!("fixtures/highlight/UserController.php").to_string(),
    )];
    let all = include_str!("fixtures/highlight/samples.txt");
    for chunk in all.split("=== ").filter(|c| !c.is_empty()) {
        let (tag, body) = chunk.split_once('\n').unwrap();
        let lang = match tag.trim() {
            "py" => Language::Python,
            "go" => Language::Go,
            "c" => Language::C,
            "css" => Language::Css,
            "json" => Language::Json,
            other => panic!("unknown sample {other}"),
        };
        out.push((lang, body.to_string()));
    }
    out.push((
        Language::Rust,
        include_str!("../src/undotree.rs").to_string(),
    ));
    out
}

#[test]
fn full_highlight_matches_tree_sitter_highlight() {
    for (lang, text) in samples() {
        assert_full_matches(lang, &text, &format!("{lang:?} sample"));
    }
    assert_full_matches(
        Language::Rust,
        include_str!("../src/syntax.rs"),
        "syntax.rs",
    );
}

#[test]
fn every_edit_matches_a_full_highlight() {
    for (i, (lang, text)) in samples().into_iter().enumerate() {
        for seed in 1..=4u64 {
            assert_edits_match(lang, &text, seed * 7919 + i as u64, 120);
        }
    }
}

#[test]
fn edits_that_keep_the_code_valid_match_a_full_highlight() {
    let mut clean = 0;
    for (i, (lang, text)) in samples().into_iter().enumerate() {
        for seed in 1..=12u64 {
            clean += assert_clean_edits_match(lang, &text, seed * 104_729 + i as u64, 150);
        }
    }
    // The point of this test is the clean-to-clean path; make sure it ran.
    assert!(clean > 800, "only {clean} clean-to-clean edits");
}

#[test]
fn edits_from_an_empty_document() {
    for lang in [
        Language::Php,
        Language::Rust,
        Language::Python,
        Language::Json,
    ] {
        assert_edits_match(lang, "", 42, 200);
    }
}

#[test]
fn renaming_the_db_call_recolours_its_sql_lines_away() {
    let text = "<?php\n$r = DB::select(\"\nSELECT id\nFROM users\n\");\n";
    let (mut tree, mut lines) = SyntaxTree::new(Language::Php, text).unwrap();
    let renamed = text.replace("select", "selectNot");
    tree.update(&renamed, &mut lines);
    assert_eq!(
        canon(&lines),
        canon(&highlight_lines(Language::Php, &renamed))
    );
}

/// Every supported file under `$E_HL_CORPUS`, highlighted both ways, plus a
/// few random edits each.
#[test]
#[ignore]
fn corpus() {
    let Some(root) = std::env::var_os("E_HL_CORPUS") else {
        return;
    };
    let mut stack = vec![std::path::PathBuf::from(root)];
    let (mut files, mut edited) = (0, 0);
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if !matches!(
                    name.as_str(),
                    ".git" | "node_modules" | "target" | "storage"
                ) {
                    stack.push(p);
                }
                continue;
            }
            let lang = Language::from_path(&p);
            if !SyntaxTree::supports(lang) || name.ends_with(".blade.php") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            if text.len() > 200_000 {
                continue;
            }
            assert_full_matches(lang, &text, &p.display().to_string());
            files += 1;
            if files % 10 == 0 {
                assert_edits_match(lang, &text, files as u64, 20);
                assert_clean_edits_match(lang, &text, files as u64, 40);
                edited += 1;
            }
        }
    }
    println!("corpus: {files} files identical, {edited} edited 60 times each");
}
