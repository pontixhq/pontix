# Using Pontix

Pontix reads the source of your custom Odoo modules and of Odoo itself, in
the version you run and the version you upgrade to, and reports what the
upgrade breaks, with the place in Odoo's source that shows it. It reads
code only: it never imports or runs module code, connects to a database,
or downloads anything.

Supported upgrade: **Odoo 18.0 → 19.0**.

- [What you need](#what-you-need)
- [Running a plan](#running-a-plan)
- [Which modules are checked](#which-modules-are-checked)
- [Third-party dependencies](#third-party-dependencies)
- [Reading the text report](#reading-the-text-report)
- [The JSON report](#the-json-report)
- [Exit codes and CI](#exit-codes-and-ci)
- [PTX codes](#ptx-codes)
- [What Pontix checks and how](#what-pontix-checks-and-how)
- [Known limitations](#known-limitations)
- [Security](#security)

## What you need

1. **The `pontix` binary.** From a checkout of this repository:

   ```sh
   cargo build --release
   # target/release/pontix
   ```

2. **The Odoo source of both versions**: folders with `odoo/release.py`,
   such as clones of `odoo/odoo` on branches `18.0` and `19.0`. Pontix
   reads `odoo/` (the framework), `odoo/addons/` and `addons/`. A shallow
   clone is enough:

   ```sh
   git clone --depth 1 -b 18.0 https://github.com/odoo/odoo ~/odoo/18.0
   git clone --depth 1 -b 19.0 https://github.com/odoo/odoo ~/odoo/19.0
   ```

   If you use Odoo Enterprise, add its folder as a `--deps` /
   `--deps-to` pair (see below), so that the enterprise modules your
   modules depend on are known in both versions.

3. **Your custom modules**, in the version you run now: one or more
   folders that hold module directories (each with `__manifest__.py`).

## Running a plan

```sh
pontix plan --from 18.0 --to 19.0 \
  --core-from ~/odoo/18.0 --core-to ~/odoo/19.0 \
  --addons ~/client/custom
```

| Option | Meaning |
| --- | --- |
| `--from VERSION` | The Odoo version the modules run on now (`18.0`). |
| `--to VERSION` | The version to upgrade to (`19.0`). |
| `--core-from DIR` | The Odoo source tree of `--from`. |
| `--core-to DIR` | The Odoo source tree of `--to`. |
| `--addons DIR` | A folder of your custom modules. Repeat it for several. Required. |
| `--deps DIR` | A folder of third-party modules your modules depend on (OCA, a vendor, Enterprise), in the `--from` version. Repeat it for several. |
| `--deps-to DIR` | The same third-party modules in the `--to` version. Needs `--deps`. |
| `--modules a,b,c` | Report only these modules. |
| `--format text\|json` | `text` (default) for people, `json` for tools and AI agents. |
| `--output FILE` | Write the report to a file instead of the screen. |
| `--fail-on error\|warning` | Exit with code 1 when a finding is at least this severe. |

Pontix prints `pontix: checking N module(s) ...` on stderr, then the
report. Colors are used on a terminal only; `NO_COLOR` turns them off.

## Which modules are checked

- Every module found in the `--addons` folders is **being upgraded**:
  Pontix checks each one against Odoo, and a dependency between two of
  them is never reported as missing.
- `--modules` only narrows the report. The other modules of `--addons`
  are still read, so that the selected ones resolve their dependencies.
- Each module is checked **against what it installs itself**: its
  dependencies, their dependencies, and the `auto_install` modules they
  trigger, in each version. A field that some unrelated module of your
  folders declares does not hide a field the upgrade removes for this
  one. When a dependency cannot be installed in the source version,
  Pontix falls back to everything it found, rather than guess.
- A module whose manifest version is not of the `--from` series gets a
  notice: it may not be migrated yet, and the findings may be noisy.

## Third-party dependencies

Give modules you do not maintain yourself, but that your modules depend
on, with `--deps` (the version you run) and `--deps-to` (the version you
upgrade to):

```sh
pontix plan --from 18.0 --to 19.0 \
  --core-from ~/odoo/18.0 --core-to ~/odoo/19.0 \
  --addons ~/client/custom \
  --deps ~/oca/18.0/sale-workflow   --deps-to ~/oca/19.0/sale-workflow \
  --deps ~/oca/18.0/stock-logistics-workflow --deps-to ~/oca/19.0/stock-logistics-workflow
```

- Modules in `--deps` are **not upgraded**: they are treated like Odoo
  core, and checked in their `--deps-to` version.
- A module that is in `--deps` but in none of the `--deps-to` folders is
  **gone in the target version** (renamed, merged or not migrated yet):
  your modules that depend on it get `PTX-M001`.
- Fields, methods and views of a dependency that change between the two
  versions are reported like core changes, with their findings in scope
  `dependency`.
- With `--deps` and no `--deps-to`, the dependencies are taken as
  unchanged, and Pontix says so on stderr.
- If a module exists in both `--addons` and a `--deps` folder, the
  `--addons` copy wins, as it would first in Odoo's addons path.

## Reading the text report

```text
Pontix plan 18.0 -> 19.0 · 2 module(s) checked

Custom modules: 5 error(s) · 5 warning(s) · 1 info
  code break 9 · data loss 0 · data risk 0 · behavior 1 · housekeeping 0
Core and dependency data: 145 error(s) · 12 warning(s)

sale_mrp_bom
  ERROR  PTX-F202  Field sale.order.line.route_id does not exist in 19.0 (path route_id in view sale_mrp_bom.sale_order_line_tree).
         at: sale_mrp_bom/views/sale_order_line.xml:8
         evidence: [18.0] addons/sale_stock/models/sale_order_line.py:17 · gone in 19.0
         suggestion: sale.order.line.route_ids (high)
```

- **Per module**, each finding gives its severity, its PTX code and what
  is wrong; `at:` where your module says it (several places are grouped
  on one finding); `evidence:` where Odoo declares it in each version
  (`[18.0]` / `[19.0]` mark paths in the Odoo or dependency source), or the
  Odoo rule it comes from; and, when Pontix can tell, a `suggestion` with
  its confidence (`high`, `medium`, `low`).
- **Severity**: `ERROR` breaks install, loading or a code path;
  `WARN` needs a look and may break; `INFO` is worth knowing. A problem
  that was already there before the upgrade is `INFO`: the upgrade did
  not cause it. Findings in tests are warnings.
- **Core and dependency data** lists what the upgrade does to the data
  of every database with these modules installed, whatever your code
  does: stored columns and tables that go away (`PTX-D901`), selection
  keys that are dropped (`PTX-D902`), stored fields that change type
  (`PTX-D903`). Odoo's own upgrade scripts may handle them; Pontix shows
  them so that you check.
- The report ends with **what Pontix does not check** yet. No finding
  does not mean the upgrade is safe: test it on a copy of the database.

## The JSON report

`--format json` writes one document, for tools, CI and AI agents:

```json
{
  "schema_version": 1,
  "pontix_version": "0.0.0",
  "from": "18.0",
  "to": "19.0",
  "modules": ["base_location", "sale_mrp_bom"],
  "summary": { "error": 5, "warning": 5, "info": 1 },
  "findings": [
    {
      "code": "PTX-F202",
      "severity": "error",
      "category": "code_break",
      "module": "sale_mrp_bom",
      "scope": "custom",
      "subject": "sale.order.line.route_id",
      "message": "Field sale.order.line.route_id does not exist in 19.0 (...)",
      "locations": [
        { "root": "addons", "path": "sale_mrp_bom/views/sale_order_line.xml", "line": 8 }
      ],
      "basis": {
        "kind": "core",
        "before": { "root": "core_from", "path": "addons/sale_stock/models/sale_order_line.py", "line": 17 },
        "after": null
      },
      "suggestion": { "rename_to": "sale.order.line.route_ids", "confidence": "high" }
    }
  ],
  "notices": [{ "module": "...", "message": "..." }],
  "not_covered": ["js", "qweb_outside_reports", "untyped_python"]
}
```

| Field | Meaning |
| --- | --- |
| `schema_version` | Raised whenever a change could break a tool reading the report. |
| `modules` | The modules reported on. |
| `summary` | Counts by severity, over all findings. |
| `code`, `severity`, `category` | See [PTX codes](#ptx-codes). `category` is `code_break`, `data_loss`, `data_risk`, `behavior` or `housekeeping`. |
| `module`, `scope` | The module the finding is about, and whether it is one of yours (`custom`), a third-party dependency (`dependency`) or Odoo (`core`). |
| `subject` | What it is about, such as `model.field`, a record id, or a view and its XPath. Stable enough to compare runs. |
| `locations` | Where your module says it. `root` is the folder the `path` is relative to: `addons`, `deps`, `deps_to`, `core_from` or `core_to`. Paths never contain the absolute folder. |
| `basis` | Why Pontix says so: `core` with where Odoo (or a dependency) declares it `before` and `after` the upgrade (`null` when gone), `rule` with the Odoo rule it comes from, or `not_in_core`. |
| `suggestion` | A likely rename, with its confidence, or `null`. |
| `notices` | What to know about a module that is not a finding. |
| `not_covered` | What Pontix does not check yet. |

## Exit codes and CI

| Code | Meaning |
| --- | --- |
| 0 | The plan ran; no finding reached `--fail-on`. |
| 1 | A finding reached `--fail-on`. |
| 2 | Bad input: a folder that does not exist, an Odoo tree without `odoo/release.py`, an unknown version, a module in `--modules` that is not in `--addons`. |

```sh
# Fail the pipeline on errors, keep the report as an artifact
pontix plan ... --format json --output pontix.json --fail-on error
```

## PTX codes

The severity is the one a finding gets when the upgrade causes it.

| Code | Severity | What it means |
| --- | --- | --- |
| PTX-M001 | error | A module in `depends` is gone or renamed. |
| PTX-M002 | error | The manifest version is of another series: Odoo skips the module without an error. |
| PTX-M003 | warning | A `migrations/` folder holds scripts for versions below the target: they never run again. |
| PTX-M004 | error | A class with an `_inherit` list and no `_name` gets another model name in 19.0. |
| PTX-M005 | error | The manifest is not one literal expression: Odoo ignores the module. |
| PTX-M006 | error | The module only has `__openerp__.py`, which 19.0 no longer reads. |
| PTX-M007 | error | An import inside the module points to nothing: the module fails to load. |
| PTX-M101 | error | A model the module uses is gone. |
| PTX-M102 | error | A model the module uses is probably merged or renamed (with a suggestion). |
| PTX-M103 | error | A model the module defines now also exists in core. |
| PTX-F201 | error | A field the module refers to is gone. |
| PTX-F202 | error | A field the module refers to is probably renamed (with a suggestion). |
| PTX-F203 | error | A core field the module declares again has another type. |
| PTX-F205 | warning | A core field the module declares again is gone from core: it is now a field of its own. |
| PTX-F206 | warning | A field is declared with a parameter its field class no longer knows (`auto_join` in 19.0): Odoo ignores it, with a warning. |
| PTX-P401 | error | A core method the module overrides with `super()` is gone. |
| PTX-P402 | warning | A core method the module overrides takes other parameters (info when only their names change). |
| PTX-P403 | warning | A core method the module overrides does something else: check the override (info when it calls `super()`). |
| PTX-P404 | warning | A core method the module overrides without `super()` is gone. |
| PTX-A101 | error | A framework name (import, function, decorator) or a model method the module uses is gone. |
| PTX-A102 | warning | A framework name or model method the module uses is deprecated in the target. |
| PTX-R601 | error | A record the module refers to by external id is gone. |
| PTX-R602 | warning | A data file loaded without demo data refers to a record only demo data creates: installing on a database without demo data fails. |
| PTX-X301 | error | An inheriting view's XPath or locator (`<field>`, `<button name>`, ...) finds nothing in the view it inherits, as the target version makes it. |
| PTX-T802 | warning | Tests use records only demo data creates, and 19.0 makes databases without demo data unless run with `--with-demo`. One finding per module. |
| PTX-D901 | error | A stored column or table goes away, and its data with it. |
| PTX-D902 | warning | A selection field no longer accepts keys stored rows may hold. |
| PTX-D903 | warning | A stored field changes type: its data must be converted. |

## What Pontix checks and how

- **Manifests**: versions, dependencies (installed as Odoo would, with
  `auto_install`), `__openerp__.py`, `migrations/` folders.
- **Python models**: `_name` / `_inherit` / `_inherits`, field
  declarations (type, comodel, `related`, `compute`, selection keys,
  parameters), method overrides (gone, parameters, core body changed),
  `@api.depends` / `onchange` / `constrains` paths.
- **Python code** in methods and tests: fields and models reached through
  `self`, `self.env[...]`, `env.ref(...)`, `Form(...)`, comprehensions and
  local variables; framework imports and calls (`odoo.fields.first`,
  `odoo.osv`, `BaseModel` methods), against the framework of each version.
- **XML and CSV data**: records and their fields, external ids (`ref`,
  `eval`, `groups`, `%(...)d`, `:id` and `/id` columns of CSV files such
  as `ir.model.access.csv`), view fields, the fields that view
  expressions name (`invisible`, `readonly`, `required`,
  `column_invisible`, `context`, and both sides of `domain` and
  `filter_domain`), and inheriting views.
- **QWeb reports**: for each report (`ir.actions.report`), Pontix renders
  its template as the source version does (every extension in scope
  applied, `t-call` followed) knowing that `docs` holds records of the
  report's model, and follows `t-foreach`/`t-as` and `t-set`/`t-value`
  variables through fields. It checks the fields your module's elements
  read in `t-field`, `t-esc`, `t-out`, `t-if`, `t-value`, `t-att-*` and
  `t-attf-*`. A variable it cannot type, such as one a method returns,
  is left alone. For views,
  Pontix rebuilds the inherited view in each version (the base view plus
  every extension your module installs, in Odoo's order) and checks each
  XPath and locator against it. It reports only what found something in
  the source version, so a view Pontix cannot rebuild with certainty is
  left alone.
- **Tests**: what they use from core, demo data they rely on, records
  they `env.ref`.
- **Data**: what the upgrade does to stored columns, tables and selection
  keys of the installed modules.

Every finding is backed by the Odoo source of both versions, or by an
Odoo rule Pontix names. When Pontix cannot tell, it says nothing rather
than guess: precision comes first.

## Known limitations

- **One upgrade step**: 18.0 → 19.0 only. Several steps (17.0 → 19.0) and
  20.0 are not supported yet.
- **Not checked yet** (also listed in every report):
  - JavaScript and OWL: imports, `patch()`, OWL template inheritance.
  - QWeb outside reports (website pages, portal, mail templates), and
    report variables whose model Pontix cannot tell (values a method
    returns, `t-call` with a dynamic name).
  - Python code whose model Pontix cannot tell, such as a method
    parameter or a value passed between functions.
- **Behavior**: `PTX-P403` says a core method you override changed; it
  does not say whether your override is still right.
- **Odoo's upgrade scripts** are not read: a `PTX-D901` may be handled by
  an upgrade script, and Odoo's `upgrade_code` tool (run it first) fixes
  some syntax Pontix deliberately does not report, such as
  `_sql_constraints`.
- **Data in your database** (records created by users, studio
  customizations, server actions written in the UI) is not seen: Pontix
  reads source code only.

## Security

- Pontix treats every source file as untrusted input. It never imports
  or runs Python, never expands XML entities (a DTD is refused), caps the
  size of a file (10 MiB), the number of XML nodes and the nesting depth
  it follows, and never reads outside the folders you give it (symbolic
  links that lead outside are refused).
- Reports contain paths relative to those folders, never absolute paths.
- Pontix does not use the network.

Security issues: see [SECURITY.md](../SECURITY.md).
