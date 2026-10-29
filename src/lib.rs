//! Bank statement types and a matching engine for reconciling a statement
//! against a ledger.
//!
//! # What this crate deliberately does not do
//!
//! **No format parsing.** OFX, CAMT.053, and the forty incompatible CSV
//! dialects banks actually export each need their own adapter; a crate that
//! half-parses all of them is worse than one that models the result precisely.
//! An adapter parses its format, normalises it (OFX signs amounts — negative
//! means money out — while CAMT carries an explicit indicator; see
//! [`Direction::from_signed`]), and produces a [`Statement`]. Everything
//! downstream of the adapter is format-independent.
//!
//! **No silent auto-matching.** [`match_window`] returns *proposals*. Where two
//! ledger entries are equally plausible for one statement line, the answer is
//! [`Proposal::Ambiguous`] — surfaced for a human, never resolved by a coin
//! flip dressed up as a heuristic. A wrong auto-match is worse than an
//! unmatched line: it misattributes money in the books, and the reconciliation
//! still balances afterwards, so nothing downstream ever catches it.
//!
//! # Direction is a value, not a sign
//!
//! OFX signs the amount; CAMT.053 uses an explicit indicator. The lesson from
//! the invoice side of the ledger applies verbatim: mixing the two conventions
//! is how a refund posts as a charge, so a [`StatementLine`] carries an
//! unsigned amount plus a [`Direction`], and adapters normalise at the
//! boundary.
//!
//! # Tolerance is zero
//!
//! There is no "match within a penny" option, deliberately. A one-cent
//! tolerance hides statement-versus-ledger drift that compounds: the recon
//! report looks clean while the money is quietly wrong. Amounts are integer
//! minor units; an exact integer comparison is the only comparison.
//!
//! # Dates
//!
//! `YYYY-MM-DD` strings on the surface, but window matching needs real
//! calendar arithmetic, so day differences go through
//! [`days_from_civil`], an exact integer algorithm verified against known
//! dates including leap years — not against a float epoch difference.

mod matching;

pub use matching::{LedgerSide, Match, Proposal, match_exact, match_reference, match_window};

/// Which way the money moved, from the account holder's perspective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Money into the account: a customer payment, interest received.
    Credit,
    /// Money out of the account: a supplier bill paid, a fee charged.
    Debit,
}

impl Direction {
    /// The opposite direction.
    #[must_use]
    pub fn flip(self) -> Self {
        match self {
            Self::Credit => Self::Debit,
            Self::Debit => Self::Credit,
        }
    }

    /// Normalise a **signed** amount as OFX encodes it: positive is money in,
    /// negative is money out.
    ///
    /// The amount is returned unsigned, so the sign convention stops at this
    /// boundary and never reaches the matching engine or the ledger.
    #[must_use]
    pub fn from_signed(signed: i64) -> (Self, u64) {
        if signed < 0 {
            (Self::Debit, signed.unsigned_abs())
        } else {
            (Self::Credit, signed as u64)
        }
    }

    /// The signed form OFX uses, the inverse of [`Direction::from_signed`].
    #[must_use]
    pub fn signed(self, amount: u64) -> i64 {
        match self {
            Self::Credit => amount as i64,
            Self::Debit => -(amount as i64),
        }
    }
}

/// Why a statement cannot be trusted for reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconError {
    /// A statement line id appears twice.
    DuplicateLineId {
        /// The duplicated id (OFX `FITID`, CAMT account-servicer reference).
        id: String,
    },
    /// Opening balance plus the lines does not reach the closing balance.
    ///
    /// This is the check that catches a truncated import: a missing line breaks
    /// the arithmetic even when every line that did arrive looks individually
    /// plausible.
    BalanceMismatch {
        /// The balance the statement declares at its start, signed.
        opening: i64,
        /// The balance the arithmetic produces.
        computed: i64,
        /// The balance the statement declares at its end, signed.
        closing: i64,
    },
    /// A line has an empty id.
    EmptyLineId,
    /// A line amount is zero, which no real booking is.
    ZeroAmount {
        /// The offending line's id.
        id: String,
    },
}

/// One booking on a bank statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementLine {
    /// Statement-unique booking id (OFX `FITID`, CAMT account-servicer
    /// reference). Idempotent re-import keys off this.
    pub id: String,
    /// Booking date, `YYYY-MM-DD` — the date the bank treats as effective,
    /// which is what reconciliation matches on.
    pub booked_on: String,
    /// The amount in minor units, never negative and never zero.
    pub amount: u64,
    /// Which way the money moved.
    pub direction: Direction,
    /// The counterparty or narration as the bank rendered it.
    pub description: String,
}

/// A bank statement: the balance bounds and the lines between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// The currency as an ISO 4217 code. A statement mixing currencies is not
    /// one statement; the adapter's job to refuse it.
    pub currency: String,
    /// The declared opening balance, signed minor units.
    pub opening_balance: i64,
    /// The declared closing balance, signed minor units.
    pub closing_balance: i64,
    /// The lines, in booking order.
    pub lines: Vec<StatementLine>,
}

impl Statement {
    /// The rules that make a statement trustworthy as a reconciliation input.
    ///
    /// The balance identity is the load-bearing one:
    /// `opening + Σ(±lines) == closing`. A bank guarantees this, so a violation
    /// means the import lost or duplicated lines — exactly the failure that
    /// otherwise looks like fine individual rows.
    pub fn validate(&self) -> Result<(), ReconError> {
        let mut seen = std::collections::HashSet::new();
        let mut running = self.opening_balance;
        for line in &self.lines {
            if line.id.trim().is_empty() {
                return Err(ReconError::EmptyLineId);
            }
            if !seen.insert(&line.id) {
                return Err(ReconError::DuplicateLineId {
                    id: line.id.clone(),
                });
            }
            if line.amount == 0 {
                return Err(ReconError::ZeroAmount {
                    id: line.id.clone(),
                });
            }
            running += line.direction.signed(line.amount);
        }
        if running != self.closing_balance {
            return Err(ReconError::BalanceMismatch {
                opening: self.opening_balance,
                computed: running,
                closing: self.closing_balance,
            });
        }
        Ok(())
    }

