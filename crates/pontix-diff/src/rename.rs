//! Removals and additions that look like one thing renamed.
//!
//! A rename is never certain from code alone, so each candidate carries a
//! [`Confidence`] and the [`Evidence`] behind it. Models come first, because
//! a renamed model also renames comodels (`product.packaging` to
//! `product.uom` turns `packaging_ids` into `product_uom_ids`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use pontix_core::{
    Change, Confidence, Evidence, FieldName, FieldType, ModelName, ModuleName, Rename,
};

use crate::view::{FieldView, ModelView};

/// Finds rename candidates among `changes`, using both versions' models.
pub(crate) fn renames(
    changes: &[Change],
    before: &BTreeMap<ModelName, ModelView<'_>>,
    after: &BTreeMap<ModelName, ModelView<'_>>,
    target_had_module: impl Fn(&ModuleName) -> bool,
) -> Vec<Rename> {
    let mut found = modules(changes, target_had_module);
    let models = models(changes, before, after);
    let model_map: HashMap<&ModelName, &ModelName> = models
        .iter()
        .filter_map(|rename| match rename {
            Rename::Model { from, to, .. } => Some((from, to)),
            _ => None,
        })
        .collect();

    // Fields of models present in both versions.
    let mut removed: BTreeMap<&ModelName, Vec<&FieldName>> = BTreeMap::new();
    let mut added: BTreeMap<&ModelName, Vec<&FieldName>> = BTreeMap::new();
    for change in changes {
        match change {
            Change::FieldRemoved { model, field, .. } => {
                removed.entry(model).or_default().push(field)
            }
            Change::FieldAdded { model, field, .. } => added.entry(model).or_default().push(field),
            _ => {}
        }
    }
    let mut fields = Vec::new();
    for (model, gone) in &removed {
        if let (Some(new), Some(old), Some(new_view)) =
            (added.get(model), before.get(*model), after.get(*model))
        {
            fields.extend(match_fields(
                model, model, gone, new, old, new_view, &model_map,
            ));
        }
    }
    // Fields of a renamed model, compared with the model it became.
    for (from, to) in &model_map {
        let (Some(old), Some(new)) = (before.get(*from), after.get(*to)) else {
            continue;
        };
        let gone: Vec<&FieldName> = old
            .fields
            .keys()
            .filter(|f| !new.fields.contains_key(*f))
            .copied()
            .collect();
        let fresh: Vec<&FieldName> = new
            .fields
            .keys()
            .filter(|f| !old.fields.contains_key(*f))
            .copied()
            .collect();
        fields.extend(match_fields(from, to, &gone, &fresh, old, new, &model_map));
    }

    found.extend(models);
    found.extend(fields);
    found
}

/// A removed module whose fields moved to one other module.
fn modules(changes: &[Change], target_had_module: impl Fn(&ModuleName) -> bool) -> Vec<Rename> {
    let removed: BTreeSet<&ModuleName> = changes
        .iter()
        .filter_map(|change| match change {
            Change::ModuleRemoved { module, .. } => Some(module),
            _ => None,
        })
        .collect();
    let mut moves: BTreeMap<(&ModuleName, &ModuleName), u32> = BTreeMap::new();
    for change in changes {
        if let Change::FieldMoved { from, to, .. } = change
            && removed.contains(from)
        {
            *moves.entry((from, to)).or_default() += 1;
        }
    }
    let mut best: BTreeMap<&ModuleName, (&ModuleName, u32)> = BTreeMap::new();
    for ((from, to), count) in moves {
        let entry = best.entry(from).or_insert((to, count));
        if count > entry.1 {
            *entry = (to, count);
        }
    }
    best.into_iter()
        .map(|(from, (to, count))| Rename::Module {
            from: from.clone(),
            to: to.clone(),
            merged: target_had_module(to),
            confidence: if count >= 2 {
                Confidence::High
            } else {
                Confidence::Medium
            },
            evidence: vec![Evidence::FieldsMoved(count)],
        })
        .collect()
}

