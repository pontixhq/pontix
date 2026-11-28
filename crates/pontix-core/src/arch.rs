//! View architectures as trees of elements, so that the views inheriting
//! them can be applied and their XPath targets looked up.

use crate::symbol::Location;

/// An element of a view architecture: its tag, its attributes in document
/// order, and its child elements. Text and comments are not kept: no check
/// reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    /// The tag, such as `field`, `xpath` or `t`.
    pub tag: String,
    /// The attributes, as written.
    pub attributes: Vec<(String, String)>,
    /// The child elements, in document order.
    pub children: Vec<Element>,
    /// Where the element is.
    pub location: Location,
}

impl Element {
    /// The value of an attribute.
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The element at a path of child indices from this one: `[]` is this
    /// element, `[0, 2]` the third child of its first child.
    pub fn at(&self, path: &[usize]) -> Option<&Element> {
        path.iter()
            .try_fold(self, |element, &index| element.children.get(index))
    }

    /// The number of elements in the tree, this one included.
    pub fn size(&self) -> usize {
        1 + self.children.iter().map(Element::size).sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::path::PathBuf;

    use super::*;

    fn element(tag: &str, attributes: &[(&str, &str)], children: Vec<Element>) -> Element {
        Element {
            tag: tag.to_string(),
            attributes: attributes
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            children,
            location: Location::new(PathBuf::from("views.xml"), NonZeroU32::MIN),
        }
    }

    #[test]
    fn elements_are_found_by_path_and_attribute() {
        let form = element(
            "form",
            &[],
            vec![element(
                "group",
                &[],
                vec![
                    element("field", &[("name", "partner_id")], vec![]),
                    element("field", &[("name", "date"), ("invisible", "1")], vec![]),
                ],
            )],
        );
        let date = form.at(&[0, 1]).expect("the second field");
        assert_eq!(date.attribute("name"), Some("date"));
        assert_eq!(date.attribute("invisible"), Some("1"));
        assert_eq!(date.attribute("string"), None);
        assert!(form.at(&[0, 2]).is_none());
        assert_eq!(form.at(&[]), Some(&form));
        assert_eq!(form.size(), 4);
    }
}