    /// The signed running balance implied by the lines, which
    /// [`Statement::validate`] requires to equal the declared closing balance.
    #[must_use]
    pub fn computed_closing(&self) -> i64 {
        self.lines.iter().fold(self.opening_balance, |acc, line| {
            acc + line.direction.signed(line.amount)
        })
    }
}

/// Days between two `YYYY-MM-DD` dates (`b - a`), as an exact integer.
///
/// Howard Hinnant's `days_from_civil`: convert each date to a day count from
/// 1970-01-01 and subtract. No float epoch arithmetic, no month-length table
/// to get wrong in a leap year; the test suite pins known dates across
/// century boundaries.
#[must_use]
pub fn days_between(a: &str, b: &str) -> Option<i64> {
    Some(days_from_civil(parse_date(b)?) - days_from_civil(parse_date(a)?))
}

/// `(year, month, day)` from a `YYYY-MM-DD` string.
fn parse_date(s: &str) -> Option<(i64, u32, u32)> {
    let mut parts = s.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || parts.next().is_some() {
        return None;
    }
    Some((year, month, day))
}

/// Days from 1970-01-01 to the given civil date (Hinnant's `days_from_civil`).
#[must_use]
pub fn days_from_civil((y, m, d): (i64, u32, u32)) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn line(id: &str, on: &str, amount: u64, direction: Direction) -> StatementLine {
        StatementLine {
            id: id.into(),
            booked_on: on.into(),
            amount,
            direction,
            description: "test".into(),
        }
    }

    #[test]
    fn the_balance_identity_catches_a_truncated_import() {
        let statement = Statement {
            currency: "GBP".into(),
            opening_balance: 10_000,
            closing_balance: 15_000,
            lines: vec![line("L1", "2026-07-02", 7_000, Direction::Credit)],
        };
        // 100 + 70 = 170, not the declared 150: a line is missing, and no
        // individual row looks wrong.
        assert_eq!(
            statement.validate(),
            Err(ReconError::BalanceMismatch {
                opening: 10_000,
                computed: 17_000,
                closing: 15_000
            })
        );
    }

    #[test]
    fn a_consistent_statement_validates() {
        let statement = Statement {
            currency: "GBP".into(),
            opening_balance: 10_000,
            closing_balance: 16_250,
            lines: vec![
                line("L1", "2026-07-02", 7_000, Direction::Credit),
                line("L2", "2026-07-03", 750, Direction::Debit),
            ],
        };
        assert_eq!(statement.validate(), Ok(()));
        assert_eq!(statement.computed_closing(), 16_250);
    }

    #[test]
    fn line_ids_must_be_unique_and_nonempty() {
        let duplicated = Statement {
            currency: "GBP".into(),
            opening_balance: 0,
            closing_balance: 0,
            lines: vec![
                line("FIT1", "2026-07-02", 100, Direction::Credit),
                line("FIT1", "2026-07-02", 100, Direction::Credit),
            ],
        };
        assert!(matches!(
            duplicated.validate(),
            Err(ReconError::DuplicateLineId { .. })
        ));

        let blank = Statement {
            currency: "GBP".into(),
            opening_balance: 0,
            closing_balance: 0,
            lines: vec![line("  ", "2026-07-02", 100, Direction::Credit)],
        };
        assert_eq!(blank.validate(), Err(ReconError::EmptyLineId));
    }

    #[test]
    fn direction_normalises_the_ofx_sign_convention_at_the_boundary() {
        // OFX: positive is money in, negative is money out. Both round-trip.
        assert_eq!(Direction::from_signed(250), (Direction::Credit, 250));
        assert_eq!(Direction::from_signed(-250), (Direction::Debit, 250));
        assert_eq!(Direction::Credit.signed(250), 250);
        assert_eq!(Direction::Debit.signed(250), -250);
        assert_eq!(Direction::Credit.flip(), Direction::Debit);
    }

    #[test]
    fn zero_amounts_are_refused() {
        let statement = Statement {
            currency: "GBP".into(),
            opening_balance: 0,
            closing_balance: 0,
            lines: vec![line("L1", "2026-07-02", 0, Direction::Credit)],
        };
        assert!(matches!(
            statement.validate(),
            Err(ReconError::ZeroAmount { .. })
        ));
    }

    #[test]
    fn day_arithmetic_survives_leap_years_and_century_boundaries() {
        assert_eq!(days_between("2026-07-01", "2026-07-01"), Some(0));
        assert_eq!(days_between("2026-07-01", "2026-07-04"), Some(3));
        // 2024 is a leap year: February has 29 days.
        assert_eq!(days_between("2024-02-28", "2024-03-01"), Some(2));
        // 1900 is not a leap year (century, not divisible by 400); 2000 is.
        assert_eq!(days_between("1900-02-28", "1900-03-01"), Some(1));
        assert_eq!(days_between("2000-02-28", "2000-03-01"), Some(2));
        assert_eq!(days_between("1969-12-31", "1970-01-01"), Some(1));
        // Ordering is signed.
        assert_eq!(days_between("2026-07-04", "2026-07-01"), Some(-3));
        // Malformed input is None, not a panic.
        assert_eq!(days_between("2026-13-01", "2026-07-01"), None);
        assert_eq!(days_between("not-a-date", "2026-07-01"), None);
    }
}
