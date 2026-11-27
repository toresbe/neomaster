/// Sort key for an item. MA's `sort_name` is already lower-cased, with
/// accents folded and leading articles dropped ("the beatles" sorts as
/// "beatles"); fall back to a lower-cased name when it is missing.
pub fn sort_key(sort_name: &str, name: &str) -> String {
    let s = if sort_name.trim().is_empty() { name } else { sort_name };
    s.trim().to_lowercase()
}

/// The letter shown in the wheel's letter-jump indicator: `A`–`Z` for
/// Latin letters, `#` for digits and symbols, and the upper-cased first
/// character otherwise (Æ, Ø, Å, Cyrillic, ...). Derived from the sort
/// key, so letter groups appear in sort order.
pub fn letter_of(sort_key: &str) -> String {
    match sort_key.chars().find(|c| !c.is_whitespace()) {
        None => "#".into(),
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase().to_string(),
        Some(c) if c.is_alphabetic() => c.to_uppercase().collect(),
        Some(_) => "#".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters() {
        assert_eq!(letter_of("50 cent"), "#");
        assert_eq!(letter_of("dr. dre"), "D");
        assert_eq!(letter_of("øystein sunde"), "Ø");
        assert_eq!(letter_of(""), "#");
        assert_eq!(sort_key("", "ABBA"), "abba");
        assert_eq!(sort_key("beatles", "The Beatles"), "beatles");
    }
}
