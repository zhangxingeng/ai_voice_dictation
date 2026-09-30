//! Which language to transcribe.
//!
//! Chosen, not detected: auto-detection costs ~0.13 s a burst and misjudges
//! short ones, and the speaker always knows which language they are about to
//! use.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    English,
    Chinese,
}

impl Language {
    pub const ALL: [Language; 2] = [Language::English, Language::Chinese];

    /// Whisper's language code.
    pub fn code(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::Chinese => "zh",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Chinese => "中文",
        }
    }

    /// Text to condition the decoder on. Whisper writes Chinese in simplified
    /// or traditional characters unpredictably; a simplified-Mandarin prompt
    /// is the standard way to pin it to simplified.
    pub fn prompt(self) -> Option<&'static str> {
        match self {
            Language::English => None,
            Language::Chinese => Some("以下是普通话的句子。"),
        }
    }
}

/// What goes between existing text and a new fragment: a space between
/// words, nothing where either side is Chinese, which is written without
/// spaces.
pub fn separator(before: &str, after: &str) -> &'static str {
    let (Some(last), Some(first)) = (before.chars().next_back(), after.chars().next()) else {
        return "";
    };
    if last.is_whitespace() || is_cjk(last) || is_cjk(first) { "" } else { " " }
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x303F     // CJK punctuation: 。、「」
        | 0x3400..=0x4DBF   // extension A
        | 0x4E00..=0x9FFF   // unified ideographs
        | 0xF900..=0xFAFF   // compatibility ideographs
        | 0xFF00..=0xFFEF   // full-width forms: ，！？
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_is_the_default() {
        assert_eq!(Language::default(), Language::English);
    }

    #[test]
    fn words_get_a_space() {
        assert_eq!(separator("Hello there.", "Next"), " ");
    }

    #[test]
    fn nothing_before_the_first_fragment() {
        assert_eq!(separator("", "Hello"), "");
    }

    #[test]
    fn no_double_space_after_whitespace() {
        assert_eq!(separator("Hello\n", "Next"), "");
    }

    #[test]
    fn chinese_on_either_side_gets_no_space() {
        assert_eq!(separator("你好。", "今天"), "");
        assert_eq!(separator("Hello.", "今天"), "");
        assert_eq!(separator("你好", "OK"), "");
    }
}
