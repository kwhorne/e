//! Structured test results.
//!
//! Running the suite told you an exit code and left you to read the output.
//! Which test failed, and where, was something you worked out by eye. PHPUnit,
//! Pest, Vitest and Jest can all write JUnit XML, so this parses that into
//! something the editor can put a cursor in.
//!
//! The parse is deliberately small: JUnit XML is a handful of nested elements
//! and the interesting parts are attributes on `<testcase>`. Pulling in an XML
//! crate to read four attributes would be a poor trade, and the shapes the
//! runners actually emit are pinned by tests below.

use std::path::{Path, PathBuf};

/// How one test ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    /// An error rather than an assertion failure — PHPUnit distinguishes them.
    Errored,
    Skipped,
}

impl Outcome {
    pub fn is_problem(self) -> bool {
        matches!(self, Outcome::Failed | Outcome::Errored)
    }
}

/// One test case.
#[derive(Debug, Clone, PartialEq)]
pub struct TestCase {
    /// The suite it belongs to, e.g. `Tests\Feature\OrderTest`.
    pub suite: String,
    /// The test's own name.
    pub name: String,
    pub outcome: Outcome,
    pub duration_s: f64,
    /// Where it is defined, when the runner said.
    pub file: Option<PathBuf>,
    /// 1-based.
    pub line: Option<u32>,
    /// The failure text, for the ones that failed.
    pub message: Option<String>,
}

impl TestCase {
    /// `Tests\Feature\OrderTest::it_places_an_order`, for display.
    pub fn full_name(&self) -> String {
        if self.suite.is_empty() {
            self.name.clone()
        } else {
            format!("{}::{}", self.suite, self.name)
        }
    }
}

/// A whole run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TestRun {
    pub cases: Vec<TestCase>,
}

impl TestRun {
    pub fn passed(&self) -> usize {
        self.count(Outcome::Passed)
    }
    pub fn skipped(&self) -> usize {
        self.count(Outcome::Skipped)
    }
    pub fn failed(&self) -> usize {
        self.cases.iter().filter(|c| c.outcome.is_problem()).count()
    }
    fn count(&self, o: Outcome) -> usize {
        self.cases.iter().filter(|c| c.outcome == o).count()
    }

    pub fn is_empty(&self) -> bool {
        self.cases.is_empty()
    }

    /// The failures, which is what a reader wants first.
    pub fn problems(&self) -> impl Iterator<Item = &TestCase> {
        self.cases.iter().filter(|c| c.outcome.is_problem())
    }

    /// `12 passed · 2 failed · 1 skipped`, for the status line.
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("{} passed", self.passed())];
        if self.failed() > 0 {
            parts.push(format!("{} failed", self.failed()));
        }
        if self.skipped() > 0 {
            parts.push(format!("{} skipped", self.skipped()));
        }
        parts.join(" · ")
    }
}

/// Read one XML attribute off a tag.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let start = tag.find(&key)? + key.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// XML entities that appear in failure messages.
fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#10;", "\n")
        .replace("&#9;", "\t")
        // Last, or it would corrupt the ones above.
        .replace("&amp;", "&")
}

/// Is this attribute a file path, rather than a human-readable description?
///
/// `php artisan test` writes `file="Demo (Tests\Feature\Demo)::It fails"`, which
/// would otherwise be joined to the project root and produce a path that opens
/// nothing.
fn looks_like_a_path(s: &str) -> bool {
    !s.is_empty()
        && !s.contains("::")
        && !s.contains(' ')
        && (s.ends_with(".php") || s.ends_with(".js") || s.ends_with(".ts"))
}

/// Pull `at tests/Feature/DemoTest.php:7` out of a failure message.
///
/// PHPUnit puts the location only here, so without this a failure has nowhere
/// to jump to.
fn location_in(message: &str) -> Option<(String, u32)> {
    message.lines().rev().find_map(|l| {
        let l = l.trim();
        let rest = l.strip_prefix("at ").unwrap_or(l);
        let (path, line) = rest.rsplit_once(':')?;
        let line: u32 = line.trim().parse().ok()?;
        looks_like_a_path(path).then(|| (path.to_string(), line))
    })
}

