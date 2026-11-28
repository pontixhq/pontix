# Pontix

Pontix analyses custom Odoo modules and predicts what will break, and why,
when you upgrade to a new Odoo version, with evidence from the target Odoo
source.

> Status: early development. Being tested on real migrations before a
> first release.

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

## Usage

`pontix plan` checks custom modules for an upgrade to the next Odoo version.
It needs the Odoo source of both versions (folders with `odoo/release.py`)
and reads code only: it never runs module code or downloads anything.

```sh
pontix plan --from 18.0 --to 19.0 \
  --core-from ~/odoo/18.0 --core-to ~/odoo/19.0 \
  --addons ~/client/custom

# Third-party modules the custom ones depend on (OCA and others): give
# them in both versions. They are not upgraded; a module only in --deps
# is reported as gone from the target version.
pontix plan ... --deps ~/oca/18.0/sale-workflow --deps-to ~/oca/19.0/sale-workflow

# JSON for tools and AI agents
pontix plan ... --format json --output plan.json

# In CI: exit 1 when there is an error
pontix plan ... --fail-on error
```

Each finding has a PTX code, a severity, where the custom module refers to
core, and where core declares it in each version. Exit codes: 0 done, 1 a
finding reached `--fail-on`, 2 bad input. A report always lists what Pontix
does not check yet; no finding does not mean the upgrade is safe.

The full guide — options, how modules and dependencies are checked, the
text and JSON reports, every PTX code, and the known limitations — is in
[docs/usage.md](docs/usage.md).

## Development

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Security issues: see [SECURITY.md](SECURITY.md).

"Odoo" is a trademark of Odoo S.A. Pontix is not affiliated with or endorsed by Odoo S.A.
