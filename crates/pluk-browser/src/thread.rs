//! X's length rules, and how a post that is over them becomes a thread.
//!
//! X does not count characters: a link costs 23 whatever its length, and
//! most scripts outside Latin, Cyrillic and a few symbol blocks cost two.
//! Pluk counts the same way, so a post that fits here fits there, and the
//! Post button is never found disabled.

/// What one post may weigh on an account without a longer limit.
pub const X_POST_LIMIT: usize = 280;
/// How many posts one thread may carry.
pub const MAX_THREAD_PARTS: usize = 25;
/// What every link weighs, whatever its length.
const LINK_WEIGHT: usize = 23;

/// A post's weight under X's rules.
pub fn weighted_length(text: &str) -> usize {
    text.split_whitespace()
        .map(|word| {
            if is_link(word) {
                LINK_WEIGHT
            } else {
                word.chars().map(char_weight).sum()
            }
        })
        .sum::<usize>()
        + text.chars().filter(|value| value.is_whitespace()).count()
}

/// Whether one word is something X turns into a link.
fn is_link(word: &str) -> bool {
    let trimmed = word.trim_matches(|value: char| matches!(value, '(' | ')' | ',' | '.' | '!' | '?'));
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return true;
    }
    let host = trimmed.split('/').next().unwrap_or_default();
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|value| value.is_ascii_alphanumeric() || value == '-')
        })
        && labels
            .last()
            .is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|value| value.is_ascii_alphabetic()))
}

/// X's per-character weight: one for the ranges it lists, two for the rest.
fn char_weight(value: char) -> usize {
    let code = value as u32;
    if code <= 4351
        || (8192..=8205).contains(&code)
        || (8208..=8223).contains(&code)
        || (8242..=8247).contains(&code)
    {
        1
    } else {
        2
    }
}

/// Cut text that is over the limit into posts that each fit, breaking at
/// sentence ends first and at words when a sentence alone is too long.
/// `None` when a single word cannot fit, or the text needs more parts than a
/// thread can hold.
pub fn split_into_parts(text: &str) -> Option<Vec<String>> {
    if weighted_length(text) <= X_POST_LIMIT {
        return Some(vec![text.to_owned()]);
    }
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    for sentence in sentences(text) {
        if fits(&current, sentence) {
            append(&mut current, sentence);
            continue;
        }
        if weighted_length(sentence) <= X_POST_LIMIT {
            flush(&mut parts, &mut current);
            current.push_str(sentence);
            continue;
        }
        for word in sentence.split_whitespace() {
            if weighted_length(word) > X_POST_LIMIT {
                return None;
            }
            if !fits(&current, word) {
                flush(&mut parts, &mut current);
            }
            append(&mut current, word);
        }
    }
    flush(&mut parts, &mut current);
    (parts.len() <= MAX_THREAD_PARTS).then_some(parts)
}

fn fits(current: &str, next: &str) -> bool {
    current.is_empty() || weighted_length(current) + 1 + weighted_length(next) <= X_POST_LIMIT
}

fn append(current: &mut String, next: &str) {
    if !current.is_empty() {
        current.push(' ');
    }
    current.push_str(next);
}

fn flush(parts: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        parts.push(std::mem::take(current));
    }
}

/// Sentences, split after `.`, `!` or `?` followed by whitespace, or at a
/// line break, each trimmed.
fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes: Vec<(usize, char)> = text.char_indices().collect();
    for (index, &(offset, value)) in bytes.iter().enumerate() {
        let ends_sentence = matches!(value, '.' | '!' | '?')
            && bytes
                .get(index + 1)
                .is_none_or(|(_, next)| next.is_whitespace());
        let breaks_line = value == '\n';
        if ends_sentence || breaks_line {
            let end = if breaks_line { offset } else { offset + value.len_utf8() };
            let piece = text[start..end].trim();
            if !piece.is_empty() {
                out.push(piece);
            }
            start = end;
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        out.push(tail);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_weigh_twenty_three_and_wide_scripts_weigh_two() {
        assert_eq!(weighted_length("hello"), 5);
        assert_eq!(weighted_length("see oga.desgn.space now"), 4 + 23 + 4);
        assert_eq!(weighted_length("https://x.com/a/status/1"), 23);
        assert_eq!(weighted_length("日本"), 4);
        assert_eq!(weighted_length("a b"), 3);
    }

    #[test]
    fn the_failed_post_is_over_the_limit_by_the_amount_x_showed() {
        let text = "You can tell the AI labs are asking their own models to optimize the older, cheaper ones. That's why everything is dropping to half price this month. Great for the average user. You'll still want the smart models watching the cheap ones though. That's what we built for: oga.desgn.space (free)";
        assert_eq!(weighted_length(text), X_POST_LIMIT + 21);
    }

    #[test]
    fn short_text_is_one_part_and_long_text_breaks_at_sentences() {
        assert_eq!(split_into_parts("Short.").unwrap(), vec!["Short."]);
        let sentence = "This sentence is exactly long enough to matter here.";
        let text = std::iter::repeat_n(sentence, 8)
            .collect::<Vec<_>>()
            .join(" ");
        let parts = split_into_parts(&text).unwrap();
        assert_eq!(parts.len(), 2);
        for part in &parts {
            assert!(weighted_length(part) <= X_POST_LIMIT);
            assert!(part.ends_with('.'));
        }
        assert_eq!(parts.join(" "), text);
    }

    #[test]
    fn a_sentence_too_long_for_one_post_breaks_at_words() {
        let text = std::iter::repeat_n("word", 100)
            .collect::<Vec<_>>()
            .join(" ");
        let parts = split_into_parts(&text).unwrap();
        assert!(parts.len() >= 2);
        assert!(parts.iter().all(|part| weighted_length(part) <= X_POST_LIMIT));
        assert!(split_into_parts(&"x".repeat(300)).is_none());
    }
}
