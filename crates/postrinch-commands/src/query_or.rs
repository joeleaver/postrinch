//! `OR`, `{ }` and `( )` in the search language, as Gmail reads them:
//! `a OR b` and `{a b}` match either, `( )` groups, and `OR` binds tighter
//! than the space between terms (`x a OR b` is x and either a or b). An
//! operator before a group applies to each term in it (`from:(a OR b)`).
//!
//! A query is spread into alternatives, each a query without them, which
//! search runs one by one and joins.

use postrinch_model::Timestamp;

use crate::query::Query;
use crate::{CommandError, CommandResult};

/// A query read as its alternatives.
#[derive(Debug, Clone)]
pub(crate) struct Either {
    pub queries: Vec<Query>,
}

impl Either {
    pub fn parse(text: &str, now: Timestamp) -> CommandResult<Self> {
        let queries = alternatives(text)?
            .iter()
            .map(|t| Query::parse(t, now))
            .collect::<CommandResult<Vec<_>>>()?;
        Ok(Self { queries })
    }

    /// The words to mark in an excerpt, from every alternative.
    pub fn terms(&self) -> Vec<String> {
        let mut terms: Vec<String> = Vec::new();
        for term in self.queries.iter().flat_map(Query::terms) {
            if !terms.contains(&term) {
                terms.push(term);
            }
        }
        terms
    }

    pub fn has_words(&self) -> bool {
        self.queries.iter().any(Query::has_words)
    }

    /// What `last:` asks of every conversation found. Alternatives that
    /// ask different things are refused: what is found is joined before it
    /// is grouped into conversations, so it could not tell them apart.
    pub fn last(&self) -> CommandResult<Option<crate::query::Last>> {
        let mut asked = self.queries.iter().map(|q| q.last);
        let first = asked.next().flatten();
        if asked.any(|l| l != first) {
            return Err(CommandError::Invalid(
                "last: must be the same on both sides of OR.".into(),
            ));
        }
        Ok(first)
    }

    pub fn set_aside_too(&self) -> bool {
        self.queries.iter().any(|q| q.set_aside_too)
    }

    /// The query in Gmail's language.
    pub fn gmail(&self) -> String {
        match self.queries.as_slice() {
            [one] => one.gmail.clone(),
            many => many
                .iter()
                .map(|q| format!("({})", q.gmail))
                .collect::<Vec<_>>()
                .join(" OR "),
        }
    }
}

/// The most alternatives one query may spread into.
const MOST: usize = 64;

/// The query's alternatives, each as text with no `OR`, braces or
/// parentheses left in it. A query without them is its own one.
pub(crate) fn alternatives(text: &str) -> CommandResult<Vec<String>> {
    let tokens = split(text)?;
    let mut at = 0;
    let tree = sequence(&tokens, &mut at, None)?;
    if at < tokens.len() {
        return Err(invalid(format!("{} has nothing to close.", tokens[at])));
    }
    let spread = spread(&tree)?;
    Ok(spread.into_iter().map(|terms| terms.join(" ")).collect())
}

#[derive(Debug)]
enum Node {
    Term(String),
    All(Vec<Node>),
    Any(Vec<Node>),
}

/// Words, quoted phrases (kept whole, quotes and all), and the brackets
/// as tokens of their own.
fn split(text: &str) -> CommandResult<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            '(' | ')' | '{' | '}' if !quoted => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                tokens.push(c.to_string());
            }
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if quoted {
        return Err(invalid("A quote is not closed.".into()));
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Ok(tokens)
}

/// Terms up to `close` (or the end), joined by `OR` where it stands
/// between two. In braces, every term is an alternative.
fn sequence(tokens: &[String], at: &mut usize, close: Option<&str>) -> CommandResult<Node> {
    let mut items: Vec<Node> = Vec::new();
    let mut or_next = false;
    loop {
        let Some(token) = tokens.get(*at) else {
            if let Some(close) = close {
                return Err(invalid(format!("A bracket is not closed: add {close}.")));
            }
            break;
        };
        if Some(token.as_str()) == close {
            *at += 1;
            break;
        }
        if token == "OR" {
            if items.is_empty() || or_next {
                return Err(invalid("OR needs a term on each side.".into()));
            }
            or_next = true;
            *at += 1;
            continue;
        }
        let item = item(tokens, at)?;
        if or_next {
            or_next = false;
            match items.pop() {
                Some(Node::Any(mut either)) => {
                    either.push(item);
                    items.push(Node::Any(either));
                }
                Some(before) => items.push(Node::Any(vec![before, item])),
                None => unreachable!("an OR follows a term"),
            }
        } else {
            items.push(item);
        }
    }
    if or_next {
        return Err(invalid("OR needs a term on each side.".into()));
    }
    Ok(if close == Some("}") {
        Node::Any(items)
    } else {
        Node::All(items)
    })
}

