# Changelog

All notable changes to this project are documented here. Format: [Keep a
Changelog](https://keepachangelog.com/); versions follow [semver](https://semver.org).

## [Unreleased]

## [0.2.0] - 2026-10-28

### Added

- `match_reference`; reference-substring proposal matching: a statement
  line's memo and an entry's memo must overlap by substring containment,
  with amount and direction also equal. The date is NOT required to match
  (references survive clearing delays). At most one proposal per line;
  two candidates means ambiguous. Minimum reference length 4 characters
  (one character is noise, not a reference).
- `LedgerSide` gains a `memo` field for the reference-matching surface.

### Changed

- **0.2.0 breaking**: `LedgerSide` gains `memo: String`. Callers that
  construct `LedgerSide` directly must supply the field; the ledger's
  memo is what reference matching reads.


## [0.1.0] - 2026-10-07

Initial release.

### Added

- `Statement`, `StatementLine`, `Direction`; the format-independent model.
  OFX's signed amounts normalise at the boundary via `Direction::from_signed`;
  the sign convention never reaches the engine.
- `Statement::validate`; unique non-empty line ids, non-zero amounts, and the
  balance identity `opening + Σ(±lines) == closing`, which is the check that
  catches a truncated import.
- `match_exact` and `match_window`; the two-pass engine, injective by
  construction, idempotent against taken-id sets, and ambiguous rather than
  guessy: `Proposal::Ambiguous` surfaces the candidates instead of picking one.
- `days_between` via Hinnant's `days_from_civil`, pinned against leap years
  and century boundaries.
- A `double-entry` integration suite running a month-end close: exact pass,
  window pass, and the leftover split into in-transit lines versus unrecorded
  entries; which mean opposite kinds of follow-up work.
