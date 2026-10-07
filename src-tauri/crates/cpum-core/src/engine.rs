//! Rule application engine: enumerate processes -> match rules -> apply
//! (affinity + priorities) -> report results.
//!
//! The GUI's "Apply rules" button and the Windows service's 5-second poll
//! both flow through this function, guaranteeing that the two paths
//! produce identical effects for the same rule set.

use std::path::Path;

use crate::matcher::rule_matches;
use crate::procwin::{self, ProcessPriorities};
use crate::rule::{self, AffinityRule};
use crate::store;

/// Result of applying a single process (the GUI uses this to emit an event
/// and patch the row in place).
#[derive(Debug, Clone)]
pub struct ProcessApplyInfo {
    pub pid: u32,
    /// Affinity mask after application, as a hex string.
    /// In soft mode this is the system mask (hard mask has been released);
    /// in strict mode it is the rule mask.
    pub mask_hex: Option<String>,
    /// Actual values read back when the rule manages priorities (used by
    /// the GUI event). None when no priorities are managed.
    pub priorities: Option<ProcessPriorities>,
}

/// Summary of one `apply_rules` invocation.
#[derive(Debug, Default)]
pub struct ApplyReport {
    /// Number of successful (process x rule) applications that actually wrote
    /// something.
    pub applied: u32,
    /// Number of (process x rule) matches that were **already** in the desired
    /// state and were therefore not written (audit item O4).
    pub skipped: u32,
    /// Number of failures (a single process failing does not abort the run).
    pub failed: u32,
    /// Per-process details of successful applications that wrote (used by the
    /// GUI event). Skipped processes are not listed - their rows already show
    /// the correct values.
    pub changed: Vec<ProcessApplyInfo>,
}

/// Apply a list of rules to all currently-running processes, skipping the
/// writes that would not change anything.
///
/// - Pre-validation: if any enabled rule is invalid (mask cannot be parsed
///   or priority is out of range), this function returns an error directly
///   so the GUI can surface it to the user (matches the v1 behavior; the
///   service's polling ignores the error and continues).
/// - A failure applying a single process is only counted; it does not
///   affect other processes.
pub fn apply_rules(rules: &[AffinityRule]) -> Result<ApplyReport, String> {
    apply_rules_with_options(rules, false)
}

/// [`apply_rules`] with an explicit `force` switch.
///
/// `force = true` writes affinity and priorities even when the process already
/// matches the rule. It exists as a debugging escape hatch
/// (`cpum_service --apply-once --force`): if idempotent application is ever
/// suspected of *not* applying a rule, forcing it rules the skip logic out in
/// one command instead of one rebuild.
pub fn apply_rules_with_options(
    rules: &[AffinityRule],
    force: bool,
) -> Result<ApplyReport, String> {
    let active: Vec<&AffinityRule> = rules.iter().filter(|r| r.enabled).collect();
    if active.is_empty() {
        return Ok(ApplyReport::default());
    }
    for r in &active {
        rule::validate_rule(r)?;
    }

    // Only resolve full paths when a Path-type rule exists, to avoid
    // opening every process for nothing.
    let need_paths = active.iter().any(|r| r.match_type == rule::MatchType::Path);
    let processes = procwin::enumerate_processes(need_paths)?;

    let mut report = ApplyReport::default();
    for r in &active {
        let masks = r
            .group_masks
            .as_ref()
            .map(|values| {
                procwin::GroupMasks::from_hex_list(values)
                    .expect("validate_rule guarantees valid masks")
                    .0
            })
            .unwrap_or_else(|| {
                vec![procwin::parse_hex_mask(&r.mask)
                    .expect("validate_rule guarantees a valid mask")]
            });
        for entry in &processes {
            if !rule_matches(r, &entry.name, entry.path.as_deref()) {
                continue;
            }
            match apply_one(r, entry.pid, &masks, force) {
                Ok(outcome) => {
                    if outcome.skipped {
                        report.skipped += 1;
                    } else {
                        report.applied += 1;
                        report.changed.push(outcome.info);
                    }
                }
                Err(e) => {
                    report.failed += 1;
                    eprintln!(
                        "failed to apply rule {} to PID {}: {}",
                        r.process_name, entry.pid, e
                    );
                }
            }
        }
    }
    Ok(report)
}

/// Load rules from the given directory and apply them immediately (used
/// by the service's polling loop and the `--apply-once` test mode).
pub fn apply_rules_from_dir(base_dir: &Path) -> Result<ApplyReport, String> {
    apply_rules_from_dir_with_options(base_dir, false)
}

/// [`apply_rules_from_dir`] with the `force` switch of
/// [`apply_rules_with_options`].
pub fn apply_rules_from_dir_with_options(
    base_dir: &Path,
    force: bool,
) -> Result<ApplyReport, String> {
    let rules = store::load_rules(base_dir)?;
    apply_rules_with_options(&rules, force)
}

/// Outcome of [`apply_one`]: the state to report, plus whether anything was
/// actually written.
struct ApplyOutcome {
    info: ProcessApplyInfo,
    /// `true` when the process already matched the rule and no write happened.
    skipped: bool,
}

