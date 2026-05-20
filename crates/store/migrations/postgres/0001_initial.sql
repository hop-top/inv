-- inv v1 initial schema (postgres).
--
-- Mirrors design spec §5. Decimal columns are NUMERIC(20,8); we read +
-- write through `rust_decimal::Decimal` (sqlx's `rust_decimal` feature).
-- The schema is otherwise identical to the sqlite version, modulo
-- `BOOLEAN` defaults using true/false instead of 0/1 and the addition
-- of `TIMESTAMPTZ` for timezone-aware columns.

CREATE TABLE customers (
    id           TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    email        TEXT,
    address_json TEXT NOT NULL,
    jurisdiction TEXT,
    metadata     TEXT,
    created_at   TIMESTAMPTZ NOT NULL,
    updated_at   TIMESTAMPTZ NOT NULL
);

CREATE TABLE invoices (
    id              TEXT PRIMARY KEY,
    number          TEXT UNIQUE,
    customer_id     TEXT NOT NULL REFERENCES customers(id),
    seller_jur      TEXT NOT NULL,
    currency        TEXT NOT NULL,
    state           TEXT NOT NULL,
    issued_at       TIMESTAMPTZ,
    due_at          TIMESTAMPTZ,
    sent_at         TIMESTAMPTZ,
    viewed_at       TIMESTAMPTZ,
    paid_at         TIMESTAMPTZ,
    voided_at       TIMESTAMPTZ,
    subtotal        NUMERIC(20,8) NOT NULL,
    tax_total       NUMERIC(20,8) NOT NULL,
    total           NUMERIC(20,8) NOT NULL,
    amount_paid     NUMERIC(20,8) NOT NULL DEFAULT 0,
    schedule_id     TEXT,
    template_path   TEXT,
    pdf_blob_ref    TEXT,
    idempotency_key TEXT UNIQUE,
    nexus_review    BOOLEAN NOT NULL DEFAULT FALSE,
    metadata        TEXT,
    created_at      TIMESTAMPTZ NOT NULL,
    updated_at      TIMESTAMPTZ NOT NULL
);

CREATE TABLE invoice_lines (
    id           TEXT PRIMARY KEY,
    invoice_id   TEXT NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    position     INTEGER NOT NULL,
    description  TEXT NOT NULL,
    quantity     NUMERIC(20,8) NOT NULL,
    unit_price   NUMERIC(20,8) NOT NULL,
    tax_rate_ids TEXT,
    tax_category TEXT NOT NULL DEFAULT 'standard',
    tax_amount   NUMERIC(20,8) NOT NULL,
    line_total   NUMERIC(20,8) NOT NULL,
    metadata     TEXT,
    UNIQUE(invoice_id, position)
);

CREATE TABLE tax_rates (
    id              TEXT PRIMARY KEY,
    jurisdiction    TEXT NOT NULL,
    applies_country TEXT,
    applies_region  TEXT,
    name            TEXT NOT NULL,
    rate            NUMERIC(20,8) NOT NULL,
    category        TEXT NOT NULL,
    effective_from  DATE NOT NULL,
    effective_to    DATE
);

CREATE TABLE nexus_thresholds (
    state     TEXT PRIMARY KEY,
    revenue   NUMERIC(20,8),
    txn_count INTEGER,
    enabled   BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE reminders (
    id           TEXT PRIMARY KEY,
    invoice_id   TEXT NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    scheduled_at TIMESTAMPTZ NOT NULL,
    sent_at      TIMESTAMPTZ,
    channel      TEXT NOT NULL,
    state        TEXT NOT NULL
);

CREATE TABLE schedules (
    id             TEXT PRIMARY KEY,
    customer_id    TEXT NOT NULL REFERENCES customers(id),
    template_lines TEXT NOT NULL,
    currency       TEXT NOT NULL,
    cadence        TEXT NOT NULL,
    start_date     DATE NOT NULL,
    end_date       DATE,
    auto_issue     BOOLEAN NOT NULL DEFAULT FALSE,
    next_run       DATE NOT NULL,
    last_run       DATE,
    state          TEXT NOT NULL,
    metadata       TEXT,
    created_at     TIMESTAMPTZ NOT NULL,
    updated_at     TIMESTAMPTZ NOT NULL
);

CREATE TABLE credit_notes (
    id         TEXT PRIMARY KEY,
    number     TEXT UNIQUE,
    invoice_id TEXT NOT NULL REFERENCES invoices(id),
    state      TEXT NOT NULL,
    amount     NUMERIC(20,8) NOT NULL,
    currency   TEXT NOT NULL,
    reason     TEXT,
    refund_ref TEXT,
    issued_at  TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL,
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
    occurred_at  TIMESTAMPTZ NOT NULL,
    published_at TIMESTAMPTZ,
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
    occurred_at    TIMESTAMPTZ NOT NULL,
    published_at   TIMESTAMPTZ,
    metadata       TEXT
);

CREATE TABLE bus_inbox (
    event_id     TEXT PRIMARY KEY,
    topic        TEXT NOT NULL,
    source       TEXT NOT NULL,
    received_at  TIMESTAMPTZ NOT NULL,
    payload_json TEXT NOT NULL,
    processed_at TIMESTAMPTZ,
    invoice_id   TEXT
);
