//! Odoo CSV data files, read as Odoo's `convert_csv_import` would: the file
//! name is the model (`ir.model.access.csv`), the first row names the
//! fields, an `id` column gives each row an external id, and a column
//! ending in `:id` or `/id` refers to records by external id.
//!
//! Inputs are untrusted. Rows are read as text only; nothing is evaluated.

use std::num::NonZeroU32;
use std::path::Path;

use pontix_core::{
    DataFile, Diagnostic, DiagnosticKind, FieldName, Location, ModelName, Record, RefKind, XmlRef,
};

/// What one CSV data file declares, and what could not be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CsvFile {
    /// Its records and their references.
    pub data: DataFile,
    /// What could not be read.
    pub diagnostics: Vec<Diagnostic>,
}

/// Reads a CSV data file. A file Odoo could not read (a quote never
/// closed) is reported and yields nothing.
pub fn parse_csv(file: &Path, text: &str) -> CsvFile {
    let mut out = CsvFile::default();
    let Some(model) = file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
    else {
        return out;
    };
    let model = ModelName::new(model);
    let rows = match rows(text) {
        Ok(rows) => rows,
        Err(line) => {
            out.diagnostics.push(Diagnostic {
                kind: DiagnosticKind::InvalidCsv,
                location: at(file, line),
                snippet: "a quoted value is never closed".to_string(),
            });
            return out;
        }
    };
    let Some(((header_line, header), rows)) = rows.split_first() else {
        return out;
    };
    let header_at = at(file, *header_line);
    out.data.models.push((model.clone(), header_at.clone()));
    let columns: Vec<Column> = header.iter().map(|name| column(name)).collect();
    // The fields are those of the header: on the first record only, so
    // that each is looked up once.
    let fields: Vec<(FieldName, Location)> = columns
        .iter()
        .filter_map(|column| column.field.as_ref())
        .map(|field| (field.clone(), header_at.clone()))
        .collect();
    let id_column = columns.iter().position(|column| column.is_id);
    let mut first = true;
    for (line, row) in rows {
        let location = at(file, *line);
        let id = id_column
            .and_then(|index| row.get(index))
            .map(|id| id.trim())
            .filter(|id| !id.is_empty())
            .map(str::to_string);
        for (column, value) in columns.iter().zip(row) {
            if !column.refers {
                continue;
            }
            for xmlid in value.split(',').map(str::trim).filter(|id| !id.is_empty()) {
                out.data.refs.push(XmlRef {
                    xmlid: xmlid.to_string(),
                    kind: RefKind::Ref,
                    location: location.clone(),
                });
            }
        }
        out.data.records.push(Record {
            id,
            model: model.clone(),
            fields: if first { fields.clone() } else { Vec::new() },
            view: None,
            location,
        });
        first = false;
    }
    out
}

/// A header column: the field it sets, if any, and whether its values are
/// external ids.
struct Column {
    field: Option<FieldName>,
    is_id: bool,
    refers: bool,
}

fn column(name: &str) -> Column {
    let name = name.trim();
    if name == "id" {
        return Column {
            field: None,
            is_id: true,
            refers: false,
        };
    }
    let refers = name.ends_with(":id") || name.ends_with("/id");
    // `field:id`, `field/id`, `field/.id`, `field.id`: the field is `field`.
    let field = name
        .split([':', '/'])
        .next()
        .map(|field| field.strip_suffix(".id").unwrap_or(field))
        .filter(|field| !field.is_empty() && !field.starts_with('.'))
        .map(FieldName::new);
    Column {
        field,
        is_id: false,
        refers,
    }
}

/// The rows of a CSV text, each with the line it starts on, as Python's
/// `csv.reader` with its defaults reads them: commas, double quotes, `""`
/// for a quote inside quotes, line breaks inside quotes. Blank lines are
/// left out. `Err` gives the line of a quote never closed.
fn rows(text: &str) -> Result<Vec<(u32, Vec<String>)>, u32> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut value = String::new();
    let mut quoted = false;
    let mut line: u32 = 1;
    let mut row_line = 1;
    let mut quote_line = 1;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    value.push('"');
                }
                '"' => quoted = false,
                '\n' => {
                    line += 1;
                    value.push('\n');
                }
                other => value.push(other),
            }
            continue;
        }
        match c {
            '"' if value.is_empty() => {
                quoted = true;
                quote_line = line;
            }
            ',' => row.push(std::mem::take(&mut value)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut value));
                if row.iter().any(|cell| !cell.is_empty()) {
                    rows.push((row_line, std::mem::take(&mut row)));
                } else {
                    row.clear();
                }
                line += 1;
                row_line = line;
            }
            other => value.push(other),
        }
    }
    if quoted {
        return Err(quote_line);
    }
    row.push(value);
    if row.iter().any(|cell| !cell.is_empty()) {
        rows.push((row_line, row));
    }
    Ok(rows)
}

fn at(file: &Path, line: u32) -> Location {
    Location::new(file, NonZeroU32::new(line).unwrap_or(NonZeroU32::MIN))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_rules_name_their_model_fields_and_references() {
        let file = parse_csv(
            Path::new("custom/security/ir.model.access.csv"),
            "id,name,model_id:id,group_id:id,perm_read\n\
             access_order,order,model_sale_order,sales_team.group_sale_salesman,1\n\
             \n\
             access_line,\"line, \"\"all\"\"\",sale.model_sale_order_line,,1\r\n",
        );
        assert!(file.diagnostics.is_empty());
        let data = &file.data;
        assert_eq!(data.models[0].0, ModelName::new("ir.model.access"));
        let ids: Vec<(Option<&str>, u32)> = data
            .records
            .iter()
            .map(|record| (record.id.as_deref(), record.location.line.get()))
            .collect();
        assert_eq!(ids, [(Some("access_order"), 2), (Some("access_line"), 4)]);
        let fields: Vec<&str> = data.records[0]
            .fields
            .iter()
            .map(|(field, _)| field.as_str())
            .collect();
        assert_eq!(fields, ["name", "model_id", "group_id", "perm_read"]);
        assert!(data.records[1].fields.is_empty());
        let refs: Vec<(&str, u32)> = data
            .refs
            .iter()
            .map(|found| (found.xmlid.as_str(), found.location.line.get()))
            .collect();
        assert_eq!(
            refs,
            [
                ("model_sale_order", 2),
                ("sales_team.group_sale_salesman", 2),
                ("sale.model_sale_order_line", 4)
            ]
        );
    }

    #[test]
    fn many2many_references_and_multiline_values_are_read() {
        let file = parse_csv(
            Path::new("res.groups.csv"),
            "id,name,implied_ids/id,comment\n\
             group_a,A,\"base.group_user, base.group_portal\",\"two\nlines\"\n\
             group_b,B,group_a,\n",
        );
        let refs: Vec<(&str, u32)> = file
            .data
            .refs
            .iter()
            .map(|found| (found.xmlid.as_str(), found.location.line.get()))
            .collect();
        assert_eq!(
            refs,
            [
                ("base.group_user", 2),
                ("base.group_portal", 2),
                ("group_a", 4)
            ]
        );
    }

    #[test]
    fn an_unclosed_quote_is_reported() {
        let file = parse_csv(Path::new("res.partner.csv"), "id,name\nx,\"never closed\n");
        assert_eq!(file.diagnostics[0].kind, DiagnosticKind::InvalidCsv);
        assert_eq!(file.diagnostics[0].location.line.get(), 2);
        assert!(file.data.records.is_empty());
    }
}
