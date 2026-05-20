-- inv v1 initial schema (sqlite).
--
-- Mirrors design spec §5. Decimal columns are TEXT (canonical
-- `rust_decimal::Decimal` string form). Timestamps are TEXT
-- (rfc3339 strings on the wire; sqlx converts to chrono::DateTime<Utc>).

CREATE TABLE customers (
    id           TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    email        TEXT,
    address_json TEXT NOT NULL,
    jurisdiction TEXT,
    metadata     TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

CREATE TABLE invoices (
    id              TEXT PRIMARY KEY,
    number          TEXT UNIQUE,
    customer_id     TEXT NOT NULL REFERENCES customers(id),
    seller_jur      TEXT NOT NULL,
    currency        TEXT NOT NULL,
    state           TEXT NOT NULL,
    issued_at       TEXT,
    due_at          TEXT,
    sent_at         TEXT,
    viewed_at       TEXT,
    paid_at         TEXT,
    voided_at       TEXT,
    subtotal        TEXT NOT NULL,
    tax_total       TEXT NOT NULL,
    total           TEXT NOT NULL,
    amount_paid     TEXT NOT NULL DEFAULT '0',
    schedule_id     TEXT,
    template_path   TEXT,
    pdf_blob_ref    TEXT,
    idempotency_key TEXT UNIQUE,
    nexus_review    INTEGER NOT NULL DEFAULT 0,
    metadata        TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE TABLE invoice_lines (
    id           TEXT PRIMARY KEY,
    invoice_id   TEXT NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    position     INTEGER NOT NULL,
    description  TEXT NOT NULL,
    quantity     TEXT NOT NULL,
    unit_price   TEXT NOT NULL,
    tax_rate_ids TEXT,
    tax_category TEXT NOT NULL DEFAULT 'standard',
    tax_amount   TEXT NOT NULL,
    line_total   TEXT NOT NULL,
    metadata     TEXT,
    UNIQUE(invoice_id, position)
);

CREATE TABLE tax_rates (
    id              TEXT PRIMARY KEY,
    jurisdiction    TEXT NOT NULL,
    applies_country TEXT,
    applies_region  TEXT,
    name            TEXT NOT NULL,
    rate            TEXT NOT NULL,
    category        TEXT NOT NULL,
    effective_from  TEXT NOT NULL,
    effective_to    TEXT
);

CREATE TABLE nexus_thresholds (
    state     TEXT PRIMARY KEY,
    revenue   TEXT,
    txn_count INTEGER,
    enabled   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE reminders (
    id           TEXT PRIMARY KEY,
    invoice_id   TEXT NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    scheduled_at TEXT NOT NULL,
    sent_at      TEXT,
    channel      TEXT NOT NULL,
    state        TEXT NOT NULL
);

CREATE TABLE schedules (
    id             TEXT PRIMARY KEY,
    customer_id    TEXT NOT NULL REFERENCES customers(id),
    template_lines TEXT NOT NULL,
    currency       TEXT NOT NULL,
    cadence        TEXT NOT NULL,
    start_date     TEXT NOT NULL,
    end_date       TEXT,
    auto_issue     INTEGER NOT NULL DEFAULT 0,
    next_run       TEXT NOT NULL,
    last_run       TEXT,
    state          TEXT NOT NULL,
    metadata       TEXT,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE TABLE credit_notes (
    id         TEXT PRIMARY KEY,
    number     TEXT UNIQUE,
    invoice_id TEXT NOT NULL REFERENCES invoices(id),
    state      TEXT NOT NULL,
    amount     TEXT NOT NULL,
    currency   TEXT NOT NULL,
    reason     TEXT,
    refund_ref TEXT,
    issued_at  TEXT,
    created_at TEXT NOT NULL,
    metadata   TEXT
);

CREATE TABLE invoice_state_history (
    id           TEXT PRIMARY KEY,
    invoice_id   TEXT NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    from_state   TEXT,
    to_state     TEXT NOT NULL,
    event        TEXT NOT NULL,
    actor        TEXT,
    channel      TEXT NOT NULL,
    bus_event_id TEXT,
    reason       TEXT,
    occurred_at  TEXT NOT NULL,
    published_at TEXT,
    metadata     TEXT
);

CREATE INDEX idx_inv_history_time ON invoice_state_history (invoice_id, occurred_at);
CREATE INDEX idx_inv_history_outbox ON invoice_state_history (published_at) WHERE published_at IS NULL;

CREATE TABLE credit_note_state_history (
    id             TEXT PRIMARY KEY,
    credit_note_id TEXT NOT NULL REFERENCES credit_notes(id) ON DELETE CASCADE,
    from_state     TEXT,
    to_state       TEXT NOT NULL,
    event          TEXT NOT NULL,
    actor          TEXT,
    channel        TEXT NOT NULL,
    bus_event_id   TEXT,
    occurred_at    TEXT NOT NULL,
    published_at   TEXT,
    metadata       TEXT
);

CREATE TABLE bus_inbox (
    event_id     TEXT PRIMARY KEY,
    topic        TEXT NOT NULL,
    source       TEXT NOT NULL,
    received_at  TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    processed_at TEXT,
    invoice_id   TEXT
);
