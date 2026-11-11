//! Held-out acceptance tests for the `wordy-unique-words` eval case. The Session never sees them.

use wordy::unique_words;

fn unique(text: &str) -> Vec<String> {
    unique_words(text)
        .into_iter()
        .map(|word| word.to_string())
        .collect()
}

#[test]
fn lists_distinct_words_in_first_appearance_order() {
    assert_eq!(unique("b a b c a"), ["b", "a", "c"]);
}

#[test]
fn compares_case_insensitively_and_keeps_the_first_spelling() {
    assert_eq!(unique("The cat and the Cat AND hat"), ["The", "cat", "and", "hat"]);
}

#[test]
fn has_no_words_for_a_blank_text() {
    assert!(unique(" \n\t").is_empty());
}
