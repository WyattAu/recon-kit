//! The matching engine: statements against ledger entries.
//!
//! Two passes, in order of trust:
//!
//! 1. [`match_exact`] — amount, direction, and booking date all equal. Not a
//!    proposal; this *is* the match, the way two copies of the same fact
//!    match.
//! 2. [`match_window`] — amount and direction equal, dates within a window
//!    (the card transaction that clears two days after it posts). This returns
//!    [`Proposal`]s, and where more than one entry is equally plausible for a
//!    line, [`Proposal::Ambiguous`] — the case a silent heuristic would
//!    resolve by picking first, which is how money gets misattributed in the
//!    books while the reconciliation still balances.
//!
//! Both passes are **injective**: a line is matched to at most one entry and
//! an entry absorbs at most one line. The functions filter out anything
//! already claimed, so running exact-then-window cannot double-take.
//!
//! One-to-one only, by design: a bank line that nets three ledger entries
//! (fees batched, split deposits) needs a grouping rule that belongs to the
//! operator, not a default that guesses. The crate's error is on the side of
//! "unmatched, awaiting review" — never "matched, possibly wrongly".

use crate::{Direction, StatementLine, days_between};

/// The ledger's view of one booking, as reconciliation sees it.
///
/// The caller derives these from journal lines; a `double-entry` amount
/// already carries direction as a side, so nothing here re-derives it from a
/// sign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerSide {
    /// The journal entry's id, so a confirmed match can be recorded.
    pub entry_id: String,
    /// The entry's effective date, `YYYY-MM-DD`.
    pub booked_on: String,
    /// The amount in minor units, never negative.
    pub amount: u64,
    /// Which way the money moved, from the account holder's perspective.
    pub direction: Direction,
}

/// A confirmed match: one statement line, one ledger entry, same money.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// The statement line's id.
    pub line_id: String,
    /// The journal entry's id.
    pub entry_id: String,
}

/// What window matching concluded about one unmatched line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proposal {
    /// Exactly one plausible entry within the window: a proposal to confirm.
    Match(Match),
    /// More than one entry is equally plausible. Surfaced, not resolved —
    /// the operator knows which invoice the money was actually for.
    Ambiguous {
        /// The statement line's id.
        line_id: String,
        /// Every entry within the window that the line could be, closest to
        /// the line's date first, ties by entry id.
        candidates: Vec<String>,
    },
}

/// Amount and direction equal, date equal: not a guess.
///
/// Lines and entries already claimed by `taken_line_ids` / `taken_entry_ids`
/// are skipped, so this can be re-run after the operator confirms a batch
/// without re-matching confirmed work. Idempotent by construction.
pub fn match_exact(
    lines: &[StatementLine],
    entries: &[LedgerSide],
    taken_line_ids: &std::collections::HashSet<String>,
    taken_entry_ids: &std::collections::HashSet<String>,
) -> Vec<Match> {
    let mut matches = Vec::new();
    let mut claimed_entries: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for line in lines {
        if taken_line_ids.contains(&line.id) {
            continue;
        }
        for entry in entries {
            if taken_entry_ids.contains(&entry.entry_id)
                || claimed_entries.contains(entry.entry_id.as_str())
            {
                continue;
            }
            if line.amount == entry.amount
                && line.direction == entry.direction
                && line.booked_on == entry.booked_on
            {
                claimed_entries.insert(entry.entry_id.as_str());
                matches.push(Match {
                    line_id: line.id.clone(),
                    entry_id: entry.entry_id.clone(),
                });
                break;
            }
        }
    }
    matches
}

