//! Small helpers for working with the words in a text.

/// Counts the words in `text`, splitting on whitespace.
pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words_separated_by_any_whitespace() {
        assert_eq!(word_count("one two\tthree\nfour"), 4);
    }
}
