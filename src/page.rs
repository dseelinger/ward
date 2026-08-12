//! The settings page, drawn from the schema rather than by hand.
//!
//! Every row on this page comes from a declaration in [`crate::schema`]. There
//! is no list of controls here to keep in step with the list of settings, so a
//! setting cannot ship without a way to change it and a control cannot outlive
//! the setting it changed.
//!
//! **It holds no settings of its own.** Everything it draws comes from the
//! picture the engine published, and everything it changes goes back as an
//! intent. That is not tidiness: this page is drawn twice, once into the window
//! and once into the headset, and a page holding its own copy of the settings
//! would be two pages disagreeing about what a setting is. What is kept here is
//! only what is being typed and has not been committed yet.
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
use crate::engine::{Intent, Listing, View};
use crate::schema::{Catalog, Field, Row};

/// Long enough to judge a voice by, short enough to hear twice.
const PREVIEW: &str = "Ward here, Commander. This is how I sound.";

/// Everything the page is holding that is not a setting.
///
/// All of it is text mid-edit. Nothing here survives being committed, and
/// nothing here is the truth about anything — the truth is in the picture.
#[derive(Default)]
pub struct Page {
    /// What is in each box. Held apart from the settings themselves, so a value
    /// being typed is not applied one character at a time.
    typed: BTreeMap<String, String>,
    /// Why a box was refused, under the box that was refused.
    problems: BTreeMap<String, String>,
    search: String,
    key_entry: String,
}

