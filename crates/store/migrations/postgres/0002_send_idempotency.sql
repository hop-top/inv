-- T-0037 — send_invoice idempotency table (postgres flavour).
--
-- Mirror of the sqlite migration; see that file for the rationale.
-- Difference: TIMESTAMPTZ in place of TEXT for `created_at`.

CREATE TABLE send_idempotency (
    invoice_id      TEXT NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    idempotency_key TEXT NOT NULL,
    history_id      TEXT NOT NULL,
    delivered_to    TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (invoice_id, idempotency_key)
);

CREATE INDEX idx_send_idempotency_history ON send_idempotency (history_id);
