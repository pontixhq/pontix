//! The XPath that Odoo views use to locate where an inheriting view applies
//! its changes, evaluated over a view architecture ([`Element`]).
//!
//! Odoo hands `<xpath expr>` to lxml, so any XPath 1.0 is allowed. Views use
//! a small part of it: paths of steps by tag, attribute tests, positions,
//! `..`, a few axes, `and` / `or`, and Odoo's own `hasclass()`. That part is
//! read here; anything else, and anything that needs the text of elements
//! (which architectures are kept without), is refused as
//! [`XPathError::Unsupported`], so that a caller can stay silent instead of
//! guessing.

use pontix_core::Element;

/// Where an XPath stops being read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XPathError {
    /// Not XPath at all.
    Invalid(String),
    /// XPath outside the part Pontix reads, such as `text()` or a union.
    Unsupported(String),
}

/// A parsed XPath expression.
#[derive(Debug, Clone, PartialEq)]
pub struct XPath(Expr);

/// An element of the architecture, by the indices of the children leading
/// to it from the root, or the document above the root.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Node {
    Document,
    Element(Vec<usize>),
}

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Or(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Equal(Box<Expr>, Box<Expr>, bool),
    Literal(String),
    Number(f64),
    Call(Function, Vec<Expr>),
    Path(Path),
    Filter(Box<Expr>, Vec<Expr>),
    Union(Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Function {
    HasClass,
    Contains,
    StartsWith,
    Not,
    Last,
    Position,
    NormalizeSpace,
    Count,
    LocalName,
    Concat,
}

#[derive(Debug, Clone, PartialEq)]
struct Path {
    absolute: bool,
    steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq)]
struct Step {
    axis: Axis,
    test: Test,
    predicates: Vec<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Child,
    Descendant,
    DescendantOrSelf,
    Parent,
    Ancestor,
    AncestorOrSelf,
    FollowingSibling,
    PrecedingSibling,
    Itself,
    Attribute,
}

#[derive(Debug, Clone, PartialEq)]
enum Test {
    Name(String),
    Any,
    Node,
}

/// What an expression gives.
enum Value {
    Nodes(Vec<Node>),
    Attributes(Vec<String>),
    Text(String),
    Number(f64),
    Boolean(bool),
}

impl XPath {
    /// Parses an expression.
    pub fn parse(text: &str) -> Result<Self, XPathError> {
        let tokens = tokenize(text)?;
        let mut parser = Parser { tokens, at: 0 };
        let expr = parser.or()?;
        if let Some(token) = parser.peek() {
            return Err(parser.unexpected(token));
        }
        // Decided here, not when evaluated, so that the answer does not
        // depend on which elements a view happens to hold.
        if reads_text(&expr) {
            return Err(XPathError::Unsupported(
                "the text of an element".to_string(),
            ));
        }
        Ok(Self(expr))
    }

    /// The elements it selects with `root` as the context, as lxml's
    /// `root.xpath(...)` would, in document order: each as the indices of
    /// the children from `root` to it ([`Element::at`]).
    pub fn select(&self, root: &Element) -> Result<Vec<Vec<usize>>, XPathError> {
        let context = Context {
            root,
            node: Node::Element(Vec::new()),
            position: 1,
            size: 1,
        };
        match context.eval(&self.0)? {
            Value::Nodes(nodes) => Ok(nodes
                .into_iter()
                .filter_map(|node| match node {
                    Node::Element(path) => Some(path),
                    Node::Document => None,
                })
                .collect()),
            _ => Err(XPathError::Unsupported(
                "an expression that selects no elements".to_string(),
            )),
        }
    }
}

/// Whether an expression compares or reads elements as text.
fn reads_text(expr: &Expr) -> bool {
    let elements = |expr: &Expr| match expr {
        Expr::Path(path) => path
            .steps
            .last()
            .is_none_or(|step| step.axis != Axis::Attribute),
        _ => false,
    };
    match expr {
        Expr::Or(left, right) | Expr::And(left, right) => reads_text(left) || reads_text(right),
        Expr::Equal(left, right, _) => {
            elements(left) || elements(right) || reads_text(left) || reads_text(right)
        }
        Expr::Call(function, arguments) => {
            let as_text = matches!(
                function,
                Function::HasClass
                    | Function::Contains
                    | Function::StartsWith
                    | Function::NormalizeSpace
                    | Function::Concat
            );
            arguments
                .iter()
                .any(|argument| (as_text && elements(argument)) || reads_text(argument))
        }
        Expr::Path(path) => path
            .steps
            .iter()
            .flat_map(|step| &step.predicates)
            .any(reads_text),
        Expr::Filter(inner, predicates) => reads_text(inner) || predicates.iter().any(reads_text),
        Expr::Union(left, right) => reads_text(left) || reads_text(right),
        Expr::Literal(_) | Expr::Number(_) => false,
    }
}

// ---------------------------------------------------------------- tokens

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Slash,
    DoubleSlash,
    Open,
    Close,
    OpenParen,
    CloseParen,
    At,
    Comma,
    Equal,
    NotEqual,
    Dot,
    DoubleDot,
    Star,
    Pipe,
    Axis(String),
    Name(String),
    Literal(String),
    Number(f64),
}

