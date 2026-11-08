//! What happens when a dialog is answered.

use std::path::Path;

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{App, STARTUP_PROFILE};
use crate::dialog::{
    AdvancedKey, ChoiceFor, Dialog, ListKey, ProfilesKey, Prompt, PromptFor, PromptKey,
};
use crate::notification::Severity;
use crate::output::Transform;

impl App {
    /// Route a key to the topmost dialog.
    pub fn dialog_key(&mut self, key: KeyEvent) {
        let current = self.baseline_states();
        let Some(dialog) = self.dialogs.last_mut() else {
            return;
        };
        match dialog {
            Dialog::Choice(choice) => match choice.key(&key) {
                ListKey::Pick(index) => self.finish_choice(index),
                ListKey::Close => drop(self.dialogs.pop()),
                ListKey::Moved | ListKey::Ignored => {}
            },
            Dialog::Prompt(prompt) => match prompt.key(key) {
                PromptKey::Submit(value) => self.finish_prompt(Some(value)),
                PromptKey::Cancel => self.finish_prompt(None),
                PromptKey::Editing => {}
            },
            Dialog::Confirm(_) => match key.code {
                KeyCode::Char('y' | 'Y') => self.finish_confirm(true),
                KeyCode::Esc | KeyCode::Char('n' | 'N' | 'q') => self.finish_confirm(false),
                _ => {}
            },
            Dialog::Advanced(advanced) => match advanced.key(&key) {
                AdvancedKey::Apply => self.finish_advanced(true),
                AdvancedKey::Cancel => self.finish_advanced(false),
                AdvancedKey::PickIcc(path) => {
                    let placeholder = "/usr/share/color/icc/display.icc";
                    let prompt = Prompt::new("ICC profile", placeholder, PromptFor::IccPath, &path);
                    self.open(Dialog::Prompt(prompt));
                }
                AdvancedKey::Warn(message) => {
                    self.notifications.push(message, Severity::Warning, 5)
                }
                AdvancedKey::Handled => {}
            },
            Dialog::Profiles(profiles) => {
                let answer = profiles.key(&key, &self.store, &current);
                if profiles.take_changed() {
                    self.detect_active_profile();
                }
                self.finish_profiles(answer);
            }
            Dialog::Help(help) => match key.code {
                KeyCode::Esc | KeyCode::Char('q' | '?') => drop(self.dialogs.pop()),
                KeyCode::Up | KeyCode::Char('k') => help.scroll = help.scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => help.scroll += 1,
                KeyCode::PageUp => help.scroll = help.scroll.saturating_sub(10),
                KeyCode::PageDown => help.scroll += 10,
                KeyCode::Home => help.scroll = 0,
                _ => {}
            },
        }
    }

    fn finish_choice(&mut self, index: usize) {
        let Some(Dialog::Choice(choice)) = self.dialogs.pop() else {
            return;
        };
        let Some(target) = self.find(&choice.output) else {
            return;
        };
        match choice.choice_for {
            ChoiceFor::Mode(modes) => {
                if let Some(mode) = modes.into_iter().nth(index) {
                    self.edit(target, |output| {
                        let label = mode.label();
                        output.mode = Some(mode);
                        label
                    });
                }
            }
            ChoiceFor::Scale(scales) => match scales.get(index) {
                Some(scale) => self.set_scale(&choice.output, *scale),
                None => self.prompt_custom_scale(&choice.output),
            },
            ChoiceFor::Transform => self.edit(target, |output| {
                output.transform = Transform::ALL[index];
                format!("rotation {}", output.transform.as_str())
            }),
        }
    }

    fn finish_prompt(&mut self, answer: Option<String>) {
        let Some(Dialog::Prompt(prompt)) = self.dialogs.pop() else {
            return;
        };
        let Some(answer) = answer else { return };
        match prompt.prompt_for {
            PromptFor::CustomScale(output) => self.set_custom_scale(&output, &answer),
            PromptFor::SaveProfile => self.save_profile(&answer),
            PromptFor::RenameProfile(old) => {
                let current = self.baseline_states();
                if let Some(Dialog::Profiles(profiles)) = self.dialogs.last_mut() {
                    profiles.rename(&self.store, &old, &answer, &current);
                    if profiles.take_changed() {
                        self.detect_active_profile();
                    }
                }
            }
            PromptFor::IccPath => self.finish_icc(&answer),
        }
    }

    fn finish_confirm(&mut self, confirmed: bool) {
        self.dialogs.pop();
        if confirmed {
            self.save_profile(STARTUP_PROFILE);
        }
    }

    fn finish_advanced(&mut self, applied: bool) {
        let Some(Dialog::Advanced(advanced)) = self.dialogs.pop() else {
            return;
        };
        let name = advanced.output.name.clone();
        let message = if applied {
            "advanced settings updated"
        } else {
            "advanced settings cancelled"
        };
        if applied && let Some(index) = self.find(&name) {
            self.outputs[index] = advanced.output;
        }
        self.guides.clear();
        self.status = format!("{name}: {message}");
    }

    /// The ICC path prompt was answered: hand a usable profile to the advanced dialog.
    fn finish_icc(&mut self, answer: &str) {
        let home = std::env::home_dir().unwrap_or_default();
        let expanded = shellexpand::tilde_with_context(answer, || Some(home.to_string_lossy()));
        let path = Path::new(expanded.as_ref());
        let profile = format!("icc:{}", path.display());
        let compositor = self.compositor();
        // Compositors open the path verbatim, so it has to be absolute.
        let error = if !path.is_absolute() {
            Some("Use an absolute path (or ~/…) for the ICC profile.".to_string())
        } else if !compositor.color_profile_supported(&profile) {
            Some(format!("{} cannot use that ICC path.", compositor.name()))
        } else {
            None
        };
        if let Some(error) = error {
            self.notifications.push(error, Severity::Error, 5);
            return;
        }
        if !path.is_file() {
            let message = format!("{} does not exist on this machine.", path.display());
            self.notifications.push(message, Severity::Warning, 5);
        }
        if let Some(Dialog::Advanced(advanced)) = self.dialogs.last_mut() {
            advanced.set_icc(profile);
        }
    }

    fn finish_profiles(&mut self, answer: ProfilesKey) {
        let standalone = self.profiles_only;
        match answer {
            ProfilesKey::Handled => {}
            ProfilesKey::Rename(old) => {
                let prompt = Prompt::new(
                    "Rename profile",
                    "new name",
                    PromptFor::RenameProfile(old.clone()),
                    &old,
                );
                self.open(Dialog::Prompt(prompt));
            }
            ProfilesKey::Close => {
                self.dialogs.pop();
                self.quit |= standalone;
            }
            ProfilesKey::OpenEditor => {
                self.dialogs.pop();
                self.profiles_only = false;
                self.status = "Monitor editor ready.".into();
            }
            ProfilesKey::SaveCurrent => {
                self.dialogs.pop();
                self.prompt_save_profile();
            }
            ProfilesKey::Apply(name) => {
                self.dialogs.pop();
                let applied = self.apply_profile(&name);
                if standalone {
                    // A failed apply drops into the editor instead of exiting.
                    self.quit = applied;
                    self.profiles_only = false;
                }
            }
        }
    }
}
