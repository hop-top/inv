# Story agency-owner-dz-03: Credit note against a paid invoice after a dispute

**Persona**: [Agency owner (Algiers, fin+inv local)](../../personas/agency-owner-dz.md)

## Story

As a DZ-16 agency owner, when a client disputes a paid invoice and we agree
to a partial refund, I want to issue a credit note against the invoice
(since `void` is closed after payment) and have it numbered `CN-YYYY-NNNN`
in the same year-sequence as my invoices, so that my accounting trail is
intact.

## Acceptance criteria

**Given** an invoice in `state = "paid"` at `invoice_01...` with `total =
"173500"` DZD
**When** I run
```sh
inv invoice void invoice_01... --reason "Client dispute"
```
**Then** the command fails with `CoreError::FsmTransition` mapped to an
"illegal transition: void not allowed from Paid" message (per
[contracts/invoice-state-fsm.md](../../contracts/invoice-state-fsm.md) and
[user/troubleshooting.md#illegal-transition--not-allowed-from-paid](../../user/troubleshooting.md#illegal-transition--not-allowed-from-paid)),
no state change, no bus emission.

**Given** the same invoice
**When** I run
```sh
inv creditnote draft --invoice invoice_01... \
  --amount 50000 --reason "Partial refund: under-delivered scope"
inv creditnote issue creditnote_01...
```
**Then** a credit-note row is persisted in `state = "draft"`, then
transitions to `issued` (terminal per
[contracts/creditnote-state-fsm.md](../../contracts/creditnote-state-fsm.md)),
receives a number like `CN-2026-0001`, and emits
`inv.billing.creditnote.drafted` followed by `inv.billing.creditnote.issued`
(plus the mechanic triplet on each transition).

**Given** the original invoice is still `paid`
**When** I run `inv creditnote list --invoice invoice_01...`
**Then** the credit note is listed with its number, amount (50000 DZD),
and reason. The original invoice's state is unchanged — credit notes
**net against** the invoice for reporting but do not transition the
invoice's FSM.

**Given** later, `fin` publishes `fin.billing.payment.refunded` against the
same invoice (e.g. the agency's bank refund posted through `fin`)
**When** the bus consumer processes the refund
**Then** a separate **auto-created draft** credit note is materialised (per
[contracts/creditnote-state-fsm.md#refund-driven-drafts](../../contracts/creditnote-state-fsm.md#refund-driven-drafts)).
The operator reviews + issues it (or doesn't, if the manual credit note
already covers the same refund — operator judgement, no auto-dedup at v1).

## Surfaces touched

- CLI — `inv invoice void` (fails), `inv creditnote draft`, `inv creditnote
  issue`, `inv creditnote list`.
- Commands — `void_invoice` (rejects), `create_credit_note`, `issue_credit_note`.
- Store — `credit_notes`, `credit_note_state_history`.
- Bus emission — `inv.billing.creditnote.drafted`,
  `inv.billing.creditnote.issued` (+ mechanic triplet per transition).
- Bus consumer — `fin.billing.payment.refunded` → auto-drafted credit
  note (separate from the manual one).

## Out of scope for this story

- Voiding a paid invoice — not legal at v1 by design (FSM rejects).
- Auto-dedup between manual credit notes and refund-driven drafts — not
  implemented at v1; operator reviews both.
- Refund posting in `fin` itself — that's `fin`'s concern.

## See also

- [contracts/creditnote-state-fsm.md](../../contracts/creditnote-state-fsm.md) — Draft → Issued; terminal.
- [contracts/invoice-state-fsm.md](../../contracts/invoice-state-fsm.md) — why `void` is rejected after payment.
- [user/troubleshooting.md#illegal-transition--not-allowed-from-paid](../../user/troubleshooting.md#illegal-transition--not-allowed-from-paid).