fn tokenize(text: &str) -> Result<Vec<Token>, XPathError> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0;
    while let Some(&c) = chars.get(at) {
        let next = chars.get(at + 1).copied();
        let (token, width) = match c {
            ' ' | '\t' | '\n' | '\r' => {
                at += 1;
                continue;
            }
            '/' if next == Some('/') => (Token::DoubleSlash, 2),
            '/' => (Token::Slash, 1),
            '[' => (Token::Open, 1),
            ']' => (Token::Close, 1),
            '(' => (Token::OpenParen, 1),
            ')' => (Token::CloseParen, 1),
            '@' => (Token::At, 1),
            ',' => (Token::Comma, 1),
            '=' => (Token::Equal, 1),
            '!' if next == Some('=') => (Token::NotEqual, 2),
            '.' if next == Some('.') => (Token::DoubleDot, 2),
            '.' if !next.is_some_and(|c| c.is_ascii_digit()) => (Token::Dot, 1),
            '*' => (Token::Star, 1),
            '|' => (Token::Pipe, 1),
            '\'' | '"' => {
                let end = chars[at + 1..]
                    .iter()
                    .position(|&other| other == c)
                    .ok_or_else(|| XPathError::Invalid("an unclosed string".to_string()))?;
                let literal: String = chars[at + 1..at + 1 + end].iter().collect();
                (Token::Literal(literal), end + 2)
            }
            c if c.is_ascii_digit() || c == '.' => {
                let width = chars[at..]
                    .iter()
                    .take_while(|c| c.is_ascii_digit() || **c == '.')
                    .count();
                let number: String = chars[at..at + width].iter().collect();
                let number = number
                    .parse()
                    .map_err(|_| XPathError::Invalid(format!("number {number}")))?;
                (Token::Number(number), width)
            }
            c if is_name_start(c) => {
                let width = chars[at..].iter().take_while(|c| is_name_char(**c)).count();
                let name: String = chars[at..at + width].iter().collect();
                if chars.get(at + width) == Some(&':') && chars.get(at + width + 1) == Some(&':') {
                    (Token::Axis(name), width + 2)
                } else {
                    (Token::Name(name), width)
                }
            }
            other => return Err(XPathError::Invalid(format!("character {other:?}"))),
        };
        tokens.push(token);
        at += width;
    }
    Ok(tokens)
}

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.')
}

