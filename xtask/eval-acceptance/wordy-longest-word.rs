//! Held-out acceptance tests for the `wordy-longest-word` eval case. The Session never sees them.

use wordy::longest_word;

fn longest(text: &str) -> Option<String> {
    longest_word(text).map(|word| word.to_string())
}

#[test]
fn returns_the_longest_word() {
    assert_eq!(longest("a quick brownish fox"), Some("brownish".to_owned()));
}

#[test]
fn keeps_the_first_of_equally_long_words() {
    assert_eq!(longest("cat dog emu"), Some("cat".to_owned()));
    assert_eq!(longest("ox lion bear wolf"), Some("lion".to_owned()));
}

#[test]
fn has_no_answer_for_a_text_without_words() {
    assert_eq!(longest(""), None);
    assert_eq!(longest(" \t\n "), None);
}