/// A removed model and the model that continues it, matched by fields that
/// now point to the new one, by references swapped from one to the other,
/// and by the field names they share. The new model is usually added; it may
/// also exist already, when the old one was merged into it.
fn models(
    changes: &[Change],
    before: &BTreeMap<ModelName, ModelView<'_>>,
    after: &BTreeMap<ModelName, ModelView<'_>>,
) -> Vec<Rename> {
    let removed: BTreeSet<&ModelName> = changes
        .iter()
        .filter_map(|change| match change {
            Change::ModelRemoved { model, .. } => Some(model),
            _ => None,
        })
        .collect();
    let added: BTreeSet<&ModelName> = changes
        .iter()
        .filter_map(|change| match change {
            Change::ModelAdded { model, .. } => Some(model),
            _ => None,
        })
        .collect();

    let mut repointed: HashMap<(&ModelName, &ModelName), u32> = HashMap::new();
    for change in changes {
        if let Change::FieldComodelChanged { from, to, .. } = change
            && removed.contains(from)
        {
            *repointed.entry((from, to)).or_default() += 1;
        }
    }
    let swapped = swapped_references(changes, before, after, &removed);

    let mut scored: Vec<ModelPair<'_>> = Vec::new();
    for &from in &removed {
        let Some(old) = before.get(from) else {
            continue;
        };
        for (to, new) in after {
            let merged = !added.contains(to);
            let count = repointed.get(&(from, to)).copied().unwrap_or(0);
            let swaps = swapped.get(&(from, to)).copied().unwrap_or(0);
            // A merge target exists anyway, so it needs references as proof.
            if merged && count + swaps < 2 {
                continue;
            }
            let shared = shared_fields(old, new);
            let mut evidence = Vec::new();
            if count > 0 {
                evidence.push(Evidence::FieldsRepointed(count));
            }
            if swaps > 0 {
                evidence.push(Evidence::ReferencesSwapped(swaps));
            }
            if let Some(percent) = shared {
                evidence.push(Evidence::SharedFields(percent));
            }
            let references = count + swaps;
            let percent = shared.unwrap_or(0);
            let confidence = if references > 0 && percent >= 30 || percent >= 70 || references >= 3
            {
                Confidence::High
            } else if references > 0 || percent >= 50 {
                Confidence::Medium
            } else {
                continue;
            };
            let similar = similarity(from.as_str(), to.as_str());
            if similar >= 50 {
                evidence.push(Evidence::SimilarName(similar));
            }
            let strength = references * 100 + u32::from(percent) + u32::from(similar) / 10;
            scored.push(ModelPair {
                confidence,
                structural: !merged && percent >= 20,
                strength,
                from,
                to,
                merged,
                evidence,
            });
        }
    }
    // Strongest pairs first. Each removed model continues in one model at
    // most, and a renamed-to model takes over one model only; a merge target
    // may absorb several.
    // A rename to a new model with shared fields is the more specific claim,
    // so it is preferred to a merge; otherwise the stronger evidence wins.
    scored.sort_by(|a, b| {
        (b.structural, b.confidence, b.strength)
            .cmp(&(a.structural, a.confidence, a.strength))
            .then_with(|| (a.from, a.to).cmp(&(b.from, b.to)))
    });
    let (mut used_from, mut used_to) = (BTreeSet::new(), BTreeSet::new());
    let mut found = Vec::new();
    for ModelPair {
        confidence,
        from,
        to,
        merged,
        evidence,
        ..
    } in scored
    {
        if used_from.contains(from) || !merged && used_to.contains(to) {
            continue;
        }
        used_from.insert(from);
        used_to.insert(to);
        found.push(Rename::Model {
            from: from.clone(),
            to: to.clone(),
            merged,
            confidence,
            evidence,
        });
    }
    found.sort_by(|a, b| model_key(a).cmp(&model_key(b)));
    found
}

/// A scored pair of a removed model and the model that may continue it.
struct ModelPair<'m> {
    confidence: Confidence,
    /// A rename to a new model that shares a fair part of the old model's
    /// fields: preferred to a merge, which is the vaguer claim.
    structural: bool,
    strength: u32,
    from: &'m ModelName,
    to: &'m ModelName,
    merged: bool,
    evidence: Vec<Evidence>,
}