/// Resolve a reported path against the project.
fn resolve(f: &str, root: &Path) -> PathBuf {
    let p = Path::new(f);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    }
}
/// Parse a JUnit XML report.
///
/// `root` resolves the relative paths some runners emit.
pub fn parse_junit(xml: &str, root: &Path) -> TestRun {
    let mut run = TestRun::default();
    // Walk `<testcase ...>` tags; everything else is scaffolding.
    let mut rest = xml;
    while let Some(i) = rest.find("<testcase") {
        rest = &rest[i..];
        let Some(tag_end) = rest.find('>') else { break };
        let tag = &rest[..tag_end];
        let self_closing = tag.trim_end().ends_with('/');

        // The body, up to `</testcase>`, holds any failure element.
        let body_end = rest.find("</testcase>").unwrap_or(tag_end);
        let body = if self_closing {
            ""
        } else {
            &rest[tag_end + 1..body_end.max(tag_end + 1)]
        };

        let (outcome, message) = classify(body);
        // Where the test lives. The `file` attribute is only trusted when it
        // actually looks like a path: `php artisan test` puts a *description*
        // there — `file="Demo (Tests\\Feature\\Demo)::It fails"` — and joining
        // that to the project root produces a path that opens nothing.
        let attr_file = attr(tag, "file").filter(|f| looks_like_a_path(f));
        let attr_line = attr(tag, "line").and_then(|l| l.parse().ok());
        // A failure message ends with `at tests/Feature/DemoTest.php:7`, which
        // is the real location and the only one PHPUnit gives for a failure.
        let from_message = message.as_deref().and_then(location_in);
        let (file, line) = match (attr_file, from_message) {
            (Some(f), Some((_, l))) => (Some(f.to_string()), attr_line.or(Some(l))),
            (Some(f), None) => (Some(f.to_string()), attr_line),
            (None, Some((f, l))) => (Some(f), Some(l)),
            (None, None) => (None, attr_line),
        };
        run.cases.push(TestCase {
            suite: attr(tag, "classname").unwrap_or("").to_string(),
            name: attr(tag, "name").unwrap_or("").to_string(),
            outcome,
            duration_s: attr(tag, "time")
                .and_then(|t| t.parse().ok())
                .unwrap_or(0.0),
            file: file.map(|f| resolve(&f, root)),
            line,
            message,
        });
        rest = &rest[tag_end + 1..];
    }
    run
}

/// Decide a case's outcome from the elements inside it.
fn classify(body: &str) -> (Outcome, Option<String>) {
    for (tag, outcome) in [
        ("<failure", Outcome::Failed),
        ("<error", Outcome::Errored),
        ("<skipped", Outcome::Skipped),
    ] {
        let Some(i) = body.find(tag) else { continue };
        let after = &body[i..];
        // Prefer the element's text, which carries the stack trace; fall back to
        // the `message` attribute, which is all a self-closing element has.
        let text = after
            .find('>')
            .map(|g| &after[g + 1..])
            .and_then(|t| t.find("</").map(|e| t[..e].trim()))
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .or_else(|| {
                after
                    .find('>')
                    .and_then(|g| attr(&after[..g], "message"))
                    .map(str::to_string)
            });
        return (outcome, text.map(|t| unescape(&t)));
    }
    (Outcome::Passed, None)
}

