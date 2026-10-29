//! The matching engine: statements against ledger entries.

use crate::{Direction, StatementLine, days_between};

/// The ledger's view of one booking, as reconciliation sees it.
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
    /// The journal line's memo or narration, for reference-substring
    /// proposals.
    pub memo: String,
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

/// Reference-substring proposal: a statement line's memo matches an entry's
/// memo or narration by substring containment, with amount and direction
/// also equal. The date is NOT required to match — references survive
/// clearing delays of any length.
///
/// At most one proposal per line: if two entries' memos both contain the
/// line's reference, the result is ambiguous, not a coin flip. Only fires
/// when the reference is non-trivial (at least 4 characters), because a
/// one-character substring match is noise, not a proposal.
pub fn match_reference(
    lines: &[StatementLine],
    entries: &[LedgerSide],
    taken_line_ids: &std::collections::HashSet<String>,
    taken_entry_ids: &std::collections::HashSet<String>,
) -> Vec<Proposal> {
    const MIN_REFERENCE_LEN: usize = 4;
    let mut proposals = Vec::new();
    let mut claimed_entries: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for line in lines {
        if taken_line_ids.contains(&line.id) {
            continue;
        }
        let reference = &line.description;
        if reference.len() < MIN_REFERENCE_LEN {
            continue;
        }
        let candidates: Vec<&LedgerSide> = entries
            .iter()
            .filter(|e| {
                !taken_entry_ids.contains(&e.entry_id)
                    && !claimed_entries.contains(e.entry_id.as_str())
            })
            .filter(|e| e.amount == line.amount && e.direction == line.direction)
            .filter(|e| e.memo.contains(reference.as_str()) || reference.contains(e.memo.as_str()))
            .collect();

        match candidates.len() {
            0 => {}
            1 => {
                if let Some(entry) = candidates.first() {
                    claimed_entries.insert(entry.entry_id.as_str());
                    proposals.push(Proposal::Match(Match {
                        line_id: line.id.clone(),
                        entry_id: entry.entry_id.clone(),
                    }));
                }
            }
            _ => {
                proposals.push(Proposal::Ambiguous {
                    line_id: line.id.clone(),
                    candidates: candidates.iter().map(|e| e.entry_id.clone()).collect(),
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
            memo: String::new(),
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
        let m = matches.first().expect("length asserted");
        assert_eq!(m.line_id, "S1");
        assert_eq!(m.entry_id, "JE-1");
    }

    #[test]
    fn an_entry_is_never_taken_twice_across_passes() {
        let lines = vec![
            line("S1", "2026-07-02", 5_000, Direction::Credit),
            line("S2", "2026-07-02", 5_000, Direction::Credit),
        ];
        let entries = vec![entry("JE-1", "2026-07-02", 5_000, Direction::Credit)];
        let exact = match_exact(&lines, &entries, &empty(), &empty());
        assert_eq!(exact.len(), 1);
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
        let tight = match_window(&lines, &entries, 1, &empty(), &empty());
        assert_eq!(
            tight.first().expect("length asserted"),
            &Proposal::Match(Match {
                line_id: "S1".into(),
                entry_id: "JE-A".into()
            })
        );
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

    #[test]
    fn reference_matching_proposes_on_memo_containment() {
        let lines = vec![StatementLine {
            id: "FIT-R1".into(),
            booked_on: "2026-07-04".into(),
            amount: 55_000,
            direction: Direction::Credit,
            description: "BANK TRANSFER re PAYMENT INV-071".into(),
        }];
        let entries = vec![LedgerSide {
            entry_id: "JE-INV071".into(),
            booked_on: "2026-07-10".into(),
            amount: 55_000,
            direction: Direction::Credit,
            memo: "BANK TRANSFER re PAYMENT INV-071".into(),
        }];
        let empty = HashSet::new();
        let proposals = match_reference(&lines, &entries, &empty, &empty);
        assert_eq!(proposals.len(), 1, "one reference-substring proposal");
    }

    #[test]
    fn a_short_reference_is_noise_not_a_proposal() {
        let lines = vec![StatementLine {
            id: "FIT-S1".into(),
            booked_on: "2026-07-02".into(),
            amount: 1_000,
            direction: Direction::Credit,
            description: "x".into(),
        }];
        let entries = vec![LedgerSide {
            entry_id: "JE-X".into(),
            booked_on: "2026-07-02".into(),
            amount: 1_000,
            direction: Direction::Credit,
            memo: "contains x".into(),
        }];
        let empty = HashSet::new();
        let proposals = match_reference(&lines, &entries, &empty, &empty);
        assert!(
            proposals.is_empty(),
            "one character is noise, not a reference"
        );
    }
}
