//! Offline Hunspell checking. All dictionary IO and linguistic work stays off the UI thread.
use std::{
    collections::HashSet, fs, ops::Range, path::PathBuf, sync::mpsc, thread, time::Duration,
};

use spellbook::Dictionary;
use unicode_segmentation::UnicodeSegmentation;

use crate::config::SpellcheckConfig;

const MAX_TEXT_BYTES: usize = 32 * 1024;
const MAX_WORD_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Misspelling {
    pub range: Range<usize>,
    pub word: String,
}

enum Request {
    Check {
        generation: u64,
        text: String,
        ignored: HashSet<String>,
    },
    Suggest {
        generation: u64,
        text: String,
        range: Range<usize>,
    },
}
enum Response {
    Loaded(Option<String>),
    Checked(u64, String, Vec<Misspelling>),
    Suggested(u64, String, Range<usize>, Vec<String>),
}

pub struct Spellchecker {
    enabled: bool,
    generation: u64,
    text: String,
    errors: Vec<Misspelling>,
    suggestions: Vec<String>,
    suggestion_range: Option<Range<usize>>,
    ignored: HashSet<String>,
    status: Option<String>,
    requests: mpsc::Sender<Request>,
    responses: mpsc::Receiver<Response>,
}

impl Spellchecker {
    pub fn new(config: &SpellcheckConfig) -> Self {
        let (requests, receive) = mpsc::channel();
        let (send, responses) = mpsc::channel();
        let enabled = config.enabled;
        let languages = config.languages.clone();
        let directories = config.dictionary_dirs.clone();
        let status = if enabled {
            Some("Loading spelling dictionaries…".into())
        } else {
            None
        };
        let mut checker = Self {
            enabled,
            generation: 0,
            text: String::new(),
            errors: Vec::new(),
            suggestions: Vec::new(),
            suggestion_range: None,
            ignored: config
                .ignored_words
                .iter()
                .map(|w| w.to_lowercase())
                .collect(),
            status,
            requests,
            responses,
        };
        if enabled
            && thread::Builder::new()
                .name("spellcheck".into())
                .spawn(move || {
                    let (dictionaries, status) = load_dictionaries(&languages, &directories);
                    let _ = send.send(Response::Loaded(status));
                    while let Ok(mut request) = receive.recv() {
                        // Coalesce bursts of typing rather than processing an obsolete backlog.
                        loop {
                            match &request {
                                Request::Check { .. } => {
                                    match receive.recv_timeout(Duration::from_millis(300)) {
                                        Ok(newer) => request = newer,
                                        Err(_) => break,
                                    }
                                }
                                Request::Suggest { .. } => match receive.try_recv() {
                                    Ok(newer) => request = newer,
                                    Err(_) => break,
                                },
                            }
                        }
                        let response = match request {
                            Request::Check {
                                generation,
                                text,
                                ignored,
                            } => {
                                let errors = check_text(&text, &dictionaries, &ignored);
                                Response::Checked(generation, text, errors)
                            }
                            Request::Suggest {
                                generation,
                                text,
                                range,
                            } => {
                                let suggestions = text
                                    .get(range.clone())
                                    .map(|word| suggest_word(word, &dictionaries))
                                    .unwrap_or_default();
                                Response::Suggested(generation, text, range, suggestions)
                            }
                        };
                        if send.send(response).is_err() {
                            break;
                        }
                    }
                })
                .is_err()
        {
            checker.status = Some("Could not start spelling worker".into());
        }
        checker
    }

    pub fn request(&mut self, text: &str) {
        if !self.enabled || self.text == text {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        self.text = text.to_owned();
        self.errors.clear();
        self.suggestions.clear();
        let _ = self.requests.send(Request::Check {
            generation: self.generation,
            text: self.text.clone(),
            ignored: self.ignored.clone(),
        });
    }

    /// Drain completed work. Results for an older editor snapshot are never exposed.
    pub fn poll(&mut self, current: &str) -> bool {
        self.request(current);
        let mut changed = false;
        while let Ok(response) = self.responses.try_recv() {
            match response {
                Response::Loaded(status) => {
                    self.status = status;
                    changed = true;
                }
                Response::Checked(generation, text, errors)
                    if generation == self.generation && text == current =>
                {
                    self.errors = errors;
                    changed = true;
                }
                Response::Suggested(generation, text, range, suggestions)
                    if generation == self.generation
                        && text == current
                        && self.suggestion_range.as_ref() == Some(&range) =>
                {
                    self.suggestions = suggestions;
                    changed = true;
                }
                _ => {}
            }
        }
        changed
    }

    pub fn errors(&self) -> &[Misspelling] {
        &self.errors
    }
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }
    pub fn suggestions(&self) -> &[String] {
        &self.suggestions
    }