impl Page {
    /// Forgets what was typed, so the boxes are read from the settings again.
    pub fn reread(&mut self) {
        self.typed.clear();
        self.problems.clear();
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

    pub fn show(&mut self, ui: &mut egui::Ui, view: &View, out: &mut Vec<Intent>) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(8.0);
                ui.heading("Settings");
                ui.weak("Only what you change is written down. Empty a box to put it back.");
                ui.add_space(12.0);

                self.keys_card(ui, view, out);
                self.setup_card(ui, view, out);

                for card in crate::schema::cards() {
                    ui.add_space(12.0);
                    ui.group(|ui| {
                        ui.set_width(ui.available_width());
                        ui.heading(card);
                        ui.add_space(4.0);

                        for row in crate::schema::all().into_iter().filter(|r| r.card == card) {
                            self.row(ui, row, view, out);
                        }
                    });
                }

                ui.add_space(24.0);
            });
    }

    fn row(&mut self, ui: &mut egui::Ui, row: &'static Row, view: &View, out: &mut Vec<Intent>) {
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

            if view.settings.overridden(row.key) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("Reset")
                        .on_hover_text("Put this back to what Ward ships")
                        .clicked()
                    {
                        out.push(Intent::Clear(row.key));
                        self.typed.remove(row.key);
                        self.problems.remove(row.key);
                    }
                });
            }
        });

        ui.weak(row.help);

        if let Some(doc) = row.doc {
            ui.weak(format!("Explained in {doc}"));
        }

        match row.kind {
            Field::Flag => self.flag(ui, row, view, out),
            Field::Choice(Catalog::Voices) => self.voice(ui, row, view, out),
            Field::Choice(Catalog::Models) => self.model(ui, row, view, out),
            _ => self.text(ui, row, view, out),
        }

        if let Some(problem) = self.problems.get(row.key) {
            ui.colored_label(ui.visuals().error_fg_color, problem);
        }
    }

    /// A checkbox has no empty state, so this one commits on click rather than
    /// leaving a default behind an empty box. Resetting is the button above.
    fn flag(&mut self, ui: &mut egui::Ui, row: &'static Row, view: &View, out: &mut Vec<Intent>) {
        let mut on = view.settings.flag(row.key);

        if ui.checkbox(&mut on, "").changed() {
            out.push(Intent::Set(row.key, serde_json::json!(on)));
        }
    }

    fn text(&mut self, ui: &mut egui::Ui, row: &'static Row, view: &View, out: &mut Vec<Intent>) {
        let mut typed = self.box_for(row.key, &view.settings);
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
            self.commit(row, &typed, out);
        }
    }

    fn commit(&mut self, row: &'static Row, typed: &str, out: &mut Vec<Intent>) {
        match crate::schema::read(row, typed) {
            Ok(Some(value)) => {
                out.push(Intent::Set(row.key, value));
                self.problems.remove(row.key);
            }
            Ok(None) => {
                out.push(Intent::Clear(row.key));
                self.problems.remove(row.key);
            }
            Err(why) => {
                self.problems.insert(row.key.to_string(), why);
            }
        }
    }

    fn model(&mut self, ui: &mut egui::Ui, row: &'static Row, view: &View, out: &mut Vec<Intent>) {
        let current = view.settings.string(row.key);

        if let Listing::Have(models) = &view.models {
            egui::ComboBox::from_id_salt(row.key)
                .selected_text(&current)
                .width(ui.available_width() - 8.0)
                .show_ui(ui, |ui| {
                    for model in models {
                        if ui.selectable_label(*model == current, model).clicked() {
                            out.push(Intent::Set(row.key, serde_json::json!(model)));
                        }
                    }
                });
            return;
        }

        if let Listing::Refused(why) = &view.models {
            ui.weak(format!("Type it in — {why}"));
        }

        let idle = view.models.idle();
        self.text(ui, row, view, out);

        if idle && ui.small_button("Fetch the list").clicked() {
            out.push(Intent::Models);
        }
    }

    /// The voice picker: browse, search, and hear one before keeping it.
    ///
    /// Choosing a voice is the first thing anyone does and there are hundreds,
    /// so a text box holding a name from memory is not a way to choose. Hearing
    /// it is the whole decision — a voice reads differently in a cockpit than
    /// its name suggests.
    fn voice(&mut self, ui: &mut egui::Ui, row: &'static Row, view: &View, out: &mut Vec<Intent>) {
        let current = view.settings.string(row.key);

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&current).monospace());

            if ui.small_button("Hear it").clicked() {
                out.push(Intent::Preview(PREVIEW.to_string()));
            }
        });

        let Listing::Have(voices) = &view.voices else {
            if let Listing::Refused(why) = &view.voices {
                ui.weak(format!("Type a voice name — {why}"));
            }

            let idle = view.voices.idle();
            self.text(ui, row, view, out);

            if idle && ui.button("Browse voices").clicked() {
                out.push(Intent::Voices);
            }

            return;
        };

        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("Search — a name, a language, a country")
                .desired_width(ui.available_width() - 8.0),
        );

        let needle = self.search.trim().to_lowercase();

        let matching: Vec<&crate::voice::Voice> = voices
            .iter()
            .filter(|v| needle.is_empty() || v.searchable().contains(&needle))
            .collect();

        ui.weak(format!("{} of {} voices", matching.len(), voices.len()));

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
                        out.push(Intent::Set(row.key, serde_json::json!(voice.name)));
                        // Heard the moment it is picked. Choosing from a list of
                        // names and then having to ask separately is one step too
                        // many for something you are about to listen to for hours.
                        out.push(Intent::Preview(PREVIEW.to_string()));
                    }
                }
            });
    }

    /// Hand-built, because a key is not a setting.
    ///
    /// It never appears on screen, it is never read back, and all the page ever
    /// shows is whether there is one. A key rendered into a text box is a key in
    /// the next screenshot or stream.
    fn keys_card(&mut self, ui: &mut egui::Ui, view: &View, out: &mut Vec<Intent>) {
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.heading("API key");
            ui.add_space(4.0);

            match view.key_stored {
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
                        .hint_text(match view.key_stored {
                            true => "Paste a new key to replace it",
                            false => "sk-ant-…",
                        })
                        .desired_width(ui.available_width() - 90.0),
                );

                let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

                if (ui.button("Save").clicked() || entered) && !self.key_entry.trim().is_empty() {
                    out.push(Intent::Key(self.key_entry.trim().to_string()));
                    self.key_entry.clear();
                }
            });

            if let Some(problem) = &view.key_problem {
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
    fn setup_card(&mut self, ui: &mut egui::Ui, view: &View, out: &mut Vec<Intent>) {
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
                .add_enabled(!view.testing, egui::Button::new("Run the test"))
                .clicked()
            {
                out.push(Intent::TestSetup);
            }

            if view.testing {
                ui.weak("Testing…");
            }

            for check in &view.checks {
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
        let refused: Listing<Vec<String>> = Listing::Refused("no network".into());
        assert!(refused.idle());

        let asking: Listing<Vec<String>> = Listing::Asking;
        assert!(!asking.idle(), "it would ask twice at once");
    }

    #[test]
    fn a_committed_box_asks_the_engine_rather_than_writing_a_setting() {
        // The page holds no settings, so the only thing a commit can produce is
        // an intent. If this ever went back to mutating a Settings of its own,
        // the headset and the window would each have one and they would drift.
        let mut page = Page::default();
        let mut out = Vec::new();
        let row = crate::schema::row("text size").expect("text size is a setting");

        page.commit(row, "1.8", &mut out);

        // Compared as a number rather than against a literal. The schema reads this row as an
        // `f32` and JSON holds an `f64`, so the widened value is not bit-for-bit the `1.8` a test
        // would write - and asserting on the wrong one of those fails on a correct commit.
        let [Intent::Set("text size", value)] = out.as_slice() else {
            panic!(
                "a good value should be one Set and nothing else, got {} intents",
                out.len()
            );
        };

        let stored = value.as_f64().expect("a number setting stores a number");
        assert!((stored - 1.8).abs() < 1e-6, "stored {stored}");
    }

    #[test]
    fn emptying_a_box_asks_for_the_default_rather_than_storing_nothing() {
        let mut page = Page::default();
        let mut out = Vec::new();
        let row = crate::schema::row("speaking rate").expect("speaking rate is a setting");

        page.commit(row, "   ", &mut out);

        assert!(
            matches!(out.as_slice(), [Intent::Clear("speaking rate")]),
            "an emptied box should clear rather than store an empty string"
        );
    }

    #[test]
    fn a_refused_value_asks_for_nothing_and_says_why() {
        // The failure that matters: a bad value that still reaches the engine is
        // a setting the Commander did not choose, applied because they typed.
        let mut page = Page::default();
        let mut out = Vec::new();
        let row = crate::schema::row("text size").expect("text size is a setting");

        page.commit(row, "40", &mut out);

        assert!(out.is_empty(), "a refused value must not reach the engine");
        assert!(
            page.problems.contains_key("text size"),
            "and the box has to say why"
        );
    }
}
