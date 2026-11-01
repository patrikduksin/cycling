//! Locate moments in a timeline: every entry of a kind, the stretches in
//! which a shown field held a value, or entries that mention some text.
use crate::trace::timeline::{Entry, Kind, Moment, Timeline, duration, field, session_clock};
use std::fmt;

/// What to look for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Query {
    /// Entries of these kinds.
    Kinds(Vec<Kind>),
    /// Stretches in which a shown field's value matched.
    Field {
        name: String,
        test: Test,
        value: String,
    },
    /// Entries whose text contains this, ignoring case.
    Text(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Test {
    Is,
    IsNot,
    Contains,
}

impl Query {
    /// `power=--`, `hr!=--`, `speed~0.`; a kind name or list (`toast`,
    /// `page,toast`); anything else is text.
    pub fn parse(text: &str) -> Self {
        for (operator, test) in [("!=", Test::IsNot), ("=", Test::Is), ("~", Test::Contains)] {
            if let Some((name, value)) = text.split_once(operator)
                && !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphabetic())
            {
                return Self::Field {
                    name: name.into(),
                    test,
                    value: value.into(),
                };
            }
        }
        match Kind::parse_list(text) {
            Ok(kinds) => Self::Kinds(kinds),
            Err(_) => Self::Text(text.into()),
        }
    }
}

/// One answer: an entry, or a stretch of time.
#[derive(Clone, Debug, PartialEq)]
pub enum Found<'a> {
    Entry(&'a Entry),
    Stretch {
        from: Moment,
        /// When it stopped holding; none if it held to the end.
        to: Option<Moment>,
        /// The shown line that began it.
        first: &'a str,
    },
}

impl fmt::Display for Found<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Entry(entry) => write!(f, "{entry}"),
            Self::Stretch { from, to, first } => {
                write!(f, "{from}  stretch     ")?;
                match to {
                    Some(to) => write!(
                        f,
                        "until {} ({})",
                        session_clock(to.session_ms),
                        duration(to.session_ms - from.session_ms)
                    )?,
                    None => write!(f, "until the end")?,
                }
                write!(f, "  {first}")
            }
        }
    }
}

/// What `query` finds in `window` (session ms) of `timeline`; entries that
/// only repeat their source and bulky kinds not asked for by name stay out,
/// and `filter` keeps only entries whose text contains it.
pub fn find<'a>(
    timeline: &'a Timeline,
    query: &Query,
    window: (u64, u64),
    filter: Option<&str>,
) -> Vec<Found<'a>> {
    let in_window = |e: &&Entry| (window.0..=window.1).contains(&e.at.session_ms);
    let matches_filter =
        |e: &Entry| filter.is_none_or(|f| e.text.to_lowercase().contains(&f.to_lowercase()));
    match query {
        Query::Kinds(kinds) => timeline
            .entries
            .iter()
            .filter(in_window)
            .filter(|e| !e.repeat && kinds.contains(&e.kind) && matches_filter(e))
            .map(Found::Entry)
            .collect(),
        Query::Text(text) => {
            let text = text.to_lowercase();
            timeline
                .entries
                .iter()
                .filter(in_window)
                .filter(|e| !e.repeat && !e.kind.bulky())
                .filter(|e| e.text.to_lowercase().contains(&text) && matches_filter(e))
                .map(Found::Entry)
                .collect()
        }
        Query::Field { name, test, value } => {
            let holds = |shown: &str| {
                field(shown, name).is_some_and(|got| match test {
                    Test::Is => got == value,
                    Test::IsNot => got != value,
                    Test::Contains => got.contains(value.as_str()),
                })
            };
            let mut found = Vec::new();
            let mut since: Option<&Entry> = None;
            for entry in timeline.entries.iter().filter(in_window) {
                // Leaving the ride pages ends what they showed.
                let shown = match entry.kind {
                    Kind::Shown => Some(entry.text.as_str()),
                    Kind::Page if !entry.text.starts_with("ride/") => None,
                    _ => continue,
                };
                match (since, shown.is_some_and(holds)) {
                    (None, true) => since = Some(entry),
                    (Some(start), false) => {
                        found.push(Found::Stretch {
                            from: start.at,
                            to: Some(entry.at),
                            first: &start.text,
                        });
                        since = None;
                    }
                    _ => {}
                }
            }
            if let Some(start) = since {
                found.push(Found::Stretch {
                    from: start.at,
                    to: None,
                    first: &start.text,
                });
            }
            found
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_is_a_field_test_a_kind_or_text() {
        assert_eq!(
            Query::parse("power=--"),
            Query::Field {
                name: "power".into(),
                test: Test::Is,
                value: "--".into()
            }
        );
        assert!(matches!(
            Query::parse("hr!=--"),
            Query::Field {
                test: Test::IsNot,
                ..
            }
        ));
        assert_eq!(
            Query::parse("page,toast"),
            Query::Kinds(vec![Kind::Page, Kind::Toast])
        );
        assert_eq!(
            Query::parse("LOW BATTERY"),
            Query::Text("LOW BATTERY".into())
        );
        assert_eq!(Query::parse("a b=c"), Query::Text("a b=c".into()));
    }
}
