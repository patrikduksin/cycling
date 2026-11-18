//! Delta debugging: the smallest part of a failing input that still fails.
//! `trace shrink` runs it over a fixture's passes, then its records; the
//! fuzzer runs it over a failing plan's steps.
use std::time::Instant;

/// What [`ddmin`] kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shrunk<T> {
    pub kept: Vec<T>,
    /// Removing any one item more makes it pass or invalid. False when the
    /// deadline came first.
    pub minimal: bool,
    /// Candidates tried.
    pub tried: usize,
}

/// Zeller's ddmin over `items`, which `reproduces` must hold of. Each
/// candidate keeps the items' order. A candidate it holds of replaces the
/// current set at once, so a caller that saves each one keeps its best when
/// cut short.
pub fn ddmin<T: Clone>(
    items: Vec<T>,
    deadline: Option<Instant>,
    mut reproduces: impl FnMut(&[T]) -> bool,
) -> Shrunk<T> {
    let mut current = items;
    let mut tried = 0;
    let mut n = 2;
    let late = || deadline.is_some_and(|deadline| Instant::now() >= deadline);
    loop {
        if current.is_empty() {
            return Shrunk {
                kept: current,
                minimal: true,
                tried,
            };
        }
        let chunks = even_runs(&current, n);
        let mut reduced = None;
        let chunks_are_complements = n == 2;
        for chunk in chunks.iter().filter(|_| !chunks_are_complements) {
            if late() {
                break;
            }
            tried += 1;
            if reproduces(chunk) {
                reduced = Some((chunk.clone(), 2));
                break;
            }
        }
        if reduced.is_none() {
            for skipped in 0..chunks.len() {
                if late() {
                    break;
                }
                let complement: Vec<T> = (chunks.iter().enumerate())
                    .filter(|(index, _)| *index != skipped)
                    .flat_map(|(_, chunk)| chunk.iter().cloned())
                    .collect();
                tried += 1;
                if reproduces(&complement) {
                    reduced = Some((complement, (n - 1).max(2)));
                    break;
                }
            }
        }
        match reduced {
            Some((smaller, next)) => {
                current = smaller;
                n = next.min(current.len().max(2));
            }
            None if late() => break,
            None if n >= current.len() => {
                return Shrunk {
                    kept: current,
                    minimal: true,
                    tried,
                };
            }
            None => n = (n * 2).min(current.len()),
        }
    }
    Shrunk {
        kept: current,
        minimal: false,
        tried,
    }
}

fn even_runs<T: Clone>(items: &[T], n: usize) -> Vec<Vec<T>> {
    let n = n.min(items.len()).max(1);
    (0..n)
        .map(|index| items[index * items.len() / n..(index + 1) * items.len() / n].to_vec())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holds_3_and_7(items: &[u32]) -> bool {
        items.contains(&3) && items.contains(&7)
    }

    #[test]
    fn it_finds_the_items_the_failure_needs_and_proves_each_is_needed() {
        let shrunk = ddmin((0..40).collect(), None, holds_3_and_7);
        assert_eq!(shrunk.kept, [3, 7]);
        assert!(shrunk.minimal);
        for skip in 0..shrunk.kept.len() {
            let mut fewer = shrunk.kept.clone();
            fewer.remove(skip);
            assert!(!holds_3_and_7(&fewer), "1-minimal");
        }
    }

    #[test]
    fn an_order_the_failure_needs_is_kept() {
        let ordered = |items: &[u32]| {
            let at = |v| items.iter().position(|i| *i == v);
            matches!((at(9), at(2)), (Some(a), Some(b)) if a < b)
        };
        let shrunk = ddmin(vec![5, 9, 1, 2, 8], None, ordered);
        assert_eq!(shrunk.kept, [9, 2]);
    }

    #[test]
    fn a_failure_that_needs_nothing_shrinks_to_nothing() {
        let shrunk = ddmin(vec![1, 2, 3], None, |_| true);
        assert!(shrunk.kept.is_empty() && shrunk.minimal);
    }

    #[test]
    fn a_deadline_already_past_keeps_the_input_and_says_it_is_not_minimal() {
        let shrunk = ddmin((0..40).collect(), Some(Instant::now()), holds_3_and_7);
        assert_eq!(shrunk.kept.len(), 40);
        assert!(!shrunk.minimal);
        assert_eq!(shrunk.tried, 0);
    }
}
