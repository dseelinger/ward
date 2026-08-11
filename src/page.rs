//! The settings page, drawn from the schema rather than by hand.
//!
//! Every row on this page comes from a declaration in [`crate::schema`]. There
//! is no list of controls here to keep in step with the list of settings, so a
//! setting cannot ship without a way to change it and a control cannot outlive
//! the setting it changed.
//!
//! Three rules the page follows, each because of a specific way this goes
//! wrong.
//!
//! **A default is shown behind an empty box, never inside it.** If the shipped
//! default were sitting in the box as a value, tabbing through the page would
//! commit every one of them as a permanent override — and the Commander would
//! then be pinned to today's defaults forever, having chosen nothing.
//!
//! **Clearing a box returns the setting to its default.** The empty box is how
//! you say "whatever Ward thinks", and it is the only way to say it.
//!
//! **A list that cannot be fetched degrades to a text box.** An empty dropdown
//! is a setting that can no longer be changed at all, which is worse than one
//! that has to be typed. The reason is shown and the current value is kept.

use std::collections::BTreeMap;

use eframe::egui;

use crate::config::Settings;
use crate::schema::{Catalog, Field, Row};

/// Long enough to judge a voice by, short enough to hear twice.
const PREVIEW: &str = "Ward here, Commander. This is how I sound.";

/// A list that has to be fetched, and what happened when it was.
#[derive(Default)]
pub enum Options<T> {
    /// Not asked for yet.
    #[default]
    Unasked,
    Asking,
    Have(T),
    /// Carries why, because a control that silently becomes a text box looks
    /// like a bug rather than a fallback.
    Refused(String),
}

impl<T> Options<T> {
    fn idle(&self) -> bool {
        !matches!(self, Options::Asking)
    }
}

/// One thing checked by the setup test.
pub struct Check {
    pub what: &'static str,
    pub ok: bool,
    pub detail: String,
}

/// What the page wants done, decided while drawing and carried out after.
#[derive(Default)]
pub struct Wants {
    /// A setting changed, so anything built from settings is now out of date.
    pub changed: bool,
    /// Speak this, in the voice currently chosen, so it can be heard before
    /// being committed to.
    pub preview: Option<String>,
    pub fetch_voices: bool,
    pub fetch_models: bool,
    pub run_setup_test: bool,
    pub store_key: Option<String>,
}

/// Everything the page is holding that is not a setting.
#[derive(Default)]
pub struct Page {
    /// What is in each box. Held apart from the settings themselves, so a value
    /// being typed is not applied one character at a time.
    typed: BTreeMap<String, String>,
    /// Why a box was refused, under the box that was refused.
    problems: BTreeMap<String, String>,
    pub voices: Options<Vec<crate::voice::Voice>>,
    pub models: Options<Vec<String>>,
    search: String,
    key_entry: String,
    pub key_problem: Option<String>,
    pub checks: Vec<Check>,
    pub testing: bool,
}

impl Page {
    /// Forgets what was typed, so the boxes are read from the settings again.
    pub fn reread(&mut self) {
        self.typed.clear();
        self.problems.clear();
    }

    pub fn key_stored(&mut self) {
        self.key_entry.clear();
        self.key_problem = None;
    }