/// A test command with JUnit output turned on.
///
/// Returns the command and the report path. `None` when the runner is one this
/// doesn't know how to ask — better to keep the plain run than to pass a flag
/// that makes it fail.
pub fn junit_command(base: &str, report: &Path) -> Option<String> {
    let r = report.display();
    let c = base.trim();
    if c.contains("artisan test") || c.contains("phpunit") || c.contains("pest") {
        Some(format!("{c} --log-junit {r}"))
    } else if c.starts_with("npm test") || c.contains("vitest") {
        Some(format!("{c} -- --reporter=junit --outputFile={r}"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/app")
    }

    #[test]
    fn a_phpunit_report_is_parsed() {
        // The shape `php artisan test --log-junit` writes.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites>
  <testsuite name="Tests\Feature\OrderTest" tests="3" failures="1">
    <testcase name="it_lists_orders" class="Tests\Feature\OrderTest"
              classname="Tests\Feature\OrderTest" file="/app/tests/Feature/OrderTest.php"
              line="14" assertions="2" time="0.135000"/>
    <testcase name="it_rejects_an_empty_cart" class="Tests\Feature\OrderTest"
              classname="Tests\Feature\OrderTest" file="/app/tests/Feature/OrderTest.php"
              line="27" assertions="1" time="0.021000">
      <failure type="PHPUnit\Framework\ExpectationFailedException">Failed asserting that 200 matches expected 422.</failure>
    </testcase>
    <testcase name="it_is_skipped" classname="Tests\Feature\OrderTest" time="0.000100">
      <skipped/>
    </testcase>
  </testsuite>
</testsuites>"#;
        let run = parse_junit(xml, &root());
        assert_eq!(run.cases.len(), 3);
        assert_eq!(run.passed(), 1);
        assert_eq!(run.failed(), 1);
        assert_eq!(run.skipped(), 1);
        assert_eq!(run.summary(), "1 passed · 1 failed · 1 skipped");

        let fail = run.problems().next().unwrap();
        assert_eq!(fail.name, "it_rejects_an_empty_cart");
        assert_eq!(fail.line, Some(27));
        assert_eq!(
            fail.file.as_deref(),
            Some(Path::new("/app/tests/Feature/OrderTest.php"))
        );
        assert_eq!(
            fail.message.as_deref(),
            Some("Failed asserting that 200 matches expected 422.")
        );
        assert_eq!(
            fail.full_name(),
            "Tests\\Feature\\OrderTest::it_rejects_an_empty_cart"
        );
    }

    #[test]
    fn the_shape_php_artisan_test_actually_emits() {
        // Captured from a real `php artisan test --log-junit` run. Two things
        // here broke the first version of this parser:
        //
        //   * `file` is a *description*, not a path — joining it to the project
        //     root produced something that opens nothing.
        //   * there is no `line` attribute at all; the only location PHPUnit
        //     gives for a failure is the `at …:7` tail of its message.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites>
  <testsuite name="Tests\Feature\DemoTest" file="Demo (Tests\Feature\Demo)" tests="3" assertions="2" errors="0" failures="1" skipped="1" time="1.192201">
    <testcase name="It passes" file="Demo (Tests\Feature\Demo)::It passes" class="Tests\Feature\DemoTest" classname="Tests.Feature.DemoTest" assertions="1" time="0.887590"/>
    <testcase name="It fails" file="Demo (Tests\Feature\Demo)::It fails" class="Tests\Feature\DemoTest" classname="Tests.Feature.DemoTest" assertions="1" time="0.199910">
      <failure type="PHPUnit\Framework\ExpectationFailedException">It failsFailed asserting that 200 is identical to 422.
at tests/Feature/DemoTest.php:7</failure>
    </testcase>
    <testcase name="It is skipped" file="Demo (Tests\Feature\Demo)::It is skipped" class="Tests\Feature\DemoTest" classname="Tests.Feature.DemoTest" assertions="0" time="0.104700">
      <skipped/>
    </testcase>
  </testsuite>
</testsuites>"#;
        let run = parse_junit(xml, &root());
        assert_eq!(run.summary(), "1 passed · 1 failed · 1 skipped");

        let passed = &run.cases[0];
        assert!(
            passed.file.is_none(),
            "a description must not be mistaken for a path: {:?}",
            passed.file
        );

        let failed = run.problems().next().unwrap();
        assert_eq!(
            failed.file.as_deref(),
            Some(Path::new("/app/tests/Feature/DemoTest.php")),
            "the location comes from the message, since there is no usable attribute"
        );
        assert_eq!(failed.line, Some(7));
        assert!(failed
            .message
            .as_deref()
            .unwrap()
            .contains("identical to 422"));
    }

    #[test]
    fn a_description_shaped_file_attribute_is_refused() {
        let xml = r#"<testsuites><testsuite name="s">
  <testcase name="t" classname="S" file="Demo (Tests\Feature\Demo)::It fails" time="0"/>
</testsuite></testsuites>"#;
        assert!(parse_junit(xml, &root()).cases[0].file.is_none());
    }

    #[test]
    fn an_error_is_distinguished_from_a_failure() {
        // PHPUnit separates them, and so should the panel: an assertion that
        // failed and a test that blew up want different attention.
        let xml = r#"<testsuites><testsuite name="s">
  <testcase name="boom" classname="S" time="0.1">
    <error type="Error">Call to a member function get() on null</error>
  </testcase>
</testsuite></testsuites>"#;
        let run = parse_junit(xml, &root());
        assert_eq!(run.cases[0].outcome, Outcome::Errored);
        assert!(run.cases[0].outcome.is_problem());
        assert_eq!(run.failed(), 1);
    }

    #[test]
    fn a_self_closing_failure_falls_back_to_its_message_attribute() {
        let xml = r#"<testsuites><testsuite name="s">
  <testcase name="t" classname="S" time="0"><failure message="nope" type="X"/></testcase>
</testsuite></testsuites>"#;
        let run = parse_junit(xml, &root());
        assert_eq!(run.cases[0].outcome, Outcome::Failed);
        assert_eq!(run.cases[0].message.as_deref(), Some("nope"));
    }

    #[test]
    fn entities_in_a_failure_message_are_decoded() {
        let xml = r#"<testsuites><testsuite name="s">
  <testcase name="t" classname="S" time="0">
    <failure>Failed asserting that &apos;&lt;p&gt;a &amp; b&lt;/p&gt;&apos; is empty.</failure>
  </testcase>
</testsuite></testsuites>"#;
        let run = parse_junit(xml, &root());
        assert_eq!(
            run.cases[0].message.as_deref(),
            Some("Failed asserting that '<p>a & b</p>' is empty.")
        );
    }

    #[test]
    fn a_relative_file_is_resolved_against_the_project() {
        let xml = r#"<testsuites><testsuite name="s">
  <testcase name="t" classname="S" file="tests/Unit/AThing.php" line="9" time="0"/>
</testsuite></testsuites>"#;
        let run = parse_junit(xml, &root());
        assert_eq!(
            run.cases[0].file.as_deref(),
            Some(Path::new("/app/tests/Unit/AThing.php")),
            "a relative path must land inside the project, not the cwd"
        );
    }

    #[test]
    fn a_case_without_a_file_still_parses() {
        // Pest's higher-order tests often carry no file attribute; dropping them
        // would silently shrink the count.
        let xml = r#"<testsuites><testsuite name="s">
  <testcase name="it works" classname="P\Tests" time="0.5"/>
</testsuite></testsuites>"#;
        let run = parse_junit(xml, &root());
        assert_eq!(run.cases.len(), 1);
        assert_eq!(run.cases[0].outcome, Outcome::Passed);
        assert!(run.cases[0].file.is_none());
        assert_eq!(run.cases[0].duration_s, 0.5);
    }

    #[test]
    fn nested_suites_are_flattened() {
        let xml = r#"<testsuites>
  <testsuite name="all">
    <testsuite name="Feature">
      <testcase name="a" classname="F" time="0"/>
    </testsuite>
    <testsuite name="Unit">
      <testcase name="b" classname="U" time="0"/>
      <testcase name="c" classname="U" time="0"><failure>x</failure></testcase>
    </testsuite>
  </testsuite>
</testsuites>"#;
        let run = parse_junit(xml, &root());
        assert_eq!(run.cases.len(), 3);
        assert_eq!(run.passed(), 2);
        assert_eq!(run.failed(), 1);
    }

    #[test]
    fn a_passing_case_after_a_failing_one_is_not_infected() {
        // The failure body must not leak into the next case — an off-by-one in
        // the scan would mark everything after the first failure as failed.
        let xml = r#"<testsuites><testsuite name="s">
  <testcase name="bad" classname="S" time="0"><failure>boom</failure></testcase>
  <testcase name="good" classname="S" time="0"/>
  <testcase name="alsogood" classname="S" time="0"/>
</testsuite></testsuites>"#;
        let run = parse_junit(xml, &root());
        assert_eq!(run.failed(), 1);
        assert_eq!(run.passed(), 2);
        assert_eq!(run.cases[1].outcome, Outcome::Passed);
        assert_eq!(run.cases[2].outcome, Outcome::Passed);
    }

    #[test]
    fn junk_yields_an_empty_run_rather_than_a_wrong_one() {
        assert!(parse_junit("", &root()).is_empty());
        assert!(parse_junit("PHP Fatal error: something", &root()).is_empty());
        assert!(parse_junit("<testsuites></testsuites>", &root()).is_empty());
    }

    #[test]
    fn the_summary_stays_quiet_about_what_did_not_happen() {
        let xml = r#"<testsuites><testsuite name="s">
  <testcase name="a" classname="S" time="0"/>
</testsuite></testsuites>"#;
        assert_eq!(parse_junit(xml, &root()).summary(), "1 passed");
    }

    #[test]
    fn junit_output_is_only_requested_from_runners_that_support_it() {
        let report = Path::new("/tmp/r.xml");
        assert_eq!(
            junit_command("php artisan test", report).as_deref(),
            Some("php artisan test --log-junit /tmp/r.xml")
        );
        assert!(junit_command("vendor/bin/pest", report).is_some());
        assert!(junit_command("vendor/bin/phpunit", report).is_some());
        // Passing an unknown runner a flag it doesn't take would break the run
        // that used to work.
        assert_eq!(junit_command("cargo test", report), None);
        assert_eq!(junit_command("go test ./...", report), None);
    }
}

/// Against a real runner. Opt-in, because it needs a project whose suite runs.
///
/// ```sh
/// E_TEST_PROJECT=/path/to/app cargo test -p e-app live_run -- --ignored --nocapture
/// ```
#[cfg(test)]
mod live_run {
    use super::*;

    fn report_path() -> PathBuf {
        std::env::temp_dir().join("e-junit-live.xml")
    }

    #[test]
    #[ignore]
    fn a_real_suite_produces_locatable_failures() {
        let Ok(root) = std::env::var("E_TEST_PROJECT") else {
            eprintln!("set E_TEST_PROJECT to a project whose suite runs");
            return;
        };
        let root = PathBuf::from(root);
        // Skip rather than panic: a missing or unrecognised project means the
        // harness wasn't pointed anywhere useful, not that the code is wrong.
        let Some(base) = crate::tasks::test_command(&root) else {
            eprintln!("no test command detected in {} — skipping", root.display());
            return;
        };
        let Some(cmd) = junit_command(&base, &report_path()) else {
            eprintln!("runner `{base}` cannot write JUnit — skipping");
            return;
        };
        let report = report_path();
        let _ = std::fs::remove_file(&report);

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let out = std::process::Command::new(shell)
            .arg("-lc")
            .arg(&cmd)
            .current_dir(&root)
            .output()
            .expect("run the suite");
        println!("exit {:?}", out.status.code());

        let xml = std::fs::read_to_string(&report).expect("the runner wrote a report");
        let run = parse_junit(&xml, &root);
        println!("{}", run.summary());
        for c in run.problems() {
            println!(
                "  {} -> {:?}:{:?}",
                c.full_name(),
                c.file.as_ref().map(|f| f.display().to_string()),
                c.line
            );
        }
        assert!(!run.is_empty(), "no cases parsed from a real run");
        // Every failure must be locatable, or the panel has nowhere to send you.
        for c in run.problems() {
            assert!(c.file.is_some(), "{} has no file", c.full_name());
            assert!(c.line.is_some(), "{} has no line", c.full_name());
            assert!(
                c.file.as_ref().unwrap().is_file(),
                "{} points at a path that does not exist: {:?}",
                c.full_name(),
                c.file
            );
        }
        let _ = std::fs::remove_file(&report);
    }
}