// ---------------------------------------------------------------- parser

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        token
    }

    fn eat(&mut self, token: &Token) -> bool {
        if self.peek() == Some(token) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &Token) -> Result<(), XPathError> {
        match self.next() {
            Some(found) if &found == token => Ok(()),
            Some(found) => Err(self.unexpected(&found)),
            None => Err(XPathError::Invalid("an expression cut short".to_string())),
        }
    }

    fn unexpected(&self, token: &Token) -> XPathError {
        match token {
            Token::Pipe => XPathError::Unsupported("a union".to_string()),
            other => XPathError::Invalid(format!("unexpected {other:?}")),
        }
    }

    /// A name that is an operator here, not a step: `and`, `or`.
    fn keyword(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Token::Name(name)) if name == word)
    }

    fn or(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.and()?;
        while self.keyword("or") {
            self.at += 1;
            left = Expr::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.equality()?;
        while self.keyword("and") {
            self.at += 1;
            left = Expr::And(Box::new(left), Box::new(self.equality()?));
        }
        Ok(left)
    }

    fn equality(&mut self) -> Result<Expr, XPathError> {
        let left = self.union()?;
        let equal = match self.peek() {
            Some(Token::Equal) => true,
            Some(Token::NotEqual) => false,
            _ => return Ok(left),
        };
        self.at += 1;
        let right = self.union()?;
        Ok(Expr::Equal(Box::new(left), Box::new(right), equal))
    }

    fn union(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.primary()?;
        while self.eat(&Token::Pipe) {
            left = Expr::Union(Box::new(left), Box::new(self.primary()?));
        }
        Ok(left)
    }

    fn primary(&mut self) -> Result<Expr, XPathError> {
        match self.peek().cloned() {
            Some(Token::Literal(literal)) => {
                self.at += 1;
                Ok(Expr::Literal(literal))
            }
            Some(Token::Number(number)) => {
                self.at += 1;
                Ok(Expr::Number(number))
            }
            Some(Token::OpenParen) => {
                self.at += 1;
                let inner = self.or()?;
                self.expect(&Token::CloseParen)?;
                // `(//field)[2]`: predicates over the whole set, in
                // document order.
                let mut predicates = Vec::new();
                while self.eat(&Token::Open) {
                    predicates.push(self.or()?);
                    self.expect(&Token::Close)?;
                }
                if matches!(self.peek(), Some(Token::Slash | Token::DoubleSlash)) {
                    return Err(XPathError::Unsupported(
                        "a path after a parenthesized expression".to_string(),
                    ));
                }
                if predicates.is_empty() {
                    Ok(inner)
                } else {
                    Ok(Expr::Filter(Box::new(inner), predicates))
                }
            }
            Some(Token::Name(name))
                if self.tokens.get(self.at + 1) == Some(&Token::OpenParen)
                    && !matches!(name.as_str(), "node" | "text") =>
            {
                self.call(&name)
            }
            _ => self.path().map(Expr::Path),
        }
    }

    fn call(&mut self, name: &str) -> Result<Expr, XPathError> {
        let function = match name {
            "hasclass" => Function::HasClass,
            "contains" => Function::Contains,
            "starts-with" => Function::StartsWith,
            "not" => Function::Not,
            "last" => Function::Last,
            "position" => Function::Position,
            "normalize-space" => Function::NormalizeSpace,
            "count" => Function::Count,
            "local-name" | "name" => Function::LocalName,
            "concat" => Function::Concat,
            other => return Err(XPathError::Unsupported(format!("function {other}()"))),
        };
        self.at += 2;
        let mut arguments = Vec::new();
        if !self.eat(&Token::CloseParen) {
            loop {
                arguments.push(self.or()?);
                if self.eat(&Token::CloseParen) {
                    break;
                }
                self.expect(&Token::Comma)?;
            }
        }
        Ok(Expr::Call(function, arguments))
    }

    fn path(&mut self) -> Result<Path, XPathError> {
        let mut steps = Vec::new();
        let absolute = match self.peek() {
            Some(Token::Slash) => {
                self.at += 1;
                // `/` alone is the document.
                if !self.starts_step() {
                    return Ok(Path {
                        absolute: true,
                        steps,
                    });
                }
                true
            }
            Some(Token::DoubleSlash) => {
                self.at += 1;
                steps.push(descendant_or_self());
                true
            }
            _ => false,
        };
        steps.push(self.step()?);
        loop {
            match self.peek() {
                Some(Token::Slash) => self.at += 1,
                Some(Token::DoubleSlash) => {
                    self.at += 1;
                    steps.push(descendant_or_self());
                }
                _ => break,
            }
            steps.push(self.step()?);
        }
        Ok(Path { absolute, steps })
    }

    fn starts_step(&self) -> bool {
        matches!(
            self.peek(),
            Some(
                Token::Name(_)
                    | Token::Axis(_)
                    | Token::Star
                    | Token::At
                    | Token::Dot
                    | Token::DoubleDot
            )
        )
    }

    fn step(&mut self) -> Result<Step, XPathError> {
        let (axis, test) = match self.peek().cloned() {
            Some(Token::Dot) => {
                self.at += 1;
                return Ok(simple(Axis::Itself, Test::Node));
            }
            Some(Token::DoubleDot) => {
                self.at += 1;
                return Ok(simple(Axis::Parent, Test::Node));
            }
            Some(Token::At) => {
                self.at += 1;
                (Axis::Attribute, self.test()?)
            }
            Some(Token::Axis(name)) => {
                self.at += 1;
                let axis = match name.as_str() {
                    "child" => Axis::Child,
                    "descendant" => Axis::Descendant,
                    "descendant-or-self" => Axis::DescendantOrSelf,
                    "parent" => Axis::Parent,
                    "ancestor" => Axis::Ancestor,
                    "ancestor-or-self" => Axis::AncestorOrSelf,
                    "following-sibling" => Axis::FollowingSibling,
                    "preceding-sibling" => Axis::PrecedingSibling,
                    "self" => Axis::Itself,
                    "attribute" => Axis::Attribute,
                    other => return Err(XPathError::Unsupported(format!("axis {other}"))),
                };
                (axis, self.test()?)
            }
            _ => (Axis::Child, self.test()?),
        };
        let mut predicates = Vec::new();
        while self.eat(&Token::Open) {
            predicates.push(self.or()?);
            self.expect(&Token::Close)?;
        }
        Ok(Step {
            axis,
            test,
            predicates,
        })
    }

    fn test(&mut self) -> Result<Test, XPathError> {
        match self.next() {
            Some(Token::Star) => Ok(Test::Any),
            Some(Token::Name(name)) if self.peek() == Some(&Token::OpenParen) => {
                self.at += 1;
                self.expect(&Token::CloseParen)?;
                match name.as_str() {
                    "node" => Ok(Test::Node),
                    other => Err(XPathError::Unsupported(format!("{other}()"))),
                }
            }
            Some(Token::Name(name)) => Ok(Test::Name(name)),
            Some(token) => Err(self.unexpected(&token)),
            None => Err(XPathError::Invalid("an expression cut short".to_string())),
        }
    }
}

