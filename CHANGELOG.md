# Changelog

## [0.1.0-alpha.1](https://github.com/hop-top/inv/compare/v0.1.0-alpha.0...v0.1.0-alpha.1) (2026-05-23)


### ⚠ BREAKING CHANGES

* package names changed for all 9 workspace crates to align with the hop-top namespace convention (matches hop-top-kit, hop-top-uri, hop-top-xrr). The bin/inv binary keeps its end-user-facing name. Internal folder paths (crates/api, crates/core, etc.) unchanged.

### Features

* **api:** HTTP API channel adapter (T-0017) ([2bdfeda](https://github.com/hop-top/inv/commit/2bdfeda23f51fd3e4e2fb9e80d6deaf1ece182aa))
* **bus:** consumer adapter for fin.billing.* events (T-0015) ([6b59dd2](https://github.com/hop-top/inv/commit/6b59dd2b5b311af494d83edb4b00675691977046))
* **bus:** event publisher + outbox relay + inbox dedup (T-0014) ([f3acddd](https://github.com/hop-top/inv/commit/f3acddddef2835e29b1c94cbc01aecc759828723))
* **cli:** full inv CLI surface (T-0016) ([9bde4e1](https://github.com/hop-top/inv/commit/9bde4e1a13ab8f9ec48df2d1121abe0d34ce456c))
* **cli:** inv invoice list surfaces schedule column with typeid suffix (T-0042) ([58cec63](https://github.com/hop-top/inv/commit/58cec631c5a72bd0f568f41b197d28189fa7b286))
* **cli:** inv server composes all adapters (T-0020) ([0b5a0b3](https://github.com/hop-top/inv/commit/0b5a0b3934ad3c7dfbf0dc9d5004bc707236db49))
* **commands,api:** viewed publish via CoreCtx.publisher; relocate Publisher trait into inv-commands (T-0031) ([d5a1998](https://github.com/hop-top/inv/commit/d5a1998c4133b3e7a10ac7ac0b4a771d0fa69976))
* **commands,store:** materialised invoices carry schedule_id provenance (T-0039) ([29d9acf](https://github.com/hop-top/inv/commit/29d9acf27ba5ec7e42843eddaa2af9e9a1360ec4))
* **commands,store:** send_invoice idempotency dedup via send_idempotency table (T-0037) ([0b442c5](https://github.com/hop-top/inv/commit/0b442c5dd0c5873d0ef5a39ab803712460105fe0))
* **commands:** derive Serialize on EmittedEvent, drop adapter mirrors (T-0028) ([f7e85be](https://github.com/hop-top/inv/commit/f7e85be21c27e10a2d44dedd1f58f63e9e9148f0))
* **commands:** draft / issue / send commands (T-0011) ([df774b1](https://github.com/hop-top/inv/commit/df774b17591b99cfdb2b399f38083495c731c8b9))
* **commands:** payment / void / credit / overdue commands (T-0012) ([b99089b](https://github.com/hop-top/inv/commit/b99089b888760520f2becb5ced12df4346f3f966))
* **commands:** schedules + reminders (T-0013) ([033d20d](https://github.com/hop-top/inv/commit/033d20dfd758159319296258b6da44820e764a03))
* **commands:** wire BlobStore into issue_invoice (T-0025) ([3c1c2c6](https://github.com/hop-top/inv/commit/3c1c2c6f8fef7da1ba5f377c99986cb0474a4e8e))
* convert scaffold to Cargo workspace (T-0002) ([050171d](https://github.com/hop-top/inv/commit/050171d1a57b3b3a3a7da82766cf8c031da6290d))
* **core:** domain types (T-0004) ([8a1dc23](https://github.com/hop-top/inv/commit/8a1dc232f1cce896ecfc9721f878852fcc7ee6f1))
* **core:** FSM facade + statig adapter for CreditNote (T-0006) ([acc7e54](https://github.com/hop-top/inv/commit/acc7e5403ccc0bf3251f167e71c71b9369087023))
* **core:** FSM facade + statig adapter for Invoice (T-0005) ([d1242fa](https://github.com/hop-top/inv/commit/d1242fa792c826a7b98c93483c63a27d831d42a0))
* **core:** JsonSchema derives behind 'schema' cargo feature (T-0029) ([f7ddecb](https://github.com/hop-top/inv/commit/f7ddecb87e58be71c47b99d2a402831d0082d111))
* **core:** render pipeline — Tera HTML + stubbed PDF (T-0010) ([187fc3e](https://github.com/hop-top/inv/commit/187fc3e9cc68b8d77a88b63910b7c9a7249e5d71))
* **core:** tax engine (T-0007) ([6e783cb](https://github.com/hop-top/inv/commit/6e783cbf481bb425a4f192aa79247e1742b2f203))
* **e2e:** inv-e2e test crate covering 12 user stories with xrr cassettes (T-0036) ([8bdf2f8](https://github.com/hop-top/inv/commit/8bdf2f8d0f00b734f9c27d8d91dba5ed65f8a48f))
* initialize inv ([9aa1f27](https://github.com/hop-top/inv/commit/9aa1f27c9694289474a5cfdb69f3146215b70572))
* **mcp:** inv_invoice_show tool includes invoice_lines (parity with resource) (T-0041) ([a72dc75](https://github.com/hop-top/inv/commit/a72dc753085ca4a4c8089d7d3daa5c38e4dd3aaa))
* **mcp:** inv://invoice/{id} resource includes invoice_lines (T-0038) ([694d5a3](https://github.com/hop-top/inv/commit/694d5a3140f1038bdc5c6d7682ec55793e189ba8))
* **mcp:** MCP server adapter (T-0019) ([9a3f5d3](https://github.com/hop-top/inv/commit/9a3f5d397687b2170822817950aa6cd401fda18c))
* pin baseline deps and Cargo features (T-0003) ([8f454d3](https://github.com/hop-top/inv/commit/8f454d301b7191171692397b57595487c9a0a9fc))
* scaffold inv ([2081f3a](https://github.com/hop-top/inv/commit/2081f3ae56a3e8ae4f215caae938d8b7b8dd9d1b))
* **store,api,cli:** list(filter) methods on ScheduleRepo and CreditNoteRepo (T-0033) ([a03d3c2](https://github.com/hop-top/inv/commit/a03d3c2c1f3f7079828d2327e76442f6249841b0))
* **store,bus:** CreditNoteHistoryRepo outbox API, drop raw sqlx from relay (T-0030) ([6427dbb](https://github.com/hop-top/inv/commit/6427dbb8a2a08c756296e5693ae698305c51ab55))
* **store,commands:** InvoiceRepo::find_by_idempotency_key indexed lookup (T-0040) ([7c3ec01](https://github.com/hop-top/inv/commit/7c3ec010c99b39a6e38503d25ed0de48817616ce))
* **store:** blob storage facade with local backend (T-0009) ([7dc967f](https://github.com/hop-top/inv/commit/7dc967ffab834b8d3b661a1d830087e5eff7f0c9))
* **store:** portable SQL placeholders for postgres backend (T-0051) ([810f136](https://github.com/hop-top/inv/commit/810f13619db341ed53953fc7e9d04f8426924682))
* **store:** sqlx wiring + migrations + repository structs (T-0008) ([4ef946d](https://github.com/hop-top/inv/commit/4ef946de53bbaa90a2b47ceb2663c1dd27253a8f))
* **test:** parameterise integration fixtures on DATABASE_URL (T-0050) ([1bea6f6](https://github.com/hop-top/inv/commit/1bea6f60b683685d43c7d5df82036f640ca2bd50))
* **ws:** WebSocket channel adapter (T-0018) ([da7cdbf](https://github.com/hop-top/inv/commit/da7cdbf14462f4f84a55f2df871f16152e7aec3d))


### Bug Fixes

* **api,commands:** constant-time bearer auth + render-only send path for webhook destination (T-0032) ([0363e8f](https://github.com/hop-top/inv/commit/0363e8f95e2c424242e3c498efe79d18a7711411))
* **api:** deterministic tampered-token swap in signed_link_view_tampered_404 (T-0034) ([65c8670](https://github.com/hop-top/inv/commit/65c86704ae4d84f7cdff233a26bc9882e787428d))
* **ci:** release-please uses simple release-type to avoid Cargo workspace walk (T-0047) ([217fcef](https://github.com/hop-top/inv/commit/217fcef4fdc7ddf4b7f21a7e54406c5b73edc88c))
* **core:** allow needless_return on render_pdf (load-bearing across cfg arms) ([9d568ef](https://github.com/hop-top/inv/commit/9d568ef8a7f0856cf11ecded6c86a7e371d8144c))
* **deps:** switch hop-top-kit + hop-top-uri to crates.io versions (T-0045) ([e37f72c](https://github.com/hop-top/inv/commit/e37f72c1fb2fee4cd5a5a4dc18cadf44b1e59351))
* **render:** template guards metadata, removes _render_placeholder workaround (T-0026) ([904585f](https://github.com/hop-top/inv/commit/904585f2c61005329ded32282798817830403d7c))
* **store:** transaction-aware repo writes via ON CONFLICT upsert (T-0024) ([260cfaa](https://github.com/hop-top/inv/commit/260cfaa918f7fe93e7ccf8f56177be5e0a7612ab))


### Refactoring

* rename internal crates from inv-X to hop-top-inv-X ([d7a55de](https://github.com/hop-top/inv/commit/d7a55de849e3d4f2c654fdf26a959ea70ea5ed47))

## Changelog
