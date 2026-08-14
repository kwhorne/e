//! Fuzzy matching for the search dialog.
//!
//! The matcher this replaces was greedy and lowercased its input before
//! comparing, which threw away the one signal that matters most in a codebase:
//! `OC` should find `OrderController.php` instantly, and it can only know that
//! if it can still see where the words start.
//!
//! So this keeps the original text, scores word boundaries — `/`, `_`, `-`, `.`,
//! and the lower→upper transition inside a name — and finds the *best* alignment
//! rather than the first one. Paths are short and queries are shorter, so the
//! full table costs nothing worth saving.

/// A match, with where it landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub score: i32,
    /// Byte indices in the haystack that the query matched, ascending. Used to
    /// highlight the match in the list.
    pub positions: Vec<usize>,
}

// The weights. Ratios matter more than absolute values: a boundary match is
// worth about as much as two consecutive ones, which is what makes `oc` prefer
// `OrderController` over `documentation`.
const MATCH: i32 = 16;
const BONUS_BOUNDARY: i32 = 18;
const BONUS_CAMEL: i32 = 14;
const BONUS_CONSECUTIVE: i32 = 12;
const BONUS_FIRST_CHAR: i32 = 8;
const GAP_START: i32 = -5;
const GAP_EXTEND: i32 = -1;

fn is_separator(c: u8) -> bool {
    matches!(c, b'/' | b'\\' | b'_' | b'-' | b'.' | b' ' | b':')
}

/// The bonus for matching at `i`, from what precedes it.
fn bonus_at(text: &[u8], i: usize) -> i32 {
    if i == 0 {
        return BONUS_BOUNDARY;
    }
    let (prev, cur) = (text[i - 1], text[i]);
    if is_separator(prev) {
        BONUS_BOUNDARY
    } else if prev.is_ascii_lowercase() && cur.is_ascii_uppercase() {
        // The transition in `OrderController` — invisible to a matcher that
        // lowercases first, and the whole reason this one doesn't.
        BONUS_CAMEL
    } else if prev.is_ascii_digit() != cur.is_ascii_digit() {
        BONUS_CAMEL
    } else {
        0
    }
}

fn eq_ignore_case(a: u8, b: u8) -> bool {
    a.eq_ignore_ascii_case(&b)
}

/// Score `query` against `text`, or `None` when it isn't a subsequence.
///
/// Case-insensitive, but an exact-case hit scores higher — typing `OC` reads as
/// "the capitals", and honouring that is most of what makes the search feel
/// like it understands the codebase.
pub fn match_score(query: &str, text: &str) -> Option<Match> {
    if query.is_empty() {
        return Some(Match {
            score: 0,
            positions: Vec::new(),
        });
    }
    let q = query.as_bytes();
    let t = text.as_bytes();
    if q.len() > t.len() {
        return None;
    }

    // Cheap reject before building any table.
    {
        let mut qi = 0;
        for &c in t {
            if qi < q.len() && eq_ignore_case(c, q[qi]) {
                qi += 1;
            }
        }
        if qi != q.len() {
            return None;
        }
    }

    let (n, m) = (q.len(), t.len());
    // best[i][j] — score of matching q[..=i] with q[i] landing on t[j].
    let mut best = vec![i32::MIN; n * m];

    for i in 0..n {
        for j in 0..m {
            if !eq_ignore_case(t[j], q[i]) {
                continue;
            }
            let mut cell = i32::MIN;

            if i == 0 {
                // A leading gap costs, so an early match wins ties.
                let gap = if j == 0 {
                    0
                } else {
                    GAP_START + GAP_EXTEND * (j as i32 - 1)
                };
                cell = MATCH + bonus_at(t, j) + gap + if j == 0 { BONUS_FIRST_CHAR } else { 0 };
            } else {
                for k in 0..j {
                    let prev = best[(i - 1) * m + k];
                    if prev == i32::MIN {
                        continue;
                    }
                    let adjacent = k + 1 == j;
                    let gap = if adjacent {
                        0
                    } else {
                        GAP_START + GAP_EXTEND * (j as i32 - k as i32 - 2)
                    };
                    let bonus = if adjacent {
                        // A run is worth more than the boundary it might sit on,
                        // unless the boundary is stronger.
                        BONUS_CONSECUTIVE.max(bonus_at(t, j))
                    } else {
                        bonus_at(t, j)
                    };
                    let candidate = prev + MATCH + bonus + gap;
                    if candidate > cell {
                        cell = candidate;
                    }
                }
            }
            if cell != i32::MIN {
                // Exact case is a small, deliberate thumb on the scale.
                if t[j] == q[i] {
                    cell += 2;
                }
                best[i * m + j] = cell;
            }
        }
    }

    // Best end position for the last query char.
    let (mut end, mut score) = (usize::MAX, i32::MIN);
    for j in 0..m {
        let v = best[(n - 1) * m + j];
        if v > score {
            score = v;
            end = j;
        }
    }
    if end == usize::MAX {
        return None;
    }

    // Walk back for the positions.
    let mut positions = vec![0usize; n];
    let mut j = end;
    for i in (0..n).rev() {
        positions[i] = j;
        if i == 0 {
            break;
        }
        let target = best[i * m + j] - MATCH;
        let mut chosen = usize::MAX;
        for k in (0..j).rev() {
            let prev = best[(i - 1) * m + k];
            if prev == i32::MIN {
                continue;
            }
            // Any predecessor that could have produced this cell will do; the
            // rightmost keeps the run tight, which is what the score preferred.
            if prev <= target + BONUS_BOUNDARY + 2 {
                chosen = k;
                break;
            }
        }
        j = if chosen == usize::MAX {
            j.saturating_sub(1)
        } else {
            chosen
        };
    }

    Some(Match { score, positions })
}

