//! Reconciliation against a real `double-entry` ledger.
//!
//! Types that match in isolation prove shape, not accounting. This suite posts
//! journal entries, derives [`LedgerSide`]s from the posted lines, and runs
//! the engine the way a real month-end close would: exact pass, window pass,
//! and the leftover is the report the operator actually reads — unmatched
//! lines are in transit, unmatched entries are unrecorded bank activity, and
//! each is a different follow-up.

#![allow(clippy::expect_used, clippy::panic)]

use double_entry::money::currencies;
use double_entry::{Account, AccountId, AccountType, JournalEntry, Ledger, Line};
use recon_kit::{
    Direction, LedgerSide, Proposal, Statement, StatementLine, match_exact, match_window,
};
use std::collections::HashSet;

fn chart() -> Ledger {
    let mut ledger = Ledger::new();
    for (id, name, kind) in [
        ("1000", "Current account", AccountType::Asset),
        ("4000", "Sales", AccountType::Revenue),
        ("5000", "Bank fees", AccountType::Expense),
        ("2000", "Trade payables", AccountType::Liability),
    ] {
        ledger.add_account(Account::new(id, name, kind));
    }
    ledger
}

/// Post the month's activity: two customer payments, one supplier payment,
/// one bank fee the books have not seen yet.
fn post_month(ledger: &mut Ledger) -> Vec<LedgerSide> {
    let entries = [
        (
            "JE-1",
            "2026-07-02",
            vec![
                Line::debit(
                    "1000",
                    double_entry::Amount::minor(120_000, currencies::GBP),
                ),
                Line::credit(
                    "4000",
                    double_entry::Amount::minor(120_000, currencies::GBP),
                ),
            ],
            Direction::Credit,
            120_000,
        ),
        (
            "JE-2",
            "2026-07-03",
            vec![
                Line::debit("2000", double_entry::Amount::minor(45_500, currencies::GBP)),
                Line::credit("1000", double_entry::Amount::minor(45_500, currencies::GBP)),
            ],
            Direction::Debit,
            45_500,
        ),
        (
            "JE-3",
            "2026-07-09",
            vec![
                Line::debit("1000", double_entry::Amount::minor(88_000, currencies::GBP)),
                Line::credit("4000", double_entry::Amount::minor(88_000, currencies::GBP)),
            ],
            Direction::Credit,
            88_000,
        ),
        (
            "JE-4",
            "2026-07-15",
            vec![
                Line::debit("5000", double_entry::Amount::minor(1_500, currencies::GBP)),
                Line::credit("1000", double_entry::Amount::minor(1_500, currencies::GBP)),
            ],
            Direction::Debit,
            1_500,
        ),
    ];
    let mut sides = Vec::new();
    let mut version = 0u64;
    for (id, date, lines, direction, amount) in entries {
        version = ledger
            .post(&JournalEntry::draft(id, date, "2026-07", lines), version)
            .expect("the month posts");
        let _ = version;
        // The bank side of each entry: the 1000 line is what the statement
        // should show.
        sides.push(LedgerSide {
            entry_id: id.into(),
            booked_on: date.into(),
            amount,
            direction,
        });
    }
    sides
}

fn statement_for_month() -> Statement {
    // The bank's view: everything above except the fee, plus a card debit that
    // has not been entered in the books (books lag reality), booked two days
    // off from the sales entry it settles.
    let lines = vec![
        StatementLine {
            id: "FIT-100".into(),
            booked_on: "2026-07-02".into(),
            amount: 120_000,
            direction: Direction::Credit,
            description: "ACME LTD INVOICE 2026-071".into(),
        },
        StatementLine {
            id: "FIT-101".into(),
            booked_on: "2026-07-04".into(),
            amount: 45_500,
            direction: Direction::Debit,
            description: "SUPPLIER BULK PAYMENT".into(),
        },
        StatementLine {
            id: "FIT-102".into(),
            booked_on: "2026-07-08".into(),
            amount: 88_000,
            direction: Direction::Credit,
            description: "GLOBEX PAYMENT".into(),
        },
        StatementLine {
            id: "FIT-103".into(),
            booked_on: "2026-07-16".into(),
            amount: 23_400,
            direction: Direction::Debit,
            description: "CARD 4421 CLOUD HOSTING".into(),
        },
    ];
    // Opening 50,000.00; the four lines land on 233,600.00 exactly.
    Statement {
        currency: "GBP".into(),
        opening_balance: 5_000_000,
        closing_balance: 5_000_000 + (120_000 + 88_000) - (45_500 + 23_400),
        lines,
    }
}

#[test]
fn the_statement_from_the_bank_validates_before_any_matching() {
    let statement = statement_for_month();
    assert_eq!(
        statement.validate(),
        Ok(()),
        "the fixture is arithmetically sound"
    );
}