fn simple(axis: Axis, test: Test) -> Step {
    Step {
        axis,
        test,
        predicates: Vec::new(),
    }
}

fn descendant_or_self() -> Step {
    simple(Axis::DescendantOrSelf, Test::Node)
}

// ---------------------------------------------------------------- evaluation

struct Context<'e> {
    root: &'e Element,
    node: Node,
    position: usize,
    size: usize,
}

impl<'e> Context<'e> {
    fn with(&self, node: Node, position: usize, size: usize) -> Context<'e> {
        Context {
            root: self.root,
            node,
            position,
            size,
        }
    }

    fn element(&self, node: &Node) -> Option<&'e Element> {
        match node {
            Node::Document => None,
            Node::Element(path) => self.root.at(path),
        }
    }

    fn eval(&self, expr: &Expr) -> Result<Value, XPathError> {
        Ok(match expr {
            Expr::Or(left, right) => Value::Boolean(self.truth(left)? || self.truth(right)?),
            Expr::And(left, right) => Value::Boolean(self.truth(left)? && self.truth(right)?),
            Expr::Equal(left, right, equal) => {
                let same = equals(&self.eval(left)?, &self.eval(right)?)?;
                Value::Boolean(same == *equal)
            }
            Expr::Literal(literal) => Value::Text(literal.clone()),
            Expr::Number(number) => Value::Number(*number),
            Expr::Call(function, arguments) => self.call(*function, arguments)?,
            Expr::Path(path) => self.path(path)?,
            Expr::Union(left, right) => match (self.eval(left)?, self.eval(right)?) {
                (Value::Nodes(mut left), Value::Nodes(right)) => {
                    left.extend(right);
                    left.sort();
                    left.dedup();
                    Value::Nodes(left)
                }
                _ => return Err(XPathError::Invalid("a union of no elements".to_string())),
            },
            Expr::Filter(inner, predicates) => match self.eval(inner)? {
                Value::Nodes(nodes) => Value::Nodes(self.filter(nodes, predicates)?),
                _ => {
                    return Err(XPathError::Unsupported(
                        "a filter over no elements".to_string(),
                    ));
                }
            },
        })
    }

    fn truth(&self, expr: &Expr) -> Result<bool, XPathError> {
        Ok(match self.eval(expr)? {
            Value::Nodes(nodes) => !nodes.is_empty(),
            Value::Attributes(values) => !values.is_empty(),
            Value::Text(text) => !text.is_empty(),
            Value::Number(number) => number != 0.0 && !number.is_nan(),
            Value::Boolean(truth) => truth,
        })
    }

    fn text(&self, expr: &Expr) -> Result<String, XPathError> {
        text(self.eval(expr)?)
    }

    fn call(&self, function: Function, arguments: &[Expr]) -> Result<Value, XPathError> {
        let arity = |count: usize| {
            if arguments.len() == count {
                Ok(())
            } else {
                Err(XPathError::Invalid(format!(
                    "{function:?} takes {count} argument(s)"
                )))
            }
        };
        Ok(match function {
            Function::HasClass => {
                let classes = self
                    .element(&self.node)
                    .and_then(|element| element.attribute("class"))
                    .unwrap_or_default();
                let mut wanted = Vec::new();
                for argument in arguments {
                    wanted.push(self.text(argument)?);
                }
                Value::Boolean(
                    !wanted.is_empty()
                        && wanted
                            .iter()
                            .all(|class| classes.split_whitespace().any(|own| own == class)),
                )
            }
            Function::Contains => {
                arity(2)?;
                Value::Boolean(
                    self.text(&arguments[0])?
                        .contains(&self.text(&arguments[1])?),
                )
            }
            Function::StartsWith => {
                arity(2)?;
                let start = self.text(&arguments[1])?;
                Value::Boolean(self.text(&arguments[0])?.starts_with(&start))
            }
            Function::Not => {
                arity(1)?;
                Value::Boolean(!self.truth(&arguments[0])?)
            }
            Function::Last => {
                arity(0)?;
                Value::Number(self.size as f64)
            }
            Function::Position => {
                arity(0)?;
                Value::Number(self.position as f64)
            }
            Function::NormalizeSpace => {
                arity(1)?;
                let text = self.text(&arguments[0])?;
                Value::Text(text.split_whitespace().collect::<Vec<_>>().join(" "))
            }
            Function::LocalName => {
                let node = match arguments {
                    [] => Some(self.node.clone()),
                    [argument] => match self.eval(argument)? {
                        Value::Nodes(nodes) => nodes.into_iter().next(),
                        _ => {
                            return Err(XPathError::Invalid(
                                "local-name() of no elements".to_string(),
                            ));
                        }
                    },
                    _ => return Err(XPathError::Invalid("local-name() takes one".to_string())),
                };
                let tag = node.as_ref().and_then(|node| self.element(node));
                Value::Text(tag.map(|element| element.tag.clone()).unwrap_or_default())
            }
            Function::Concat => {
                let mut joined = String::new();
                for argument in arguments {
                    joined.push_str(&self.text(argument)?);
                }
                Value::Text(joined)
            }
            Function::Count => {
                arity(1)?;
                match self.eval(&arguments[0])? {
                    Value::Nodes(nodes) => Value::Number(nodes.len() as f64),
                    Value::Attributes(values) => Value::Number(values.len() as f64),
                    _ => return Err(XPathError::Invalid("count() of no nodes".to_string())),
                }
            }
        })
    }

    fn path(&self, path: &Path) -> Result<Value, XPathError> {
        let mut nodes = vec![if path.absolute {
            Node::Document
        } else {
            self.node.clone()
        }];
        for (index, step) in path.steps.iter().enumerate() {
            if step.axis == Axis::Attribute {
                // Attributes end a path: they have no children.
                if index + 1 != path.steps.len() || !step.predicates.is_empty() {
                    return Err(XPathError::Unsupported(
                        "a path through an attribute".to_string(),
                    ));
                }
                let values = nodes
                    .iter()
                    .filter_map(|node| self.element(node))
                    .flat_map(|element| {
                        element
                            .attributes
                            .iter()
                            .filter(|(name, _)| match &step.test {
                                Test::Name(wanted) => name == wanted,
                                Test::Any | Test::Node => true,
                            })
                            .map(|(_, value)| value.clone())
                    })
                    .collect();
                return Ok(Value::Attributes(values));
            }
            let mut next = Vec::new();
            for node in &nodes {
                next.extend(self.step(node, step)?);
            }
            next.sort();
            next.dedup();
            nodes = next;
        }
        Ok(Value::Nodes(nodes))
    }

    /// The nodes one step selects from one node, after its predicates.
    fn step(&self, from: &Node, step: &Step) -> Result<Vec<Node>, XPathError> {
        // In proximity order: reverse document order on reverse axes.
        let found: Vec<Node> = self
            .axis(from, step.axis)
            .into_iter()
            .filter(|node| self.matches(node, &step.test))
            .collect();
        self.filter(found, &step.predicates)
    }

    /// The nodes each predicate keeps, in turn, positions counted in the
    /// order given.
    fn filter(&self, mut found: Vec<Node>, predicates: &[Expr]) -> Result<Vec<Node>, XPathError> {
        for predicate in predicates {
            let size = found.len();
            let mut kept = Vec::new();
            for (index, node) in found.into_iter().enumerate() {
                let context = self.with(node.clone(), index + 1, size);
                let keep = match context.eval(predicate)? {
                    Value::Number(number) => number == (index + 1) as f64,
                    Value::Nodes(nodes) => !nodes.is_empty(),
                    Value::Attributes(values) => !values.is_empty(),
                    Value::Text(text) => !text.is_empty(),
                    Value::Boolean(truth) => truth,
                };
                if keep {
                    kept.push(node);
                }
            }
            found = kept;
        }
        Ok(found)
    }

    fn matches(&self, node: &Node, test: &Test) -> bool {
        match (test, self.element(node)) {
            (Test::Node, _) => true,
            (Test::Any, element) => element.is_some(),
            (Test::Name(name), Some(element)) => &element.tag == name,
            (Test::Name(_), None) => false,
        }
    }

    fn children(&self, node: &Node) -> Vec<Node> {
        match node {
            Node::Document => vec![Node::Element(Vec::new())],
            Node::Element(path) => {
                let count = self
                    .root
                    .at(path)
                    .map_or(0, |element| element.children.len());
                (0..count)
                    .map(|index| {
                        let mut child = path.clone();
                        child.push(index);
                        Node::Element(child)
                    })
                    .collect()
            }
        }
    }

    fn descendants(&self, node: &Node, found: &mut Vec<Node>) {
        for child in self.children(node) {
            found.push(child.clone());
            self.descendants(&child, found);
        }
    }

    fn parent(node: &Node) -> Option<Node> {
        match node {
            Node::Document => None,
            Node::Element(path) if path.is_empty() => Some(Node::Document),
            Node::Element(path) => Some(Node::Element(path[..path.len() - 1].to_vec())),
        }
    }

    fn axis(&self, from: &Node, axis: Axis) -> Vec<Node> {
        match axis {
            Axis::Child => self.children(from),
            Axis::Descendant => {
                let mut found = Vec::new();
                self.descendants(from, &mut found);
                found
            }
            Axis::DescendantOrSelf => {
                let mut found = vec![from.clone()];
                self.descendants(from, &mut found);
                found
            }
            Axis::Parent => Self::parent(from).into_iter().collect(),
            Axis::Ancestor | Axis::AncestorOrSelf => {
                let mut found = Vec::new();
                if axis == Axis::AncestorOrSelf {
                    found.push(from.clone());
                }
                let mut node = from.clone();
                while let Some(parent) = Self::parent(&node) {
                    found.push(parent.clone());
                    node = parent;
                }
                found
            }
            Axis::FollowingSibling | Axis::PrecedingSibling => {
                let Node::Element(path) = from else {
                    return Vec::new();
                };
                let Some((&index, parent)) = path.split_last() else {
                    return Vec::new();
                };
                let count = self
                    .root
                    .at(parent)
                    .map_or(0, |element| element.children.len());
                let sibling = |at: usize| {
                    let mut sibling = parent.to_vec();
                    sibling.push(at);
                    Node::Element(sibling)
                };
                if axis == Axis::FollowingSibling {
                    (index + 1..count).map(sibling).collect()
                } else {
                    (0..index).rev().map(sibling).collect()
                }
            }
            Axis::Itself => vec![from.clone()],
            Axis::Attribute => Vec::new(),
        }
    }
}

