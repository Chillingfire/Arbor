//! Held-out acceptance tests for the `wordy-capitalize-words` eval case. The Session never sees
//! them.

use wordy::capitalize_words;

fn capitalized(text: &str) -> String {
    capitalize_words(text).to_string()
}

#[test]
fn upper_cases_the_first_letter_of_each_word() {
    assert_eq!(capitalized("hello wide world"), "Hello Wide World");
}

#[test]
fn keeps_the_whitespace_between_words_exactly() {
    assert_eq!(capitalized("  two\t\tspaced\nlines  "), "  Two\t\tSpaced\nLines  ");
}

#[test]
fn leaves_the_rest_of_each_word_alone() {
    assert_eq!(capitalized("mcDonald iPhone ALREADY"), "McDonald IPhone ALREADY");
    assert_eq!(capitalized(""), "");
}