/// For each pair of a removed model and another model: on how many models a
/// field pointing to the removed one disappeared while a field pointing to
/// the other appeared, as `product_packaging_id` gave way to `product_uom_id`.
fn swapped_references<'m>(
    changes: &'m [Change],
    before: &'m BTreeMap<ModelName, ModelView<'_>>,
    after: &'m BTreeMap<ModelName, ModelView<'_>>,
    removed: &BTreeSet<&'m ModelName>,
) -> HashMap<(&'m ModelName, &'m ModelName), u32> {
    let mut gone: BTreeMap<&ModelName, BTreeSet<&ModelName>> = BTreeMap::new();
    let mut fresh: BTreeMap<&ModelName, BTreeSet<&ModelName>> = BTreeMap::new();
    for change in changes {
        match change {
            Change::FieldRemoved { model, field, .. } => {
                if let Some((comodel, _)) = before
                    .get(model)
                    .and_then(|v| v.fields.get(field))
                    .and_then(|f| f.comodel)
                    && removed.contains(comodel)
                {
                    gone.entry(model).or_default().insert(comodel);
                }
            }
            Change::FieldAdded { model, field, .. } => {
                if let Some((comodel, _)) = after
                    .get(model)
                    .and_then(|v| v.fields.get(field))
                    .and_then(|f| f.comodel)
                {
                    fresh.entry(model).or_default().insert(comodel);
                }
            }
            _ => {}
        }
    }
    let mut swapped: HashMap<(&ModelName, &ModelName), u32> = HashMap::new();
    for (model, olds) in &gone {
        let Some(news) = fresh.get(model) else {
            continue;
        };
        for old in olds {
            for new in news {
                if old != new {
                    *swapped.entry((*old, *new)).or_default() += 1;
                }
            }
        }
    }
    swapped
}

fn model_key(rename: &Rename) -> Option<&ModelName> {
    match rename {
        Rename::Model { from, .. } => Some(from),
        _ => None,
    }
}

/// Pairs removed and added fields of the same shape, best names first.
fn match_fields(
    model: &ModelName,
    to_model: &ModelName,
    gone: &[&FieldName],
    fresh: &[&FieldName],
    old: &ModelView<'_>,
    new: &ModelView<'_>,
    model_map: &HashMap<&ModelName, &ModelName>,
) -> Vec<Rename> {
    let same_shape = |a: &FieldView<'_>, b: &FieldView<'_>| {
        same_type(a.field_type, b.field_type)
            && match (a.comodel, b.comodel) {
                (Some((x, _)), Some((y, _))) => x == y || model_map.get(x).is_some_and(|z| *z == y),
                _ => true,
            }
    };
    let mut scored: Vec<(Confidence, u8, &FieldName, &FieldName, Vec<Evidence>)> = Vec::new();
    for &from in gone {
        let Some(a) = old.fields.get(from) else {
            continue;
        };
        let shaped: Vec<&FieldName> = fresh
            .iter()
            .copied()
            .filter(|to| new.fields.get(*to).is_some_and(|b| same_shape(a, b)))
            .collect();
        let only = shaped.len() == 1
            && gone
                .iter()
                .filter(|other| {
                    old.fields
                        .get(**other)
                        .is_some_and(|c| same_shape(c, &new.fields[shaped[0]]))
                })
                .count()
                == 1;
        for &to in &shaped {
            let similar = similarity(from.as_str(), to.as_str());
            let mut evidence = vec![Evidence::SameShape];
            let confidence = if same_stem(from.as_str(), to.as_str()) {
                evidence.insert(0, Evidence::SameStem);
                Confidence::High
            } else if similar >= 60 {
                evidence.insert(0, Evidence::SimilarName(similar));
                if only {
                    Confidence::High
                } else {
                    Confidence::Medium
                }
            } else if name_contained(from.as_str(), to.as_str()) {
                evidence.insert(0, Evidence::NameContained);
                Confidence::Medium
            } else if only {
                evidence.push(Evidence::OnlyCandidate);
                Confidence::Low
            } else {
                continue;
            };
            if only && !evidence.contains(&Evidence::OnlyCandidate) {
                evidence.push(Evidence::OnlyCandidate);
            }
            scored.push((confidence, similar, from, to, evidence));
        }
    }
    scored.sort_by(|a, b| {
        (b.0, b.1)
            .cmp(&(a.0, a.1))
            .then_with(|| (a.2, a.3).cmp(&(b.2, b.3)))
    });
    let (mut used_from, mut used_to) = (BTreeSet::new(), BTreeSet::new());
    let mut found = Vec::new();
    for (confidence, _, from, to, evidence) in scored {
        if used_from.insert(from) && used_to.insert(to) {
            found.push(Rename::Field {
                model: model.clone(),
                from: from.clone(),
                to_model: to_model.clone(),
                to: to.clone(),
                confidence,
                evidence,
            });
        }
    }
    found
}

/// Whether a field could have been renamed into the other: the same type,
/// or two relational types that both reach many records or that both point
/// from one record (`tax_id` Many2one to `tax_ids` Many2many, `move_ids`
/// One2many to Many2many). A Many2one and a One2many point opposite ways, so
/// they never match.
fn same_type(a: &FieldType, b: &FieldType) -> bool {
    use FieldType::{Many2many, Many2one, One2many};
    a == b
        || matches!(
            (a, b),
            (Many2one | One2many, Many2many) | (Many2many, Many2one | One2many)
        )
}