#[test]
fn a_month_close_matches_exact_then_window_and_reports_the_leftovers() {
    let mut ledger = chart();
    let entries = post_month(&mut ledger);
    let statement = statement_for_month();

    // Pass 1: exact.
    let exact = match_exact(&statement.lines, &entries, &HashSet::new(), &HashSet::new());
    assert_eq!(exact.len(), 1, "only the same-day pair is exact");
    let exact_pair = exact.first().expect("length asserted");
    assert_eq!(exact_pair.line_id, "FIT-100");
    assert_eq!(exact_pair.entry_id, "JE-1");

    // Pass 2: window, excluding whatever pass 1 took.
    let taken_lines: HashSet<String> = exact.iter().map(|m| m.line_id.clone()).collect();
    let taken_entries: HashSet<String> = exact.iter().map(|m| m.entry_id.clone()).collect();
    let proposals = match_window(&statement.lines, &entries, 3, &taken_lines, &taken_entries);

    // FIT-101 booked 2026-07-04 vs JE-2's 07-03: one day off, clean proposal.
    // FIT-102 booked 07-08 vs JE-3's 07-09: one day off, clean proposal.
    // FIT-103 has no ledger entry at all: it is not a proposal, it is a gap.
    let matched: Vec<&Match> = proposals
        .iter()
        .filter_map(|p| match p {
            Proposal::Match(m) => Some(m),
            Proposal::Ambiguous { .. } => None,
        })
        .collect();
    assert_eq!(matched.len(), 2, "{proposals:?}");
    assert!(
        matched
            .iter()
            .any(|m| m.line_id == "FIT-101" && m.entry_id == "JE-2")
    );
    assert!(
        matched
            .iter()
            .any(|m| m.line_id == "FIT-102" && m.entry_id == "JE-3")
    );
    assert!(
        !proposals
            .iter()
            .any(|p| matches!(p, Proposal::Match(m) if m.line_id == "FIT-103")),
        "an unrecorded bank line must not be matched to a fabricated entry"
    );
}

#[test]
fn the_leftover_is_the_close_report_and_both_sides_mean_different_work() {
    let mut ledger = chart();
    let entries = post_month(&mut ledger);
    let statement = statement_for_month();

    let exact = match_exact(&statement.lines, &entries, &HashSet::new(), &HashSet::new());
    let taken_lines: HashSet<String> = exact.iter().map(|m| m.line_id.clone()).collect();
    let taken_entries: HashSet<String> = exact.iter().map(|m| m.entry_id.clone()).collect();
    let proposals = match_window(&statement.lines, &entries, 3, &taken_lines, &taken_entries);

    let matched_line_ids: HashSet<String> = exact
        .iter()
        .map(|m| m.line_id.clone())
        .chain(proposals.iter().filter_map(|p| match p {
            Proposal::Match(m) => Some(m.line_id.clone()),
            _ => None,
        }))
        .collect();
    let matched_entry_ids: HashSet<String> = exact
        .iter()
        .map(|m| m.entry_id.clone())
        .chain(proposals.iter().filter_map(|p| match p {
            Proposal::Match(m) => Some(m.entry_id.clone()),
            _ => None,
        }))
        .collect();

    // Unmatched statement lines = the bank knows money moved that the books
    // do not: enter it.
    let in_transit: Vec<&StatementLine> = statement
        .lines
        .iter()
        .filter(|l| !matched_line_ids.contains(&l.id))
        .collect();
    assert_eq!(in_transit.len(), 1);
    let stray_line = in_transit.first().expect("length asserted");
    assert_eq!(stray_line.id, "FIT-103");
    assert_eq!(
        stray_line.description, "CARD 4421 CLOUD HOSTING",
        "the close report names the line so the operator can act on it"
    );

    // Unmatched ledger entries = the books claim money moved that the bank
    // does not show: in transit, or wrong.
    let unrecorded: Vec<&LedgerSide> = entries
        .iter()
        .filter(|e| !matched_entry_ids.contains(&e.entry_id))
        .collect();
    // JE-4 is the fee the books recorded that the statement does not show:
    // the mirror image of FIT-103, and the other half of a real close report.
    assert_eq!(unrecorded.len(), 1);
    assert_eq!(
        unrecorded.first().expect("length asserted").entry_id,
        "JE-4"
    );

    // And the ledger itself balances as always: reconciliation reports, it
    // does not correct.
    assert_eq!(
        ledger.balance(&AccountId::new("1000")),
        120_000 - 45_500 + 88_000 - 1_500
    );
}

use recon_kit::Match;

#[test]
fn a_reimport_of_the_same_statement_matches_the_same_way() {
    let mut ledger = chart();
    let entries = post_month(&mut ledger);
    let statement = statement_for_month();

    let first = match_exact(&statement.lines, &entries, &HashSet::new(), &HashSet::new());
    // Idempotent re-import: the same statement, matched with the first run's
    // confirmations already taken, proposes nothing new.
    let taken_lines: HashSet<String> = first.iter().map(|m| m.line_id.clone()).collect();
    let taken_entries: HashSet<String> = first.iter().map(|m| m.entry_id.clone()).collect();
    let again = match_exact(&statement.lines, &entries, &taken_lines, &taken_entries);
    assert!(again.is_empty(), "confirmed work is never re-matched");
}
