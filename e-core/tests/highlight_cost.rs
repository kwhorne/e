//! Measures what one keystroke costs today. Opt-in:
//! `cargo test -p e-core --test highlight_cost -- --ignored --nocapture`
use std::time::Instant;

use e_core::language::Language;
use e_core::syntax::highlight_lines;

fn bench(label: &str, lang: Language, text: &str, runs: u32) {
    // warm the thread-local grammar config
    let _ = highlight_lines(lang, text);
    let start = Instant::now();
    for _ in 0..runs {
        let _ = highlight_lines(lang, text);
    }
    let per = start.elapsed() / runs;
    println!(
        "{label:<28} {:>7} KB  {:>8.2} ms/keystroke",
        text.len() / 1024,
        per.as_secs_f64() * 1000.0
    );
}

#[test]
#[ignore]
fn cost_of_one_keystroke() {
    let rust = std::fs::read_to_string("../e-app/src/state.rs").unwrap();
    println!();
    bench("rust (state.rs)", Language::Rust, &rust, 20);
    bench("rust, 1/4", Language::Rust, &rust[..rust.len() / 4], 20);

    // PHP goes through two full parses: ts_spans + php_sql_spans.
    let php_unit = "<?php\nclass A{ public function b(){ $x = DB::select(\"SELECT * FROM t WHERE id = 1\"); return $x; } }\n";
    for factor in [200usize, 800, 3200] {
        let php = php_unit.repeat(factor);
        bench(&format!("php x{factor}"), Language::Php, &php, 10);
    }

    let blade_unit =
        "<div class=\"flex items-center gap-2\">@if($u)<x-btn :for=\"$u\" />@endif</div>\n";
    for factor in [400usize, 1600] {
        let blade = blade_unit.repeat(factor);
        bench(&format!("blade x{factor}"), Language::Blade, &blade, 10);
    }
}