    pub fn request_suggestions(&mut self, range: Range<usize>) {
        self.suggestions.clear();
        self.suggestion_range = Some(range.clone());
        let _ = self.requests.send(Request::Suggest {
            generation: self.generation,
            text: self.text.clone(),
            range,
        });
    }

    /// The caller persists the word in configuration; this updates the live session.
    pub fn ignore_word(&mut self, word: &str) {
        self.ignored.insert(word.to_lowercase());
        self.generation = self.generation.wrapping_add(1);
        self.suggestions.clear();
        let _ = self.requests.send(Request::Check {
            generation: self.generation,
            text: self.text.clone(),
            ignored: self.ignored.clone(),
        });
        self.errors.retain(|error| {
            !error.word.eq_ignore_ascii_case(word)
                && error.word.to_lowercase() != word.to_lowercase()
        });
    }
}

fn load_dictionaries(
    languages: &[String],
    configured: &[PathBuf],
) -> (Vec<Dictionary>, Option<String>) {
    let mut directories = configured.to_vec();
    if let Some(data) = dirs::data_dir() {
        directories.push(data.join("gurk/dictionaries"));
    }
    directories.extend(
        [
            "/usr/share/hunspell",
            "/usr/share/myspell/dicts",
            "/usr/share/myspell",
        ]
        .map(PathBuf::from),
    );
    let mut dictionaries = Vec::new();
    let mut missing = Vec::new();
    for language in languages.iter().take(8) {
        // A language is a dictionary basename, never an arbitrary filesystem path.
        if language.is_empty()
            || !language
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            missing.push(language.clone());
            continue;
        }
        let dictionary = directories.iter().find_map(|directory| {
            let aff_path = directory.join(format!("{language}.aff"));
            let dic_path = directory.join(format!("{language}.dic"));
            if fs::metadata(&aff_path).ok()?.len() > 8 * 1024 * 1024
                || fs::metadata(&dic_path).ok()?.len() > 32 * 1024 * 1024
            {
                return None;
            }
            let aff = fs::read(aff_path).ok()?;
            let dic = fs::read(dic_path).ok()?;
            let encoding = String::from_utf8_lossy(&aff)
                .lines()
                .find_map(|line| line.strip_prefix("SET ").map(str::trim))
                .unwrap_or("UTF-8")
                .to_owned();
            let encoding = encoding_rs::Encoding::for_label(encoding.as_bytes())?;
            let (aff, _, aff_errors) = encoding.decode(&aff);
            let (dic, _, dic_errors) = encoding.decode(&dic);
            if aff_errors || dic_errors {
                return None;
            }
            Dictionary::new(&aff, &dic).ok()
        });
        if let Some(dictionary) = dictionary {
            dictionaries.push(dictionary);
        } else {
            missing.push(language.clone());
        }
    }
    missing.extend(languages.iter().skip(8).cloned());
    let status = if languages.is_empty() {
        Some("Spelling: no dictionary languages configured".into())
    } else if !missing.is_empty() {
        Some(format!(
            "Spelling: unavailable dictionaries: {}",
            missing.join(", ")
        ))
    } else {
        None
    };
    (dictionaries, status)
}

fn check_text(
    text: &str,
    dictionaries: &[Dictionary],
    ignored: &HashSet<String>,
) -> Vec<Misspelling> {
    if dictionaries.is_empty() {
        return Vec::new();
    }
    word_ranges(text)
        .into_iter()
        .filter_map(|range| {
            let word = &text[range.clone()];
            if ignored.contains(&word.to_lowercase())
                || dictionaries.iter().any(|dictionary| dictionary.check(word))
            {
                None
            } else {
                Some(Misspelling {
                    range,
                    word: word.into(),
                })
            }
        })
        .collect()
}

fn suggest_word(word: &str, dictionaries: &[Dictionary]) -> Vec<String> {
    if word.len() > MAX_WORD_BYTES {
        return Vec::new();
    }
    let mut suggestions = Vec::new();
    let mut per_dictionary = Vec::new();
    let mut seen = HashSet::new();
    for dictionary in dictionaries {
        // Spellbook clears its output on each call. Accumulate each language separately.
        dictionary
            .suggester()
            .with_ngram_suggestions(false)
            .suggest(word, &mut per_dictionary);
        suggestions.extend(
            per_dictionary
                .drain(..)
                .filter(|word| seen.insert(word.clone())),
        );
    }
    suggestions.truncate(8);
    suggestions
}

