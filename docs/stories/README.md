# User stories

Stories grouped by [persona](../personas/README.md). Each story maps to a
real implemented surface — CLI subcommand, HTTP route, MCP tool, bus topic.
If a story doesn't map to a shipped capability, it isn't here.

Story file naming: `<persona-slug>/<NN>-<short-slug>.md`. The `NN` orders
stories within a persona but is not load-bearing — read them in any order.

## Freelancer (QC, fin+inv local)

Persona: [freelancer-qc.md](../personas/freelancer-qc.md)

| # | Story |
|---|---|
| 01 | [Draft → issue → send via signed link → fin auto-marks paid](freelancer-qc/01-draft-link-fin-paid.md) |
| 02 | [Materialise monthly retainer from a recurring schedule](freelancer-qc/02-monthly-retainer-schedule.md) |
| 03 | [Reminder ladder for an overdue invoice](freelancer-qc/03-reminder-ladder-overdue.md) |

## SaaS biller (DE, fin+inv as a service)

Persona: [saas-biller-de.md](../personas/saas-biller-de.md)

| # | Story |
|---|---|
| 01 | [Webhook delivery records the realised URL in history](saas-biller-de/01-webhook-send-history.md) |
| 02 | [Idempotent re-send does not double-publish on the bus](saas-biller-de/02-idempotent-resend.md) |
| 03 | [Stripe payment via fin auto-advances invoice to paid](saas-biller-de/03-fin-stripe-payment-paid.md) |

## Agent platform integrator

Persona: [agent-platform-integrator.md](../personas/agent-platform-integrator.md)

| # | Story |
|---|---|
| 01 | [MCP tool inv_invoice_draft exposes a stable JsonSchema](agent-platform-integrator/01-mcp-draft-stable-schema.md) |
| 02 | [Agent reads invoice state via the `inv://invoice/<id>` resource](agent-platform-integrator/02-mcp-resource-read.md) |
| 03 | [Tool output surfaces emitted bus events to the agent](agent-platform-integrator/03-emitted-events-in-tool-output.md) |

## Agency owner (DZ-16, fin+inv local)

Persona: [agency-owner-dz.md](../personas/agency-owner-dz.md)

| # | Story |
|---|---|
| 01 | [DZ → DZ invoice resolves TVA with a reduced-rate line](agency-owner-dz/01-dz-local-tva-reduced.md) |
| 02 | [DZ → foreign-buyer invoice resolves as zero-rated export](agency-owner-dz/02-dz-export-zero-rated.md) |
| 03 | [Credit note against a paid invoice after a dispute](agency-owner-dz/03-credit-note-after-paid.md) |

## See also

- [personas/](../personas/) — who each story is for.
- [contracts/invoice-state-fsm.md](../contracts/invoice-state-fsm.md) — every legal `(state, event)` pair the stories exercise.
- [reference/](../reference/) — surface-by-surface lookup for the CLI commands, HTTP routes, MCP tools, and bus topics referenced.