/// Whether the names match once `_id`/`_ids` and a plural ending are dropped.
fn same_stem(a: &str, b: &str) -> bool {
    let (a, b) = (stems(a), stems(b));
    a.iter().any(|stem| b.contains(stem))
}

fn stems(name: &str) -> Vec<String> {
    let base = name
        .strip_suffix("_ids")
        .or_else(|| name.strip_suffix("_id"))
        .unwrap_or(name);
    let mut stems = vec![base.to_string()];
    for ending in ["es", "s"] {
        if let Some(singular) = base.strip_suffix(ending) {
            stems.push(singular.to_string());
        }
    }
    // `groups_id` and `group_ids`: the plural may sit before the suffix.
    stems.extend(
        stems
            .clone()
            .into_iter()
            .map(|stem| stem.replace("s_", "_")),
    );
    stems
}

/// Whether one name, without `_id`/`_ids`, ends or starts with the other,
/// on a word boundary and with at least four letters in common.
fn name_contained(a: &str, b: &str) -> bool {
    let strip = |name: &str| {
        name.strip_suffix("_ids")
            .or_else(|| name.strip_suffix("_id"))
            .unwrap_or(name)
            .to_string()
    };
    let (a, b) = (strip(a), strip(b));
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    short.len() >= 4
        && short != long
        && (long.ends_with(&format!("_{short}")) || long.starts_with(&format!("{short}_")))
}

/// How alike two names are, from 0 to 100, by edit distance.
fn similarity(a: &str, b: &str) -> u8 {
    let longest = a.chars().count().max(b.chars().count());
    if longest == 0 {
        return 100;
    }
    let distance = levenshtein(a, b);
    u8::try_from(100 * (longest - distance.min(longest)) / longest).unwrap_or(0)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.chars().enumerate() {
        let mut current = vec![i + 1; b.len() + 1];
        for (j, y) in b.iter().enumerate() {
            current[j + 1] = (previous[j] + usize::from(x != *y))
                .min(previous[j + 1] + 1)
                .min(current[j] + 1);
        }
        previous = current;
    }
    previous[b.len()]
}

/// Share of field names the two models have in common, from 0 to 100,
/// leaving out the fields every model has. `None` when fewer than three
/// names are shared: two tiny models alike by chance prove nothing.
fn shared_fields(old: &ModelView<'_>, new: &ModelView<'_>) -> Option<u8> {
    const MAGIC: [&str; 6] = [
        "id",
        "display_name",
        "create_uid",
        "create_date",
        "write_uid",
        "write_date",
    ];
    let names = |view: &ModelView<'_>| -> BTreeSet<String> {
        view.fields
            .keys()
            .map(|f| f.as_str().to_string())
            .filter(|f| !MAGIC.contains(&f.as_str()))
            .collect()
    };
    let (a, b) = (names(old), names(new));
    let shared = a.intersection(&b).count();
    let union = a.union(&b).count();
    if shared < 3 || union == 0 {
        return None;
    }
    u8::try_from(100 * shared / union).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_ignore_id_suffixes_and_plurals() {
        for (a, b) in [
            ("tax_id", "tax_ids"),
            ("taxes_id", "tax_ids"),
            ("groups_id", "group_ids"),
            ("users", "user_ids"),
            ("notes", "note"),
            ("product_uom", "product_uom_id"),
        ] {
            assert!(same_stem(a, b), "{a} {b}");
        }
        assert!(!same_stem("partner_id", "parent_id"));
    }

    #[test]
    fn contained_names_need_a_word_boundary() {
        assert!(name_contained("stock_move_ids", "move_ids"));
        assert!(name_contained("date_start", "contract_date_start"));
        assert!(name_contained("amount", "amount_total"));
        assert!(!name_contained("note", "notebook"));
        assert!(!name_contained("id", "partner_id"));
    }

    #[test]
    fn similarity_is_a_percentage() {
        assert_eq!(similarity("same", "same"), 100);
        assert_eq!(similarity("abcd", "wxyz"), 0);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }

    #[test]
    fn many2one_and_one2many_never_match() {
        use FieldType::{Char, Many2many, Many2one, One2many};
        assert!(same_type(&Many2one, &Many2many));
        assert!(same_type(&One2many, &Many2many));
        assert!(!same_type(&Many2one, &One2many));
        assert!(!same_type(&Char, &Many2one));
    }
}