fn text(value: Value) -> Result<String, XPathError> {
    match value {
        Value::Text(text) => Ok(text),
        Value::Attributes(values) => Ok(values.into_iter().next().unwrap_or_default()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Boolean(truth) => Ok(truth.to_string()),
        Value::Nodes(_) => Err(XPathError::Unsupported(
            "the text of an element".to_string(),
        )),
    }
}

fn equals(left: &Value, right: &Value) -> Result<bool, XPathError> {
    Ok(match (left, right) {
        (Value::Nodes(_), _) | (_, Value::Nodes(_)) => {
            return Err(XPathError::Unsupported(
                "the text of an element".to_string(),
            ));
        }
        (Value::Attributes(values), Value::Text(text))
        | (Value::Text(text), Value::Attributes(values)) => {
            values.iter().any(|value| value == text)
        }
        (Value::Attributes(values), Value::Number(number))
        | (Value::Number(number), Value::Attributes(values)) => values
            .iter()
            .any(|value| value.trim().parse::<f64>() == Ok(*number)),
        (Value::Attributes(left), Value::Attributes(right)) => {
            left.iter().any(|value| right.contains(value))
        }
        (Value::Boolean(truth), other) | (other, Value::Boolean(truth)) => {
            let other = match other {
                Value::Attributes(values) => !values.is_empty(),
                Value::Text(text) => !text.is_empty(),
                Value::Number(number) => *number != 0.0,
                Value::Boolean(other) => *other,
                Value::Nodes(_) => unreachable!("handled above"),
            };
            *truth == other
        }
        (Value::Number(left), Value::Number(right)) => left == right,
        (Value::Text(left), Value::Text(right)) => left == right,
        (Value::Number(number), Value::Text(text)) | (Value::Text(text), Value::Number(number)) => {
            text.trim().parse::<f64>() == Ok(*number)
        }
    })
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use pontix_core::Location;

    use super::*;

    /// A tree from `tag[attr=value,...](children)`, written compactly.
    fn el(tag: &str, attributes: &[(&str, &str)], children: Vec<Element>) -> Element {
        Element {
            tag: tag.to_string(),
            attributes: attributes
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            children,
            location: Location::new("views.xml", NonZeroU32::MIN),
        }
    }

    fn form() -> Element {
        el(
            "form",
            &[],
            vec![
                el(
                    "header",
                    &[],
                    vec![
                        el(
                            "button",
                            &[("name", "action_confirm"), ("class", "oe_highlight btn")],
                            vec![],
                        ),
                        el("button", &[("name", "action_cancel")], vec![]),
                    ],
                ),
                el(
                    "sheet",
                    &[],
                    vec![
                        el(
                            "group",
                            &[("name", "main")],
                            vec![
                                el("field", &[("name", "partner_id")], vec![]),
                                el("field", &[("name", "date_order")], vec![]),
                            ],
                        ),
                        el(
                            "notebook",
                            &[],
                            vec![el(
                                "page",
                                &[("name", "lines")],
                                vec![el(
                                    "field",
                                    &[("name", "order_line")],
                                    vec![el(
                                        "list",
                                        &[],
                                        vec![el("field", &[("name", "product_id")], vec![])],
                                    )],
                                )],
                            )],
                        ),
                    ],
                ),
            ],
        )
    }

    fn select(expr: &str) -> Vec<Vec<usize>> {
        XPath::parse(expr)
            .unwrap_or_else(|error| panic!("{expr}: {error:?}"))
            .select(&form())
            .unwrap_or_else(|error| panic!("{expr}: {error:?}"))
    }

    #[test]
    fn paths_select_elements_as_lxml_does() {
        let cases: [(&str, &[&[usize]]); 18] = [
            ("//field[@name='partner_id']", &[&[1, 0, 0]]),
            ("//field[@name=\"date_order\"]", &[&[1, 0, 1]]),
            ("/form/sheet/group", &[&[1, 0]]),
            ("/list", &[]),
            ("sheet/group[@name='main']", &[&[1, 0]]),
            ("//group/field[2]", &[&[1, 0, 1]]),
            ("//group/field[last()]", &[&[1, 0, 1]]),
            ("//field[@name='partner_id']/..", &[&[1, 0]]),
            (
                "//field[@name='order_line']/list/field",
                &[&[1, 1, 0, 0, 0, 0]],
            ),
            ("//button[hasclass('oe_highlight')]", &[&[0, 0]]),
            ("//button[hasclass('btn', 'oe_highlight')]", &[&[0, 0]]),
            ("//button[hasclass('oe')]", &[]),
            (
                "//button[@name='action_confirm' or @name='action_cancel']",
                &[&[0, 0], &[0, 1]],
            ),
            ("//button[@name and not(@class)]", &[&[0, 1]]),
            (
                "//field[@name='partner_id']/following-sibling::field",
                &[&[1, 0, 1]],
            ),
            (
                "//field[@name='date_order']/preceding-sibling::*[1]",
                &[&[1, 0, 0]],
            ),
            (
                "//field[@name='product_id']/ancestor::page[1]",
                &[&[1, 1, 0]],
            ),
            (
                "//*[contains(@name, 'order')]",
                &[&[1, 0, 1], &[1, 1, 0, 0]],
            ),
        ];
        for (expr, expected) in cases {
            let expected: Vec<Vec<usize>> = expected.iter().map(|path| path.to_vec()).collect();
            assert_eq!(select(expr), expected, "{expr}");
        }
        assert_eq!(select("."), [Vec::<usize>::new()]);
        assert_eq!(select("//field").len(), 4);
        assert_eq!(select("(//field)").len(), 4);
        assert_eq!(select("(//field)[2]"), [vec![1, 0, 1]]);
        assert_eq!(select("//header | //group"), [vec![0], vec![1, 0]]);
        assert_eq!(select("//*[local-name()='notebook']"), [vec![1, 1]]);
        assert_eq!(
            select("//*[@name=concat('date', '_order')]"),
            [vec![1, 0, 1]]
        );
        assert_eq!(select("(//group/field)[last()]"), [vec![1, 0, 1]]);
        assert_eq!(
            select("//field[1]").len(),
            3,
            "the first field of each parent"
        );
    }

    #[test]
    fn what_cannot_be_read_is_refused_as_unsupported() {
        for expr in [
            "//field[text()='x']",
            "//label[.='Name']",
            "//field[translate(@name, 'a', 'b')]",
            "//field/following::button",
        ] {
            let result = XPath::parse(expr).and_then(|xpath| xpath.select(&form()));
            assert!(
                matches!(result, Err(XPathError::Unsupported(_))),
                "{expr}: {result:?}"
            );
        }
        for expr in ["//field[@name='x'", "//field[@name='x]", "//", "#"] {
            assert!(
                matches!(XPath::parse(expr), Err(XPathError::Invalid(_))),
                "{expr}"
            );
        }
    }
}