/// Whether the process's current priorities already satisfy every priority the
/// rule manages.
///
/// Fields the OS would not tell us (`None`) never match: a process we cannot
/// observe must be written, not skipped. Comparison is by value - the CPU
/// priority class constants are bit flags, so ordering helpers such as
/// `priority_class_rank` must not be used for equality.
pub fn priorities_match(current: ProcessPriorities, rule: &AffinityRule) -> bool {
    let matches = |want: Option<u32>, have: Option<u32>| want.is_none_or(|w| have == Some(w));
    matches(rule.priority_class, current.priority_class)
        && matches(rule.io_priority, current.io_priority)
        && matches(rule.memory_priority, current.memory_priority)
}

fn apply_one(
    rule: &AffinityRule,
    pid: u32,
    masks: &[u64],
    force: bool,
) -> Result<ApplyOutcome, String> {
    // 1. Affinity / CPU Sets (soft mode automatically falls back to the
    //    hard mask when the system doesn't support CPU Sets).
    //
    //    Skipped when the process is already pinned as the rule wants: every
    //    write makes the kernel re-evaluate thread placement for all of the
    //    process's threads. `affinity_matches` returns false when it cannot
    //    read the current state, so uncertainty always means "write".
    let affinity_already_correct = !force && procwin::affinity_matches(pid, masks, rule.mode);
    let soft_applied = if affinity_already_correct {
        // No write. `soft_applied` only decides how the mask is reported back:
        // in soft mode the hard mask is the system mask.
        matches!(rule.mode, crate::rule::RuleMode::Soft) && procwin::cpu_sets_available()
    } else {
        procwin::set_affinity_by_group_masks(pid, masks, rule.mode)?
    };

    // 2. The three priority classes (each independent; any one failing
    //    counts this process as a failure). Read once, compare, and skip the
    //    writes when they would be a no-op.
    let manages_priorities = rule.manages_priorities();
    let current_priorities = if manages_priorities {
        Some(procwin::get_process_priorities(pid))
    } else {
        None
    };
    let priorities_already_correct =
        !force && current_priorities.is_some_and(|current| priorities_match(current, rule));
    if manages_priorities && !priorities_already_correct {
        if let Some(pc) = rule.priority_class {
            procwin::set_process_priority_class(pid, pc)?;
        }
        if let Some(io) = rule.io_priority {
            procwin::set_process_io_priority(pid, io)?;
        }
        if let Some(mp) = rule.memory_priority {
            procwin::set_process_memory_priority(pid, mp)?;
        }
    }

    // 3. Read back the actual state (fed to the GUI event; in soft mode
    //    the hard mask == system mask).
    let mask_hex = if soft_applied {
        procwin::get_process_affinity(pid)
            .ok()
            .and_then(|(pm, _)| pm)
            .map(procwin::mask_to_hex)
    } else {
        Some(procwin::mask_to_hex(
            masks.first().copied().unwrap_or_default(),
        ))
    };
    // `current_priorities` was read *before* the writes, so re-read when we
    // wrote something - otherwise report the values we already have.
    let priorities = if manages_priorities && !priorities_already_correct {
        Some(procwin::get_process_priorities(pid))
    } else {
        current_priorities
    };

    Ok(ApplyOutcome {
        info: ProcessApplyInfo {
            pid,
            mask_hex,
            priorities,
        },
        skipped: affinity_already_correct && (!manages_priorities || priorities_already_correct),
    })
}

#[cfg(test)]
mod apply_decision_tests {
    use super::priorities_match;
    use crate::procwin::ProcessPriorities;
    use crate::rule::{AffinityRule, MatchType, RuleMode};

    fn rule_with(
        priority_class: Option<u32>,
        io_priority: Option<u32>,
        memory_priority: Option<u32>,
    ) -> AffinityRule {
        AffinityRule {
            id: "test".into(),
            process_name: "test.exe".into(),
            match_type: MatchType::Exact,
            mask: "0xF".into(),
            group_masks: None,
            enabled: true,
            created_at: 0,
            note: String::new(),
            mode: RuleMode::Strict,
            priority_class,
            io_priority,
            memory_priority,
        }
    }

    const NORMAL: u32 = 0x20;

    #[test]
    fn matches_when_every_managed_priority_agrees() {
        let rule = rule_with(Some(NORMAL), Some(2), Some(5));
        let current = ProcessPriorities {
            priority_class: Some(NORMAL),
            io_priority: Some(2),
            memory_priority: Some(5),
        };
        assert!(priorities_match(current, &rule));
    }

    #[test]
    fn ignores_fields_the_rule_does_not_manage() {
        // A rule that only manages IO priority must not care that the CPU class
        // is "wrong" - otherwise every unrelated ProBalance downgrade would
        // force a write every tick.
        let rule = rule_with(None, Some(2), None);
        let current = ProcessPriorities {
            priority_class: Some(NORMAL),
            io_priority: Some(2),
            memory_priority: Some(1),
        };
        assert!(priorities_match(current, &rule));
    }

    #[test]
    fn does_not_match_when_a_value_is_unreadable() {
        // Uncertainty must never become "skip".
        let rule = rule_with(Some(NORMAL), None, None);
        let current = ProcessPriorities {
            priority_class: None,
            io_priority: None,
            memory_priority: None,
        };
        assert!(!priorities_match(current, &rule));
    }

    #[test]
    fn manages_nothing_means_already_correct() {
        let rule = rule_with(None, None, None);
        assert!(priorities_match(ProcessPriorities::default(), &rule));
    }
}
