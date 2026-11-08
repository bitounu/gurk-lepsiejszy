//! Composer-only spelling correction. Modal keys never reach message sending.
use super::App;
use crossterm::event::{KeyCode, KeyEvent};
use std::ops::Range;

pub(crate) struct SpellingDialog {
    pub text: String,
    pub range: Option<Range<usize>>,
    pub word: String,
    pub selected: usize,
}

impl App {
    pub(crate) fn refresh_spelling(&mut self) {
        self.spelling.poll(&self.input.data);
        if self
            .spelling_dialog
            .as_ref()
            .is_some_and(|d| d.text != self.input.data)
        {
            self.spelling_dialog = None;
        }
        if self
            .spelling_dialog
            .as_ref()
            .is_some_and(|d| d.range.is_none())
        {
            let cursor = self.input.cursor.idx;
            let error = self
                .spelling
                .errors()
                .iter()
                .find(|e| e.range.start <= cursor && cursor <= e.range.end)
                .or_else(|| self.spelling.errors().first())
                .cloned();
            if let Some(error) = error {
                self.spelling.request_suggestions(error.range.clone());
                if let Some(dialog) = self.spelling_dialog.as_mut() {
                    dialog.range = Some(error.range);
                    dialog.word = error.word;
                }
            }
        }
    }

    pub(crate) async fn open_spelling(&mut self) {
        self.spelling_dialog = Some(SpellingDialog {
            text: self.input.data.clone(),
            range: None,
            word: String::new(),
            selected: 0,
        });
        self.refresh_spelling();
    }

    pub(crate) async fn on_spelling_key(&mut self, key: KeyEvent) {
        self.refresh_spelling();
        let Some(dialog) = self.spelling_dialog.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.spelling_dialog = None,
            KeyCode::Up => dialog.selected = dialog.selected.saturating_sub(1),
            KeyCode::Down => {
                dialog.selected =
                    (dialog.selected + 1).min(self.spelling.suggestions().len().saturating_sub(1))
            }
            KeyCode::Char('i') if dialog.range.is_some() => {
                self.spelling.ignore_word(&dialog.word);
                self.spelling_dialog = None;
            }
            KeyCode::Enter => {
                let Some(range) = dialog.range.clone() else {
                    return;
                };
                let Some(replacement) = self.spelling.suggestions().get(dialog.selected).cloned()
                else {
                    return;
                };
                if dialog.text != self.input.data
                    || self.input.data.get(range.clone()) != Some(dialog.word.as_str())
                {
                    return;
                }
                let old_cursor = self.input.cursor.idx;
                let new_cursor = if old_cursor <= range.start {
                    old_cursor
                } else if old_cursor >= range.end {
                    old_cursor - range.len() + replacement.len()
                } else {
                    range.start + replacement.len()
                };
                self.input.data.replace_range(range, &replacement);
                self.input.cursor = Default::default();
                while self.input.cursor.idx < new_cursor {
                    self.input.on_right();
                }
                self.input.completion_partial = None;
                self.spelling_dialog = None;
                self.spelling.request(&self.input.data);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn modal_enter_without_suggestion_does_not_send_or_change_text() {
        let (mut app, _, _) = crate::app::tests::test_app().await;
        app.input.put_char('ż');
        app.open_spelling().await;
        app.on_spelling_key(KeyEvent::from(KeyCode::Enter)).await;
        assert_eq!(app.input.data, "ż");
        assert!(app.spelling_dialog.is_some());
        app.on_spelling_key(KeyEvent::from(KeyCode::Esc)).await;
        assert!(app.spelling_dialog.is_none());
        assert_eq!(app.input.data, "ż");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn changed_composer_invalidates_open_dialog() {
        let (mut app, _, _) = crate::app::tests::test_app().await;
        app.open_spelling().await;
        app.input.put_char('x');
        app.refresh_spelling();
        assert!(app.spelling_dialog.is_none());
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn correction_replaces_unicode_word_and_preserves_surrounding_text() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.aff"), "SET UTF-8\n").unwrap();
        std::fs::write(dir.path().join("test.dic"), "3\nżółw\nhello\nworld\n").unwrap();
        let (mut app, _, _) = crate::app::tests::test_app().await;
        let config = crate::config::SpellcheckConfig {
            languages: vec!["test".into()],
            dictionary_dirs: vec![dir.path().to_owned()],
            ..Default::default()
        };
        app.spelling = crate::spellcheck::Spellchecker::new(&config);
        for c in "żółw helllo world".chars() {
            app.input.put_char(c);
        }
        app.open_spelling().await;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.spelling.suggestions().is_empty() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            app.refresh_spelling();
        }
        let selected = app
            .spelling
            .suggestions()
            .iter()
            .position(|s| s == "hello")
            .expect("synthetic dictionary suggests hello");
        app.spelling_dialog.as_mut().unwrap().selected = selected;
        app.on_spelling_key(KeyEvent::from(KeyCode::Enter)).await;
        assert_eq!(app.input.data, "żółw hello world");
        assert_eq!(app.input.cursor.idx, app.input.data.len());
        assert!(app.spelling_dialog.is_none());
    }
}