/// Score a query against a repository-relative path.
///
/// The filename carries most of the intent — people search for `OrderController`,
/// not `app/Http/Controllers`. So the name is scored on its own and the full
/// path only tops it up, and shorter, shallower paths win ties.
pub fn match_path(query: &str, rel_path: &str) -> Option<Match> {
    let name_start = rel_path.rfind('/').map(|i| i + 1).unwrap_or(0);
    let name = &rel_path[name_start..];

    let on_name = match_score(query, name).map(|m| Match {
        score: m.score * 2 + 120,
        positions: m.positions.iter().map(|p| p + name_start).collect(),
    });
    let on_path = match_score(query, rel_path);

    let mut best = match (on_name, on_path) {
        (Some(n), Some(p)) => {
            if n.score >= p.score {
                Match {
                    score: n.score + p.score / 4,
                    positions: n.positions,
                }
            } else {
                p
            }
        }
        (Some(n), None) => n,
        (None, Some(p)) => p,
        (None, None) => return None,
    };

    // Exact and prefix hits on the filename are what someone typing a full name
    // expects to land on first.
    let name_l = name.to_ascii_lowercase();
    let q_l = query.to_ascii_lowercase();
    if name_l == q_l {
        best.score += 400;
    } else if let Some(stem) = name_l.split('.').next() {
        if stem == q_l {
            best.score += 320;
        } else if name_l.starts_with(&q_l) {
            best.score += 160;
        }
    }

    // Prefer shorter and shallower, gently — enough to break ties, not enough to
    // outrank a better match.
    best.score -= (rel_path.len() as i32) / 8;
    best.score -= rel_path.matches('/').count() as i32 * 3;
    Some(best)
}

