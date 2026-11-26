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
| `pontix-cli` | The `pontix` command line tool | FSL-1.1-ALv2 |

## Development

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Security issues: see [SECURITY.md](SECURITY.md).

"Odoo" is a trademark of Odoo S.A. Pontix is not affiliated with or endorsed by Odoo S.A.