/// Amount and direction equal, dates within `window_days`: a proposal.
///
/// The window exists because clearing is not instant — a card transaction
/// posts on Friday and books on Sunday. A wider window produces more
/// ambiguity, not more matches: ambiguity is the honest output when two
/// entries are equally plausible.
pub fn match_window(
    lines: &[StatementLine],
    entries: &[LedgerSide],
    window_days: i64,
    taken_line_ids: &std::collections::HashSet<String>,
    taken_entry_ids: &std::collections::HashSet<String>,
) -> Vec<Proposal> {
    let mut proposals = Vec::new();
    let mut claimed_entries: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for line in lines {
        if taken_line_ids.contains(&line.id) {
            continue;
        }
        let mut candidates: Vec<(&LedgerSide, i64)> = Vec::new();
        for entry in entries {
            if taken_entry_ids.contains(&entry.entry_id)
                || claimed_entries.contains(entry.entry_id.as_str())
            {
                continue;
            }
            if line.amount != entry.amount || line.direction != entry.direction {
                continue;
            }
            if let Some(diff) = days_between(&entry.booked_on, &line.booked_on) {
                if diff.abs() <= window_days {
                    candidates.push((entry, diff));
                }
            }
        }
        match candidates.as_slice() {
            [] => {}
            [(entry, _)] => {
                claimed_entries.insert(entry.entry_id.as_str());
                proposals.push(Proposal::Match(Match {
                    line_id: line.id.clone(),
                    entry_id: entry.entry_id.clone(),
                }));
            }
            _ => {
                // Closest to the line's date first, ties by entry id: a
                // deterministic order, so the same situation always presents
                // the same list.
                candidates.sort_by(|a, b| {
                    a.1.abs()
                        .cmp(&b.1.abs())
                        .then(a.0.entry_id.cmp(&b.0.entry_id))
                });
                proposals.push(Proposal::Ambiguous {
                    line_id: line.id.clone(),
                    candidates: candidates
                        .into_iter()
                        .map(|(e, _)| e.entry_id.clone())
                        .collect(),
                });
            }
        }
    }
    proposals
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::StatementLine;
    use std::collections::HashSet;

    fn line(id: &str, on: &str, amount: u64, direction: Direction) -> StatementLine {
        StatementLine {
            id: id.into(),
            booked_on: on.into(),
            amount,
            direction,
            description: "test".into(),
        }
    }

    fn entry(id: &str, on: &str, amount: u64, direction: Direction) -> LedgerSide {
        LedgerSide {
            entry_id: id.into(),
            booked_on: on.into(),
            amount,
            direction,
        }
    }

    fn empty() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn exact_matching_requires_every_field() {
        let lines = vec![
            line("S1", "2026-07-02", 5_000, Direction::Credit),
            line("S2", "2026-07-02", 5_000, Direction::Debit),
            line("S3", "2026-07-03", 5_000, Direction::Credit),
        ];
        let entries = vec![entry("JE-1", "2026-07-02", 5_000, Direction::Credit)];
        let matches = match_exact(&lines, &entries, &empty(), &empty());
        assert_eq!(matches.len(), 1, "direction and date must both gate");
        let the_match = matches.first().expect("length asserted");
        assert_eq!(the_match.line_id, "S1");
        assert_eq!(the_match.entry_id, "JE-1");
    }

    #[test]
    fn an_entry_is_never_taken_twice_across_passes() {
        let lines = vec![
            line("S1", "2026-07-02", 5_000, Direction::Credit),
            line("S2", "2026-07-02", 5_000, Direction::Credit),
        ];
        let entries = vec![entry("JE-1", "2026-07-02", 5_000, Direction::Credit)];
        // Exact pass takes JE-1 for S1.
        let exact = match_exact(&lines, &entries, &empty(), &empty());
        assert_eq!(exact.len(), 1);
        // The window pass must not re-take it for S2, which is equally close.
        let taken_entries: HashSet<String> = exact.iter().map(|m| m.entry_id.clone()).collect();
        let taken_lines: HashSet<String> = exact.iter().map(|m| m.line_id.clone()).collect();
        let proposals = match_window(&lines, &entries, 3, &taken_lines, &taken_entries);
        assert!(
            proposals.is_empty(),
            "a claimed entry must not match again: {proposals:?}"
        );
    }

    #[test]
    fn one_candidate_in_the_window_is_a_match_two_is_ambiguity() {
        let lines = vec![
            line("SOLO", "2026-07-04", 9_999, Direction::Credit),
            line("TWIN", "2026-07-04", 4_000, Direction::Debit),
        ];
        let entries = vec![
            entry("JE-A", "2026-07-03", 9_999, Direction::Credit),
            entry("JE-1", "2026-07-02", 4_000, Direction::Debit),
            entry("JE-2", "2026-07-06", 4_000, Direction::Debit),
        ];
        let proposals = match_window(&lines, &entries, 3, &empty(), &empty());
        assert_eq!(proposals.len(), 2);
        assert_eq!(
            proposals.first().expect("length asserted"),
            &Proposal::Match(Match {
                line_id: "SOLO".into(),
                entry_id: "JE-A".into()
            })
        );
        assert_eq!(
            proposals.get(1).expect("length asserted"),
            &Proposal::Ambiguous {
                line_id: "TWIN".into(),
                candidates: vec!["JE-1".into(), "JE-2".into()],
            },
            "two equally plausible entries are surfaced, never picked"
        );
    }

    #[test]
    fn a_wider_window_creates_ambiguity_not_more_confidence() {
        let lines = vec![line("S1", "2026-07-10", 700, Direction::Debit)];
        let entries = vec![
            entry("JE-A", "2026-07-09", 700, Direction::Debit),
            entry("JE-B", "2026-07-12", 700, Direction::Debit),
        ];
        // Tight window: JE-A is one day off, JE-B two — only JE-A is in range.
        let tight = match_window(&lines, &entries, 1, &empty(), &empty());
        assert_eq!(
            tight,
            vec![Proposal::Match(Match {
                line_id: "S1".into(),
                entry_id: "JE-A".into()
            })]
        );
        // Wider window: both are in range, so the answer is ambiguity. A
        // symmetric window cannot separate entries equidistant on either side.
        let wide = match_window(&lines, &entries, 2, &empty(), &empty());
        assert!(matches!(
            wide.first().expect("length asserted"),
            Proposal::Ambiguous { candidates, .. } if candidates.len() == 2
        ));
    }

    #[test]
    fn direction_mismatch_never_matches_even_in_a_wide_window() {
        let lines = vec![line("S1", "2026-07-02", 5_000, Direction::Credit)];
        let entries = vec![entry("JE-1", "2026-07-02", 5_000, Direction::Debit)];
        assert!(match_window(&lines, &entries, 30, &empty(), &empty()).is_empty());
    }

    #[test]
    fn re_running_with_taken_sets_is_idempotent() {
        let lines = vec![line("S1", "2026-07-02", 5_000, Direction::Credit)];
        let entries = vec![entry("JE-1", "2026-07-02", 5_000, Direction::Credit)];
        let first = match_exact(&lines, &entries, &empty(), &empty());
        let taken_lines: HashSet<String> = first.iter().map(|m| m.line_id.clone()).collect();
        let taken_entries: HashSet<String> = first.iter().map(|m| m.entry_id.clone()).collect();
        let second = match_exact(&lines, &entries, &taken_lines, &taken_entries);
        assert!(second.is_empty(), "confirmed work is not re-matched");
    }
}