    fn box_for(&mut self, key: &str, settings: &Settings) -> String {
        self.typed
            .entry(key.to_string())
            .or_insert_with(|| match settings.overridden(key) {
                // Only what the Commander chose. The default belongs behind the
                // box, not in it.
                true => crate::schema::write(&settings.get(key)),
                false => String::new(),
            })
            .clone()
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut Settings,
        key_is_stored: bool,
    ) -> Wants {
        let mut wants = Wants::default();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(8.0);
                ui.heading("Settings");
                ui.weak("Only what you change is written down. Empty a box to put it back.");
                ui.add_space(12.0);

                self.keys_card(ui, key_is_stored, &mut wants);
                self.setup_card(ui, &mut wants);

                for card in crate::schema::cards() {
                    ui.add_space(12.0);
                    ui.group(|ui| {
                        ui.set_width(ui.available_width());
                        ui.heading(card);
                        ui.add_space(4.0);

                        for row in crate::schema::all().into_iter().filter(|r| r.card == card) {
                            self.row(ui, row, settings, &mut wants);
                        }
                    });
                }

                ui.add_space(24.0);
            });

        wants
    }

    fn row(
        &mut self,
        ui: &mut egui::Ui,
        row: &'static Row,
        settings: &mut Settings,
        wants: &mut Wants,
    ) {
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(row.key).strong());

            if row.protected {
                ui.weak("· yours only").on_hover_text(
                    "Ward can read this and never change it. The model reads text \
                         nobody vetted, so anything that decides what Ward is willing \
                         to do is set here or not at all.",
                );
            }

            if settings.overridden(row.key) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("Reset")
                        .on_hover_text("Put this back to what Ward ships")
                        .clicked()
                    {
                        settings.clear(row.key);
                        self.typed.remove(row.key);
                        self.problems.remove(row.key);
                        wants.changed = true;
                    }
                });
            }
        });

        ui.weak(row.help);

        // Named rather than linked. The site is not published yet, and a link
        // that goes nowhere is worse than a path somebody can open.
        if let Some(doc) = row.doc {
            ui.weak(format!("Explained in {doc}"));
        }

        match row.kind {
            Field::Flag => self.flag(ui, row, settings, wants),
            Field::Choice(Catalog::Voices) => self.voice(ui, row, settings, wants),
            Field::Choice(Catalog::Models) => self.model(ui, row, settings, wants),
            _ => self.text(ui, row, settings, wants),
        }

        if let Some(problem) = self.problems.get(row.key) {
            ui.colored_label(ui.visuals().error_fg_color, problem);
        }
    }

    /// A checkbox has no empty state, so this one commits on click rather than
    /// leaving a default behind an empty box. Resetting is the button above.
    fn flag(
        &mut self,
        ui: &mut egui::Ui,
        row: &'static Row,
        settings: &mut Settings,
        wants: &mut Wants,
    ) {
        let mut on = settings.flag(row.key);

        if ui.checkbox(&mut on, "").changed() {
            settings.set(row.key, serde_json::json!(on));
            wants.changed = true;
        }
    }

    fn text(
        &mut self,
        ui: &mut egui::Ui,
        row: &'static Row,
        settings: &mut Settings,
        wants: &mut Wants,
    ) {
        let mut typed = self.box_for(row.key, settings);
        let placeholder = crate::schema::write(&(row.default)());

        let field = ui.add(
            egui::TextEdit::singleline(&mut typed)
                .hint_text(placeholder)
                .desired_width(ui.available_width() - 8.0),
        );

        self.typed.insert(row.key.to_string(), typed.clone());

        // Committed when the box is left or Enter is pressed, not per keystroke.
        // A folder is invalid for most of the time it takes to type one.
        let done = field.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter));

        if done && field.changed() || (done && self.problems.contains_key(row.key)) {
            self.commit(row, &typed, settings, wants);
        }
    }

    fn commit(
        &mut self,
        row: &'static Row,
        typed: &str,
        settings: &mut Settings,
        wants: &mut Wants,
    ) {
        match crate::schema::read(row, typed) {
            Ok(Some(value)) => {
                settings.set(row.key, value);
                self.problems.remove(row.key);
                wants.changed = true;
            }
            Ok(None) => {
                settings.clear(row.key);
                self.problems.remove(row.key);
                wants.changed = true;
            }
            Err(why) => {
                self.problems.insert(row.key.to_string(), why);
            }
        }
    }

    fn model(
        &mut self,
        ui: &mut egui::Ui,
        row: &'static Row,
        settings: &mut Settings,
        wants: &mut Wants,
    ) {
        let current = settings.string(row.key);

        // Taken out of the field before anything else on the page is touched.
        // Holding a borrow on the list while drawing a fallback that also needs
        // the page is the same mistake as reading and writing one setting in a
        // single expression.
        let listed = match &self.models {
            Options::Have(models) => Some(models.clone()),
            _ => None,
        };

        if let Some(models) = listed {
            egui::ComboBox::from_id_salt(row.key)
                .selected_text(&current)
                .width(ui.available_width() - 8.0)
                .show_ui(ui, |ui| {
                    for model in &models {
                        if ui.selectable_label(*model == current, model).clicked() {
                            settings.set(row.key, serde_json::json!(model));
                            wants.changed = true;
                        }
                    }
                });
            return;
        }

        let refused = match &self.models {
            Options::Refused(why) => Some(why.clone()),
            _ => None,
        };

        if let Some(why) = refused {
            ui.weak(format!("Type it in — {why}"));
        }

        let idle = self.models.idle();
        self.text(ui, row, settings, wants);

        if idle && ui.small_button("Fetch the list").clicked() {
            wants.fetch_models = true;
        }
    }

    /// The voice picker: browse, search, and hear one before keeping it.
    ///
    /// Choosing a voice is the first thing anyone does and there are hundreds,
    /// so a text box holding a name from memory is not a way to choose. Hearing
    /// it is the whole decision — a voice reads differently in a cockpit than
    /// its name suggests.
    fn voice(
        &mut self,
        ui: &mut egui::Ui,
        row: &'static Row,
        settings: &mut Settings,
        wants: &mut Wants,
    ) {
        let current = settings.string(row.key);

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&current).monospace());

            if ui.small_button("Hear it").clicked() {
                wants.preview = Some(PREVIEW.to_string());
            }
        });

        if !matches!(self.voices, Options::Have(_)) {
            let refused = match &self.voices {
                Options::Refused(why) => Some(why.clone()),
                _ => None,
            };

            if let Some(why) = refused {
                ui.weak(format!("Type a voice name — {why}"));
            }

            let idle = self.voices.idle();
            self.text(ui, row, settings, wants);

            if idle && ui.button("Browse voices").clicked() {
                wants.fetch_voices = true;
            }

            return;
        }

        // Split apart so the search box and the list can be touched at once.
        let Page { voices, search, .. } = self;
        let Options::Have(voices) = voices else {
            return;
        };

        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::singleline(search)
                .hint_text("Search — a name, a language, a country")
                .desired_width(ui.available_width() - 8.0),
        );

        let needle = search.trim().to_lowercase();

        let matching: Vec<&crate::voice::Voice> = voices
            .iter()
            .filter(|v| needle.is_empty() || v.searchable().contains(&needle))
            .collect();

        ui.weak(format!("{} of {} voices", matching.len(), voices.len()));

        let mut chosen: Option<String> = None;

        egui::ScrollArea::vertical()
            .id_salt("voices")
            .max_height(220.0)
            .show(ui, |ui| {
                for voice in matching {
                    let label = format!(
                        "{}  ·  {}  ·  {}",
                        voice.friendly, voice.locale, voice.gender
                    );

                    if ui.selectable_label(voice.name == current, label).clicked() {
                        chosen = Some(voice.name.clone());
                    }
                }
            });

        if let Some(name) = chosen {
            settings.set(row.key, serde_json::json!(name));
            wants.changed = true;
            // Heard the moment it is picked. Choosing from a list of names and
            // then having to ask separately is one step too many for something
            // you are about to listen to for hours.
            wants.preview = Some(PREVIEW.to_string());
        }
    }

    /// Hand-built, because a key is not a setting.
    ///
    /// It never appears on screen, it is never read back, and all the page ever
    /// shows is whether there is one. A key rendered into a text box is a key in
    /// the next screenshot or stream.
    fn keys_card(&mut self, ui: &mut egui::Ui, stored: bool, wants: &mut Wants) {
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.heading("API key");
            ui.add_space(4.0);

            match stored {
                true => ui.label("A key is stored. Ward can show it to nobody, including you."),
                false => ui.colored_label(egui::Color32::from_rgb(210, 160, 60), "No key stored."),
            };

            ui.weak(
                "Encrypted for this Windows account on this machine. Never written \
                 anywhere else, and never read from the environment.",
            );

            ui.add_space(6.0);

            ui.horizontal(|ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.key_entry)
                        .password(true)
                        .hint_text(match stored {
                            true => "Paste a new key to replace it",
                            false => "sk-ant-…",
                        })
                        .desired_width(ui.available_width() - 90.0),
                );

                let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

                if (ui.button("Save").clicked() || entered) && !self.key_entry.trim().is_empty() {
                    wants.store_key = Some(self.key_entry.trim().to_string());
                }
            });

            if let Some(problem) = &self.key_problem {
                ui.colored_label(ui.visuals().error_fg_color, problem);
            }
        });
    }

    /// The one card that answers "why is nothing happening".
    ///
    /// Every check here stands for an evening somebody could otherwise lose:
    /// a microphone that was never chosen, a key that was refused, a journal
    /// folder pointing at nothing. All of them fail silently in flight, which
    /// is the worst possible place to find out.
    fn setup_card(&mut self, ui: &mut egui::Ui, wants: &mut Wants) {
        ui.add_space(12.0);

        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.heading("Test my setup");
            ui.add_space(4.0);
            ui.weak(
                "Checks the things that fail quietly: the microphone, the speakers, \
                 your key, the speech model and where Elite writes its journal.",
            );
            ui.add_space(6.0);

            if ui
                .add_enabled(!self.testing, egui::Button::new("Run the test"))
                .clicked()
            {
                wants.run_setup_test = true;
            }

            if self.testing {
                ui.weak("Testing…");
            }

            for check in &self.checks {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    match check.ok {
                        true => ui.colored_label(egui::Color32::from_rgb(90, 180, 110), "OK"),
                        false => ui.colored_label(ui.visuals().error_fg_color, "Trouble"),
                    };
                    ui.label(egui::RichText::new(check.what).strong());
                });
                ui.weak(&check.detail);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_starts_empty_when_nothing_was_chosen() {
        // The rule that keeps tabbing through the page from committing every
        // shipped default as a permanent override.
        let mut page = Page::default();
        let settings = Settings::default();

        assert_eq!(page.box_for("voice", &settings), "");
        assert_eq!(page.box_for("speaking rate", &settings), "");
    }

    #[test]
    fn a_box_holds_what_the_commander_chose() {
        let mut page = Page::default();
        let mut settings = Settings::default();
        settings.set("speaking rate", serde_json::json!("+15%"));

        assert_eq!(page.box_for("speaking rate", &settings), "+15%");
    }

    #[test]
    fn a_list_that_was_refused_is_not_still_being_asked_for() {
        // The refused state has to offer the button again, or a failed fetch
        // leaves the setting unreachable for the rest of the session.
        let refused: Options<Vec<String>> = Options::Refused("no network".into());
        assert!(refused.idle());

        let asking: Options<Vec<String>> = Options::Asking;
        assert!(!asking.idle(), "it would ask twice at once");
    }
}
