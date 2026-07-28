//! The ship gate: is this changeset ready to commit?
//!
//! Pure advice, not enforcement — it collects the signals you'd otherwise carry
//! in your head (did I review everything? do the tests pass? any danger flags?)
//! and turns them into a verdict the panel can show.

/// Whether the project's test suite passed for the current tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestStatus {
    /// Not run yet for this changeset.
    Unknown,
    Running,
    Passing,
    Failing,
}

/// The inputs to the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShipCheck {
    /// `(reviewed, total)` files.
    pub reviewed: (usize, usize),
    pub danger_flags: usize,
    pub warn_flags: usize,
    pub tests: TestStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Something is actively wrong — shipping needs a deliberate override.
    Blocked,
    /// Shippable, but with loose ends.
    Warn,
    /// Reviewed, green, and clean.
    Ready,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShipVerdict {
    pub readiness: Readiness,
    /// Human reasons, most important first — shown as a checklist.
    pub reasons: Vec<String>,
}

/// Judge a changeset's readiness to ship.
pub fn ship_verdict(c: &ShipCheck) -> ShipVerdict {
    let (done, total) = c.reviewed;
    let mut blocking = Vec::new();
    let mut warnings = Vec::new();

    if total == 0 {
        return ShipVerdict {
            readiness: Readiness::Warn,
            reasons: vec!["Nothing to ship".to_string()],
        };
    }

    match c.tests {
        TestStatus::Failing => blocking.push("Tests are failing".to_string()),
        TestStatus::Unknown => warnings.push("Tests have not been run".to_string()),
        TestStatus::Running => warnings.push("Tests are still running".to_string()),
        TestStatus::Passing => {}
    }
    if c.danger_flags > 0 {
        blocking.push(format!(
            "{} danger flag{}",
            c.danger_flags,
            if c.danger_flags == 1 { "" } else { "s" }
        ));
    }
    if done < total {
        warnings.push(format!("{done} of {total} files reviewed"));
    }
    if c.warn_flags > 0 {
        warnings.push(format!(
            "{} warning{}",
            c.warn_flags,
            if c.warn_flags == 1 { "" } else { "s" }
        ));
    }

    if !blocking.is_empty() {
        blocking.extend(warnings);
        return ShipVerdict {
            readiness: Readiness::Blocked,
            reasons: blocking,
        };
    }
    if !warnings.is_empty() {
        return ShipVerdict {
            readiness: Readiness::Warn,
            reasons: warnings,
        };
    }
    ShipVerdict {
        readiness: Readiness::Ready,
        reasons: vec![format!("{total} files reviewed · tests green · no flags")],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(reviewed: (usize, usize), danger: usize, warn: usize, tests: TestStatus) -> ShipCheck {
        ShipCheck {
            reviewed,
            danger_flags: danger,
            warn_flags: warn,
            tests,
        }
    }

    #[test]
    fn ready_when_reviewed_green_and_clean() {
        let v = ship_verdict(&check((5, 5), 0, 0, TestStatus::Passing));
        assert_eq!(v.readiness, Readiness::Ready);
        assert!(v.reasons[0].contains("tests green"));
    }

    #[test]
    fn failing_tests_block() {
        let v = ship_verdict(&check((5, 5), 0, 0, TestStatus::Failing));
        assert_eq!(v.readiness, Readiness::Blocked);
        assert_eq!(v.reasons[0], "Tests are failing");
    }

    #[test]
    fn danger_flags_block_and_keep_warnings() {
        let v = ship_verdict(&check((3, 5), 2, 1, TestStatus::Passing));
        assert_eq!(v.readiness, Readiness::Blocked);
        assert_eq!(v.reasons[0], "2 danger flags");
        assert!(v.reasons.iter().any(|r| r == "3 of 5 files reviewed"));
        assert!(v.reasons.iter().any(|r| r == "1 warning"));
    }

    #[test]
    fn unreviewed_or_unrun_tests_only_warn() {
        let v = ship_verdict(&check((2, 5), 0, 0, TestStatus::Unknown));
        assert_eq!(v.readiness, Readiness::Warn);
        assert_eq!(v.reasons[0], "Tests have not been run");
        assert!(v.reasons.iter().any(|r| r == "2 of 5 files reviewed"));
    }

    #[test]
    fn empty_changeset_is_nothing_to_ship() {
        let v = ship_verdict(&check((0, 0), 0, 0, TestStatus::Passing));
        assert_eq!(v.readiness, Readiness::Warn);
        assert_eq!(v.reasons, vec!["Nothing to ship".to_string()]);
    }
}