fn word_ranges(text: &str) -> Vec<Range<usize>> {
    let mut limit = text.len().min(MAX_TEXT_BYTES);
    while !text.is_char_boundary(limit) {
        limit -= 1;
    }
    let truncated = limit < text.len();
    let text = &text[..limit];
    let mut ranges = Vec::new();
    let mut in_code = false;
    let mut offset = 0;
    for chunk in text.split_inclusive(char::is_whitespace) {
        let start = offset;
        offset += chunk.len();
        if start >= MAX_TEXT_BYTES {
            break;
        }
        let code_marker = chunk.contains('`');
        let skip = in_code
            || code_marker
            || chunk.contains("://")
            || chunk.contains('@')
            || chunk.contains('/')
            || chunk.contains('\\')
            || chunk.contains('_')
            || chunk.chars().any(|c| c.is_numeric())
            || chunk.trim().starts_with("www.");
        if chunk.matches('`').count() % 2 == 1 {
            in_code = !in_code;
        }
        if skip {
            continue;
        }
        for (index, word) in chunk.unicode_word_indices() {
            if truncated && start + index + word.len() == limit {
                continue;
            }
            if word.len() <= MAX_WORD_BYTES && word.chars().any(char::is_alphabetic) {
                ranges.push(start + index..start + index + word.len());
            }
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dictionary() -> Dictionary {
        Dictionary::new(
            "SET UTF-8\nTRY abcdefghijklmnopqrstuvwxyz\n",
            "3\nhello\nworld\nżółw\n",
        )
        .unwrap()
    }

    #[test]
    fn unicode_byte_ranges_and_nonprose_exclusions() {
        let text = "żółw helo https://bad.test a@bad.test /tmp/bad `bad code` foo_bar 42bad world";
        let errors = check_text(text, &[dictionary()], &HashSet::new());
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].word, "helo");
        assert_eq!(&text[errors[0].range.clone()], "helo");
        assert_eq!(errors[0].range.start, "żółw ".len());
    }

    #[test]
    fn any_dictionary_and_personal_words_are_accepted() {
        let second = Dictionary::new("SET UTF-8\n", "1\nwitaj\n").unwrap();
        let ignored = HashSet::from(["gurk".into()]);
        assert!(check_text("hello witaj Gurk", &[dictionary(), second], &ignored).is_empty());
        assert!(check_text("anything", &[], &HashSet::new()).is_empty());
    }

    #[test]
    fn suggestions_are_local() {
        let mut suggestions = Vec::new();
        dictionary().suggest("helo", &mut suggestions);
        assert!(suggestions.iter().any(|word| word == "hello"));
    }

    #[test]
    fn suggestions_from_first_language_survive_later_dictionary() {
        let polish = Dictionary::new("SET UTF-8\n", "1\nżółw\n").unwrap();
        let english = Dictionary::new("SET UTF-8\n", "1\nhello\n").unwrap();
        let suggestions = suggest_word("żółww", &[polish, english]);
        assert!(suggestions.iter().any(|word| word == "żółw"));
    }

    #[test]
    fn no_whitespace_input_obeys_byte_budget_without_splitting_unicode() {
        let text = "żółw,".repeat(MAX_TEXT_BYTES);
        let ranges = word_ranges(&text);
        assert!(!ranges.is_empty());
        assert!(ranges.len() <= MAX_TEXT_BYTES / "żółw,".len());
        for range in ranges {
            assert!(range.end <= MAX_TEXT_BYTES);
            assert_eq!(&text[range], "żółw");
        }
    }

    #[test]
    fn configured_dictionary_loading_decodes_polish_and_reports_missing() {
        let directory = std::env::temp_dir().join(format!(
            "gurk-spellcheck-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("fixture.aff"), "SET ISO8859-2\n").unwrap();
        let (encoded, _, _) = encoding_rs::ISO_8859_2.encode("1\nżółw\n");
        fs::write(directory.join("fixture.dic"), encoded.as_ref()).unwrap();
        let (dictionaries, status) = load_dictionaries(
            &["fixture".into(), "missing_fixture".into()],
            std::slice::from_ref(&directory),
        );
        fs::remove_dir_all(directory).unwrap();
        assert_eq!(dictionaries.len(), 1);
        assert!(dictionaries[0].check("żółw"));
        assert!(status.unwrap().contains("missing_fixture"));
    }

    #[test]
    fn stale_worker_results_are_discarded() {
        let (requests, _) = mpsc::channel();
        let (send, responses) = mpsc::channel();
        let mut checker = Spellchecker {
            enabled: true,
            generation: 2,
            text: "world".into(),
            errors: Vec::new(),
            suggestions: Vec::new(),
            suggestion_range: None,
            ignored: HashSet::new(),
            status: None,
            requests,
            responses,
        };
        send.send(Response::Checked(
            1,
            "helo".into(),
            vec![Misspelling {
                range: 0..4,
                word: "helo".into(),
            }],
        ))
        .unwrap();
        send.send(Response::Suggested(
            1,
            "helo".into(),
            0..4,
            vec!["hello".into()],
        ))
        .unwrap();
        checker.poll("world");
        assert!(checker.errors().is_empty());
        assert!(checker.suggestions().is_empty());
    }
}
