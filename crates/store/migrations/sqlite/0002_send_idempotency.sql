-- T-0037 — send_invoice idempotency table.
--
-- `send_invoice` (and `send_invoice_render`) accept an
-- `idempotency_key: Option<String>` on their input. To honour replay
-- semantics — second call returns the original output with
-- `idempotency_replay: true`, no FSM mutation, no second history row,
-- no duplicate `inv.billing.invoice.sent` event — we record the first
-- (invoice_id, idempotency_key) tuple here at the same transaction
-- that writes the `send` history row.
--
-- Why a dedicated table (not the invoices.idempotency_key column the
-- draft path reuses)?
--   - draft's key is invoice-scoped via PRIMARY KEY uniqueness; one
--     key produces one invoice, full stop. send is per-(invoice,key):
--     the same operator can resend an invoice with multiple distinct
--     keys, and the same key can legitimately scope different sends
--     on different invoices.
--   - keeping the schema polymorphism-free (no "kind" column) keeps
--     the query surface narrow and unambiguous.
--
-- delivered_to is cached so replays can return the exact destination
-- string the original call resolved to (file:// uri may have differed
-- from the input if the path was rewritten — today it isn't, but the
-- audit row records the resolved form so we match it here).

CREATE TABLE send_idempotency (
    invoice_id      TEXT NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    idempotency_key TEXT NOT NULL,
    history_id      TEXT NOT NULL,
    delivered_to    TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    PRIMARY KEY (invoice_id, idempotency_key)
);

CREATE INDEX idx_send_idempotency_history ON send_idempotency (history_id);
