# recon-kit

Bank statement types and a matching engine for reconciling a statement against
a ledger. No format parsing, no floats, no silent auto-matching.

## What is deliberately absent

**Format parsing.** OFX, CAMT.053, and the forty incompatible CSV dialects
banks actually export each need their own adapter; a crate that half-parses all
of them is worse than one that models the result precisely. An adapter parses
its format, normalises it, and produces a `Statement`. Everything downstream is
format-independent.

**Silent auto-matching.** `match_window` returns *proposals*. Where two ledger
entries are equally plausible for one statement line, the answer is
`Proposal::Ambiguous`; surfaced for a human, never resolved by a coin flip
dressed up as a heuristic. A wrong auto-match is worse than an unmatched line:
it misattributes money in the books, and the reconciliation still balances
afterwards, so nothing downstream ever catches it.

**Tolerance.** There is no "match within a penny" option. A one-cent tolerance
hides statement-versus-ledger drift that compounds: the recon report looks
clean while the money is quietly wrong. Amounts are integer minor units; an
exact integer comparison is the only comparison.

## The two load-bearing decisions

**Direction is a value, not a sign.** OFX signs the amount (negative = money
out); CAMT.053 carries an explicit indicator. Mixing the conventions is how a
refund posts as a charge, so `StatementLine` carries an unsigned amount plus a
`Direction`, and adapters normalise at the boundary
(`Direction::from_signed`).

**The balance identity is the import check.** `opening + Σ(±lines) == closing`
is guaranteed by the bank, so a violation means the import lost or duplicated
lines; exactly the failure that otherwise looks like fine individual rows.
`Statement::validate` refuses such an import rather than reconciling a
statement that is already wrong.

## The engine

1. `match_exact`; amount, direction, and booking date all equal. Not a guess.
2. `match_window`; amount and direction equal, dates within a window (the card
   transaction that clears two days after it posts). Returns `Proposal::Match`
   when exactly one entry is plausible, `Proposal::Ambiguous` when more than
   one is. A wider window produces more ambiguity, not more confidence.

Both passes are injective; a line matches at most one entry, an entry absorbs
at most one line; and both take the set of already-confirmed ids, so re-running
after a confirm batch is idempotent.

One-to-one only, by design: a bank line that nets three ledger entries (fees
batched, split deposits) needs a grouping rule that belongs to the operator,
not a default that guesses. The engine's error is on the side of "unmatched,
awaiting review"; never "matched, possibly wrongly".

## Dates

`YYYY-MM-DD` strings on the surface; day differences go through
`days_from_civil` (Howard Hinnant's exact integer algorithm), verified against
known dates across leap years and century boundaries; not a float epoch
difference.

## Licence

MIT OR Apache-2.0.