/// Rank candidates, best first, keeping at most `limit`.
///
/// The file palette scores as it walks its own list, so this is the shared
/// definition of the ordering that the tests pin.
#[cfg(test)]
pub fn rank_paths<'a>(
    query: &str,
    paths: impl IntoIterator<Item = &'a str>,
    limit: usize,
) -> Vec<(&'a str, Match)> {
    let mut scored: Vec<(&str, Match)> = paths
        .into_iter()
        .filter_map(|p| match_path(query, p).map(|m| (p, m)))
        .collect();
    // Score first, then path, so equal scores come out in a stable order.
    scored.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(b.0)));
    scored.truncate(limit);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The order `rank_paths` puts these in.
    fn order<'a>(q: &str, paths: &[&'a str]) -> Vec<&'a str> {
        rank_paths(q, paths.iter().copied(), 100)
            .into_iter()
            .map(|(p, _)| p)
            .collect()
    }

    fn first<'a>(q: &str, paths: &[&'a str]) -> &'a str {
        order(q, paths)[0]
    }

    // ── the thing the old matcher could not do ──────────────────────────

    #[test]
    fn initials_find_the_camel_case_name() {
        // The whole point. A matcher that lowercases first cannot tell that
        // `OC` is the shape of `OrderController`.
        let paths = [
            "app/Http/Controllers/OrderController.php",
            "resources/views/documentation.blade.php",
            "config/cache.php",
        ];
        assert_eq!(
            first("oc", &paths),
            "app/Http/Controllers/OrderController.php"
        );
        assert_eq!(
            first("OC", &paths),
            "app/Http/Controllers/OrderController.php"
        );
    }

    #[test]
    fn initials_across_a_longer_name() {
        let paths = [
            "app/Http/Controllers/PasswordResetLinkController.php",
            "app/Models/Post.php",
            "app/Providers/RouteServiceProvider.php",
        ];
        assert_eq!(
            first("prlc", &paths),
            "app/Http/Controllers/PasswordResetLinkController.php"
        );
        assert_eq!(
            first("rsp", &paths),
            "app/Providers/RouteServiceProvider.php"
        );
    }

    #[test]
    fn snake_and_kebab_boundaries_count_too() {
        let paths = [
            "database/migrations/2024_01_01_create_orders_table.php",
            "resources/js/components/order-list.vue",
            "app/Models/Order.php",
        ];
        assert_eq!(
            first("cot", &paths),
            "database/migrations/2024_01_01_create_orders_table.php"
        );
        assert_eq!(
            first("ol", &paths),
            "resources/js/components/order-list.vue"
        );
    }

    // ── ordinary expectations ───────────────────────────────────────────

    #[test]
    fn the_exact_filename_wins() {
        let paths = [
            "app/Models/Order.php",
            "app/Models/OrderItem.php",
            "app/Http/Controllers/OrderController.php",
        ];
        assert_eq!(first("Order.php", &paths), "app/Models/Order.php");
        assert_eq!(first("order", &paths), "app/Models/Order.php");
    }

    #[test]
    fn a_prefix_beats_a_scattered_match() {
        let paths = ["app/Models/User.php", "resources/views/auth/register.php"];
        assert_eq!(first("user", &paths), "app/Models/User.php");
    }

    #[test]
    fn the_filename_outranks_the_directory() {
        // Typing `order` means the file called Order, not everything under an
        // `orders/` folder.
        let paths = [
            "resources/views/orders/index.blade.php",
            "resources/views/orders/show.blade.php",
            "app/Models/Order.php",
        ];
        assert_eq!(first("order", &paths), "app/Models/Order.php");
    }

    #[test]
    fn a_path_fragment_still_matches() {
        let paths = ["app/Models/Order.php", "tests/Feature/OrderTest.php"];
        assert_eq!(
            first("feature/order", &paths),
            "tests/Feature/OrderTest.php"
        );
    }

    #[test]
    fn shallower_wins_a_tie() {
        let paths = [
            "app/Models/Order.php",
            "modules/shop/src/Domain/Models/Order.php",
        ];
        assert_eq!(first("order", &paths), "app/Models/Order.php");
    }

    #[test]
    fn a_non_subsequence_does_not_match() {
        assert!(match_score("xyz", "app/Models/Order.php").is_none());
        assert!(match_path("zzz", "app/Models/Order.php").is_none());
        assert!(order("qqq", &["app/Models/Order.php"]).is_empty());
    }

    #[test]
    fn an_empty_query_matches_everything_at_zero() {
        let m = match_score("", "anything").unwrap();
        assert_eq!(m.score, 0);
        assert!(m.positions.is_empty());
    }

    #[test]
    fn a_query_longer_than_the_text_cannot_match() {
        assert!(match_score("abcdefghij", "abc").is_none());
    }

    // ── positions, for highlighting ─────────────────────────────────────

    #[test]
    fn positions_point_at_the_matched_characters() {
        let m = match_score("oc", "OrderController").unwrap();
        let hit: String = m
            .positions
            .iter()
            .map(|&i| "OrderController".as_bytes()[i] as char)
            .collect();
        assert_eq!(hit.to_ascii_lowercase(), "oc");
        assert_eq!(m.positions[0], 0);
        assert_eq!(m.positions[1], 5, "the C of Controller, not an earlier c");
    }

    #[test]
    fn positions_are_ascending_and_one_per_query_char() {
        let m = match_score("prlc", "PasswordResetLinkController.php").unwrap();
        assert_eq!(m.positions.len(), 4);
        assert!(
            m.positions.windows(2).all(|w| w[0] < w[1]),
            "{:?}",
            m.positions
        );
    }

    #[test]
    fn path_positions_are_offsets_into_the_whole_path() {
        let rel = "app/Http/Controllers/OrderController.php";
        let m = match_path("oc", rel).unwrap();
        for &p in &m.positions {
            assert!(p < rel.len());
        }
        let hit: String = m
            .positions
            .iter()
            .map(|&i| rel.as_bytes()[i] as char)
            .collect();
        assert_eq!(hit.to_ascii_lowercase(), "oc");
    }

    // ── ordering as a whole ─────────────────────────────────────────────

    #[test]
    fn a_realistic_laravel_tree_ranks_sensibly() {
        let paths = [
            "app/Http/Controllers/OrderController.php",
            "app/Http/Controllers/OrderItemController.php",
            "app/Models/Order.php",
            "app/Models/OrderItem.php",
            "database/migrations/2024_01_01_create_orders_table.php",
            "resources/views/orders/index.blade.php",
            "tests/Feature/OrderTest.php",
            "vendor-ish/other/Recorder.php",
        ];
        let got = order("order", &paths);
        assert_eq!(got[0], "app/Models/Order.php");
        // Everything named Order* should come before an incidental match.
        let recorder = got.iter().position(|p| p.contains("Recorder")).unwrap();
        let test = got.iter().position(|p| p.contains("OrderTest")).unwrap();
        assert!(test < recorder, "{got:?}");
    }

    #[test]
    fn the_limit_is_respected() {
        let paths: Vec<String> = (0..50).map(|i| format!("app/File{i}.php")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        assert_eq!(rank_paths("file", refs, 10).len(), 10);
    }

    #[test]
    fn equal_scores_come_out_in_a_stable_order() {
        // Two identical shapes; the order must not depend on input order.
        let a = order("x", &["a/x.php", "b/x.php"]);
        let b = order("x", &["b/x.php", "a/x.php"]);
        assert_eq!(a, b);
    }
}

/// Against a real project tree, printed for a human to judge.
///
/// ```sh
/// E_RANK_ROOT=/path/to/project E_RANK_Q=oc \
///   cargo test -p e-app live_rank -- --ignored --nocapture
/// ```
#[cfg(test)]
mod live_rank {
    use super::*;

    #[test]
    #[ignore]
    fn top_hits_for_a_query() {
        let Ok(root) = std::env::var("E_RANK_ROOT") else {
            eprintln!("set E_RANK_ROOT and E_RANK_Q — skipping");
            return;
        };
        let q = std::env::var("E_RANK_Q").unwrap_or_else(|_| "oc".into());
        let root = std::path::PathBuf::from(root);

        let mut paths: Vec<String> = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = e.path();
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.')
                    || name == "target"
                    || name == "node_modules"
                    || name == "vendor"
                {
                    continue;
                }
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(rel) = p.strip_prefix(&root) {
                    paths.push(rel.to_string_lossy().into_owned());
                }
            }
        }
        println!("{} files, query {q:?}\n", paths.len());
        for (p, m) in rank_paths(&q, paths.iter().map(String::as_str), 10) {
            println!("{:>6}  {p}", m.score);
        }
    }
}
