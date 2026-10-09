use std::cmp::Ordering;
use std::iter::Peekable;
use std::str::Chars;

/// Case-insensitive comparison that orders digit runs numerically, so "Ep 2" < "Ep 10".
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let (na, nb) = (take_digits(&mut a), take_digits(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

fn take_digits(it: &mut Peekable<Chars>) -> String {
    let mut n = String::new();
    while let Some(c) = it.next_if(char::is_ascii_digit) {
        n.push(c);
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_numbers_numerically() {
        let mut v = vec!["Ep 10", "ep 2", "Ep 1", "Ep 02b"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["Ep 1", "ep 2", "Ep 02b", "Ep 10"]);
    }
}
