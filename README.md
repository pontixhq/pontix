# Pontix

Pontix analyses custom Odoo modules and predicts what will break, and why,
when you upgrade to a new Odoo version, with evidence from the target Odoo
source.

> Status: early development (Phase 1). Not ready for use.

## Workspace

| Crate | Purpose | License |
| --- | --- | --- |
| `pontix-core` | Shared data types: symbols, changes, findings (PTX codes) | Apache-2.0 |
| `pontix-parsers` | Python and XML parsers for Odoo source | Apache-2.0 |
| `pontix-index` | Finds modules, installs them as Odoo would, and indexes their models | Apache-2.0 |
| `pontix-diff` | Compares two indexed Odoo versions: what changed, and likely renames | Apache-2.0 |
| `pontix-scan` | Finds what custom modules rely on in core, and what an upgrade breaks | FSL-1.1-ALv2 |
| `pontix-cli` | The `pontix` command line tool | FSL-1.1-ALv2 |

Apache-2.0 crates never depend on FSL crates.

## Development

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Security issues: see [SECURITY.md](SECURITY.md).

"Odoo" is a trademark of Odoo S.A. Pontix is not affiliated with or endorsed by Odoo S.A.