fn item(tokens: &[String], at: &mut usize) -> CommandResult<Node> {
    let token = &tokens[*at];
    *at += 1;
    let group = |at: &mut usize, open: &str| -> CommandResult<Node> {
        let close = if open == "(" { ")" } else { "}" };
        sequence(tokens, at, Some(close))
    };
    match token.as_str() {
        "(" | "{" => group(at, token),
        ")" | "}" => Err(invalid(format!("{token} has nothing to close."))),
        _ => {
            // `from:(a b)` or `-{...}`: an operator or minus before a group.
            let opens = tokens.get(*at).is_some_and(|t| t == "(" || t == "{");
            if opens && (token.ends_with(':') || token == "-") {
                if token == "-" || token.starts_with('-') {
                    return Err(invalid(
                        "Leaving out a group with - is not supported; leave out each term.".into(),
                    ));
                }
                let open = tokens[*at].clone();
                *at += 1;
                return Ok(prefixed(token, group(at, &open)?));
            }
            Ok(Node::Term(token.clone()))
        }
    }
}

/// Put `prefix` (`from:`) before every term in a group.
fn prefixed(prefix: &str, node: Node) -> Node {
    match node {
        Node::Term(t) => Node::Term(format!("{prefix}{t}")),
        Node::All(n) => Node::All(n.into_iter().map(|n| prefixed(prefix, n)).collect()),
        Node::Any(n) => Node::Any(n.into_iter().map(|n| prefixed(prefix, n)).collect()),
    }
}

/// The node as alternatives, each a list of terms that must all match.
fn spread(node: &Node) -> CommandResult<Vec<Vec<String>>> {
    let out = match node {
        Node::Term(t) => vec![vec![t.clone()]],
        Node::Any(nodes) => {
            let mut out = Vec::new();
            for n in nodes {
                out.extend(spread(n)?);
            }
            out
        }
        Node::All(nodes) => {
            let mut out = vec![Vec::new()];
            for n in nodes {
                let each = spread(n)?;
                let mut next = Vec::with_capacity(out.len() * each.len());
                for have in &out {
                    for more in &each {
                        next.push([have.clone(), more.clone()].concat());
                    }
                }
                out = next;
                if out.len() > MOST {
                    break;
                }
            }
            out
        }
    };
    if out.len() > MOST {
        return Err(invalid(format!(
            "That search has too many alternatives (more than {MOST}): search for fewer at once."
        )));
    }
    Ok(out)
}

fn invalid(message: String) -> CommandError {
    CommandError::Invalid(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alts(text: &str) -> Vec<String> {
        alternatives(text).unwrap()
    }

    #[test]
    fn a_query_without_or_is_its_own_alternative() {
        assert_eq!(alts(r#"from:a "x OR y" z"#), [r#"from:a "x OR y" z"#]);
    }

    #[test]
    fn or_binds_tighter_than_and() {
        assert_eq!(alts("from:a OR from:b"), ["from:a", "from:b"]);
        assert_eq!(alts("x a OR b OR c"), ["x a", "x b", "x c"]);
        assert_eq!(
            alts("{from:a from:b} is:unread"),
            ["from:a is:unread", "from:b is:unread"]
        );
    }

    #[test]
    fn groups_and_operators_before_them() {
        assert_eq!(alts("(a b) OR c"), ["a b", "c"]);
        assert_eq!(
            alts("from:(a OR b) in:inbox"),
            ["from:a in:inbox", "from:b in:inbox"]
        );
        assert_eq!(alts("from:{a b}"), ["from:a", "from:b"]);
    }

    #[test]
    fn mistakes_say_what_is_wrong() {
        for bad in ["OR a", "a OR", "(a b", "a)", "-(a b)", "a OR OR b"] {
            assert!(alternatives(bad).is_err(), "{bad}");
        }
        let many = "{a b} {c d} {e f} {g h} {i j} {k l} {m n}";
        assert!(
            alternatives(many)
                .unwrap_err()
                .to_string()
                .contains("too many")
        );
    }
}
