//! The one widget tree, drawn to the window and to the headset.
//!
//! This file is the whole of [D4](../docs/decisions.md). There is no second
//! interface and no scaled-down copy of this one: the window and the panel call
//! the same function with the same picture, so the window cannot become more
//! capable than the headset without the extra capability having nowhere to put
//! itself.
//!
//! # It takes a picture and gives back intentions
//!
//! Nothing here owns anything Ward knows. It is handed a [`View`] the engine
//! published and a bag of half-typed text, and everything the Commander does
//! comes back as a list of [`Intent`] for the caller to send. That is what makes
//! it drawable from a thread that owns none of Ward: the overlay thread has a
//! clone of the engine's handle and nothing else, and it is enough.
//!
//! Deciding while drawing and acting afterwards is deliberate. Running a tool
//! mid-frame would mean the rest of the frame renders a list that has already
//! changed underneath it.
//!
//! # Mini is a mode, not a smaller panel
//!
//! [`Mode::Mini`] draws a reduced set of what is worth seeing at a glance, the
//! way a media player's compact mode does. It is not this panel scaled down —
//! scaling is a separate thing the big mode does — and it is not a second
//! surface with its own state. Everything on it is on the big one too.

use eframe::egui;

use crate::anthropic::Role;
use crate::engine::{Intent, Update, View};
use crate::shown;

pub use crate::panel::Mode;

/// How tall the strip along the top of the panel is, in points.
///
/// It exists so that a press has somewhere to mean "pick this up". Everything else on this surface
/// is a widget, and on a surface made only of widgets there is no press that could ever start a
/// grab — asking the toolkit whether it wants the pointer answers yes everywhere, which is the
/// same as answering nowhere.
///
/// Drawn in the shared tree rather than only in the headset, because there is one tree. On the
/// desktop it is a title bar that happens to do nothing, which is what a title bar mostly is.
pub const GRAB_STRIP: f32 = 26.0;

/// Draws the strip a press can pick the panel up by.
fn grab_strip(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), GRAB_STRIP),
        egui::Sense::hover(),
    );

    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, ui.visuals().faint_bg_color);

    // Three short lines in the middle, which is what every surface anybody has ever dragged looks
    // like. There is no text: this is the one part of the panel that says nothing.
    let middle = rect.center();
    let stroke = egui::Stroke::new(1.0, ui.visuals().weak_text_color());

    for step in -1..=1 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "three lines, and the count is a literal"
        )]
        let y = middle.y + (step as f32) * 4.0;
        painter.hline((middle.x - 22.0)..=(middle.x + 22.0), y, stroke);
    }
}

/// Everything a surface holds that the engine does not.
///
/// All of it is text mid-edit and which tab is open. None of it is the truth
/// about anything, which is why two surfaces can each have one.
#[derive(Default)]
pub struct Local {
    /// What is being typed to Ward.
    pub prompt: String,
    /// A new checklist item being typed. Held here rather than in the panel so
    /// a half-typed line survives the frame it was typed on.
    pub adding: String,
    /// The key being entered, on the screen that asks for one.
    pub key_entry: String,
    pub page: crate::page::Page,
    pub settings_open: bool,
    /// How many spoken requests for the panel this surface has already acted on.
    ///
    /// Kept beside the rest of what a surface holds because it is exactly that: two surfaces can
    /// each have answered a different number of them, and neither is the truth about anything.
    pub answered_asks: u64,
}

/// Draws the whole surface and collects what the Commander asked for.
pub fn show(ui: &mut egui::Ui, view: &View, local: &mut Local, mode: Mode) -> Vec<Intent> {
    let mut out = Vec::new();

    match mode {
        Mode::Gone => {}
        Mode::Mini => mini(ui, view, &mut out),
        Mode::Big => big(ui, view, local, &mut out),
    }

    out
}

/// The mini panel's own backing, for the same reason a caption has one.
///
/// The big panel paints itself: it is made of panels and frames, and the toolkit fills them. The
/// mini one is four bare lines, and bare lines over a starfield, a station's floodlights and a
/// cockpit's own instruments are unreadable against half of them.
///
/// Not quite opaque, so the Commander can still see what is behind it, and a little more solid
/// than a caption because this one stays up rather than passing by.
const GLANCE: egui::Color32 = egui::Color32::from_rgba_premultiplied(0, 0, 0, 215);
const GLANCE_INK: egui::Color32 = egui::Color32::from_rgb(235, 235, 235);

/// What is worth seeing at a glance.
///
/// The test for a row here is whether a Commander would look at it without
/// stopping what they are doing. What Ward is doing right now, the last thing it
/// said, how much is on the list, and anything that needs an answer.
///
/// Drawn into an area rather than the whole layer, so the box is as tall as what is on it. The
/// alternative is a black slab hanging in the cockpit with four lines in the top of it, which is
/// the exact mistake the caption layer already made once.
fn mini(ui: &mut egui::Ui, view: &View, out: &mut Vec<Intent>) {
    // The same strip as the big panel, in the same place, so picking either one up is the same
    // movement rather than two things to learn.
    egui::Panel::top("handle").show(ui, grab_strip);

    egui::Area::new(egui::Id::new("ward-mini"))
        .anchor(egui::Align2::LEFT_TOP, egui::vec2(0.0, 0.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(GLANCE)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .corner_radius(4)
                .show(ui, |ui| {
                    // Bright enough to read against anything, because there is no telling what is
                    // behind it. The toolkit's own greys are chosen against a known background and
                    // this has none.
                    ui.style_mut().visuals.override_text_color = Some(GLANCE_INK);
                    glance(ui, view, out);
                });
        });
}

/// The four lines themselves.
fn glance(ui: &mut egui::Ui, view: &View, out: &mut Vec<Intent>) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Ward").strong());
        ui.weak(state_of(view));
    });

    // No reply here. Captions already carry what Ward just said, paced by the voice and placed
    // under what the Commander is looking at, and a second copy of it on a panel a foot to the
    // side is the same duplication the transcript refuses to make on the desktop.

    if let Some(problem) = view.problem.as_ref() {
        ui.add_space(4.0);
        ui.colored_label(ui.visuals().error_fg_color, problem);
    }

    // The next thing rather than how many things. A count tells the Commander there is work and
    // nothing about what it is, so it always costs a second look at the big panel to act on -
    // which on a surface whose whole job is to save that look is the wrong trade.
    if let Some(next) = next_up(view) {
        ui.add_space(4.0);
        ui.add(egui::Label::new(egui::RichText::new(next).italics()).wrap());
    }

    // News rather than furniture, so it earns its place on a surface this small.
    if !matches!(view.update, Update::Nothing) {
        ui.add_space(4.0);
        update_notice(ui, &view.update, out);
    }
}

/// The first thing left on any list a capability is showing.
///
/// In the order the capabilities were composed and the order the items were written, which is the
/// order the Commander put them in. Nothing here ranks or guesses at what is urgent: the next
/// thing on a list is the next thing on the list.
fn next_up(view: &View) -> Option<&str> {
    view.panels
        .iter()
        .filter_map(|display| match display {
            shown::Shown::List { rows, .. } => Some(rows),
            _ => None,
        })
        .flatten()
        .find(|row| !row.done)
        .map(|row| row.text.as_str())
}

/// What Ward is doing, in the fewest words that distinguish the cases.
fn state_of(view: &View) -> &'static str {
    match (view.listening, view.streaming, view.using.is_some()) {
        // Listening wins: it is the state the Commander is currently holding a
        // key to be in, and the one they are waiting for confirmation of.
        (true, ..) => "listening",
        (_, _, true) => "looking something up",
        (_, true, _) => "answering",
        _ => "ready",
    }
}

/// Everything the window has.
fn big(ui: &mut egui::Ui, view: &View, local: &mut Local, out: &mut Vec<Intent>) {
    if !view.key_stored {
        key_screen(ui, view, local, out);
        return;
    }

    // A bar rather than a button tucked somewhere: settings is the second half
    // of what this surface is for, and a Commander whose microphone is wrong
    // needs to reach it without hunting.
    egui::Panel::top("handle").show(ui, grab_strip);

    egui::Panel::top("where").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .selectable_label(!local.settings_open, "Conversation")
                .clicked()
            {
                local.settings_open = false;
            }

            if ui
                .selectable_label(local.settings_open, "Settings")
                .clicked()
            {
                // Read from the settings again on the way in, so a box left
                // half-typed last time does not come back.
                local.page.reread();
                local.settings_open = true;
            }

            // No count of changed settings here. It said "3 changed", which is a number with no
            // noun and, worse, a tense nobody chose: a setting picked months ago is not a change,
            // and a bar that reports one every session reads as an unread notification about
            // something the Commander already decided. The settings page itself already shows
            // which rows are overridden, next to the button that puts each one back — which is
            // where that belongs, because there it is actionable rather than nagging.

            // Pushed to the right so it never moves the two tabs around, and
            // absent entirely when there is nothing to say. An update notice is
            // news the first time and furniture every time after.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                update_notice(ui, &view.update, out);
            });
        });
        ui.add_space(4.0);
    });

    if local.settings_open {
        egui::CentralPanel::default().show(ui, |ui| {
            local.page.show(ui, view, out);
        });
        return;
    }

    // Beside the conversation rather than inside it. What Ward is holding for
    // the Commander is standing state, and standing state scrolling away with
    // the transcript is the thing that makes a panel useless.
    egui::Panel::right("standing")
        .resizable(true)
        .default_size(300.0)
        // Bounded, because the panel sizes itself around what is in it and a
        // long checklist item would otherwise push it as wide as the sentence,
        // leaving the conversation a strip down the side.
        .size_range(220.0..=460.0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| checklist(ui, view, local, out));
        });

    egui::CentralPanel::default().show(ui, |ui| {
        // Compose is laid out first so it pins to the bottom; the transcript
        // then takes whatever height is left.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            compose(ui, view, local, out);
            ui.separator();

            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                transcript(ui, view);
            });
        });
    });
}

/// Says that a newer Ward exists, and takes the answer.
///
/// Nothing is downloaded until the Commander says so, and nothing is installed
/// until Ward is closing. Both halves matter: fetching and running a binary on
/// somebody's behalf is a decision they did not make, and an installer window
/// appearing over the game is one they cannot get to in a headset.
fn update_notice(ui: &mut egui::Ui, update: &Update, out: &mut Vec<Intent>) {
    match update {
        Update::Nothing => {}
        Update::Newer(version) => {
            if ui
                .button(format!("Ward {version} is out — get it"))
                .on_hover_text(
                    "Downloads it and checks it against the checksum published \
                     with the release. It installs when you close Ward.",
                )
                .clicked()
            {
                out.push(Intent::Update);
            }
        }
        Update::Fetching(version) => {
            ui.weak(format!("Fetching Ward {version}…"));
        }
        Update::Ready { version, .. } => {
            ui.weak(format!("Ward {version} installs when you close this."));
        }
        Update::Failed(why) => {
            ui.colored_label(ui.visuals().error_fg_color, format!("Update failed: {why}"));
        }
    }
}

fn key_screen(ui: &mut egui::Ui, view: &View, local: &mut Local, out: &mut Vec<Intent>) {
    egui::CentralPanel::default().show(ui, |ui| {
        ui.add_space(24.0);
        ui.heading("Ward needs an API key");
        ui.add_space(8.0);
        ui.label(
            "Stored encrypted, readable only by this Windows account on this machine. \
             It is never written anywhere else and never read from the environment.",
        );
        ui.add_space(16.0);

        if let Some(problem) = view.key_problem.as_ref() {
            ui.colored_label(ui.visuals().error_fg_color, problem);
            ui.add_space(8.0);
        }

        // Masked, because a key on screen is a key in a screenshot or a stream.
        let field = ui.add(
            egui::TextEdit::singleline(&mut local.key_entry)
                .password(true)
                .hint_text("sk-ant-…")
                .desired_width(f32::INFINITY),
        );

        let submitted = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        ui.add_space(8.0);

        if (ui.button("Save").clicked() || submitted) && !local.key_entry.trim().is_empty() {
            out.push(Intent::Key(local.key_entry.trim().to_string()));
            local.key_entry.clear();
        }
    });
}

fn compose(ui: &mut egui::Ui, view: &View, local: &mut Local, out: &mut Vec<Intent>) {
    let busy = view.streaming;

    ui.horizontal(|ui| {
        let send = ui.add_enabled(!busy, egui::Button::new("Send"));

        let hint = match (busy, view.listening) {
            // Listening wins: it is the state the Commander is currently holding
            // a key to be in, and it is the one they are waiting for
            // confirmation of.
            (_, true) => "Listening…",
            (true, _) => "Ward is answering…",
            _ => "Ask Ward something, or hold your push-to-talk key",
        };

        let field = ui.add_enabled(
            !busy,
            egui::TextEdit::singleline(&mut local.prompt)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        );

        let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        if (send.clicked() || entered) && !local.prompt.trim().is_empty() {
            out.push(Intent::Ask(std::mem::take(&mut local.prompt)));
            // Keep focus in the box so a second question does not need the
            // mouse — or, in the headset, a second go at pointing at it.
            field.request_focus();
        }
    });
}

/// Draws whatever the capabilities currently had to show when the engine last
/// published.
///
/// Every edit here goes through the same tool the model calls, with the same
/// words. The surface cannot make a change the model could not, and it cannot
/// make one in a way the checklist's own rules would refuse.
fn checklist(ui: &mut egui::Ui, view: &View, local: &mut Local, out: &mut Vec<Intent>) {
    for display in &view.panels {
        let title = display.title().unwrap_or_default().to_string();

        let shown::Shown::List { rows, .. } = display else {
            // Only lists have a panel today. The rest of the taxonomy grows with
            // the capability that needs it, rather than ahead of it.
            continue;
        };

        ui.add_space(8.0);
        ui.heading(&title);
        ui.add_space(4.0);

        if rows.is_empty() {
            ui.weak("Nothing on it yet.");
        }

        for row in rows {
            // Top aligned, because a wrapped item is taller than the tick box
            // beside it and a centered box floats in the middle of a two-line
            // item.
            ui.horizontal_top(|ui| {
                let mut done = row.done;

                // Both ways. It used to tick and never untick, on the reasoning that a control
                // with no spoken equivalent is how two surfaces start disagreeing about what can
                // be done - which was the right rule and the wrong fix. The fix is the tool, so
                // the model can put an item back too, and then the box is free to do what a box
                // does.
                let hint = match done {
                    true => "Put it back on the list",
                    false => "Mark as done",
                };

                if ui.checkbox(&mut done, "").on_hover_text(hint).clicked() {
                    out.push(Intent::Tool(
                        match row.done {
                            true => "checklist_reopen",
                            false => "checklist_complete",
                        },
                        row.text.clone(),
                    ));
                }

                let label = match row.done {
                    true => egui::RichText::new(&row.text).weak().strikethrough(),
                    false => egui::RichText::new(&row.text),
                };

                // The button is placed first and the text fills what is left,
                // wrapping onto as many lines as it needs.
                //
                // Laid out the other way round, a long item made the row wider
                // than the panel instead of taller: the text ran off the edge,
                // the button went with it, and the heading was pushed out of
                // view on the other side. Commanders write items like "go back
                // to Tod McQuin and engineer the remaining multicannons", so
                // this is the normal case rather than the extreme one.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    if ui
                        .small_button("Remove")
                        .on_hover_text("Take this off the list entirely")
                        .clicked()
                    {
                        out.push(Intent::Tool("checklist_remove", row.text.clone()));
                    }

                    // Turned back around before the text goes in. The outer
                    // layout exists to park the button on the right; laying the
                    // words out in it too would set them against the right edge,
                    // which for a wrapped item reads as a ragged left margin
                    // that moves line to line.
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
                        ui.add(egui::Label::new(label).wrap());
                    });
                });
            });
        }

        ui.add_space(8.0);

        let field =
            ui.add(egui::TextEdit::singleline(&mut local.adding).hint_text("Add something"));

        let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        if entered && !local.adding.trim().is_empty() {
            out.push(Intent::Tool(
                "checklist_add",
                local.adding.trim().to_string(),
            ));
            local.adding.clear();
            field.request_focus();
        }
    }
}

/// The live log.
///
/// **Read-only, and deliberately not a text area.** egui's selectable text is a
/// `TextEdit`, and free-form drag-selection across a log that grows underneath
/// the selection is a known weak spot — worse through a headset, where the
/// pointer is a ray at a metre and every tremor reads as a drag. These are
/// labels: they draw the same words, they wrap, they scroll, and there is
/// nothing to catch a stray drag on.
fn transcript(ui: &mut egui::Ui, view: &View) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if view.history.is_empty() && view.pending.is_none() {
                ui.add_space(24.0);
                ui.label("Ask Ward something to begin.");
            }

            for message in &view.history {
                let who = match message.role {
                    Role::User => "Commander",
                    Role::Assistant => "Ward",
                };

                ui.add_space(10.0);
                ui.label(egui::RichText::new(who).strong());
                ui.label(&message.text);
            }

            if let Some(pending) = view.pending.as_ref() {
                ui.add_space(10.0);
                ui.label(egui::RichText::new("Ward").strong());

                if let Some(tool) = view.using.as_ref() {
                    ui.weak(format!("using {tool}…"));
                }

                if !pending.is_empty() {
                    ui.label(pending);
                } else if view.using.is_none() {
                    ui.label("…");
                }
            }

            if let Some(problem) = view.problem.as_ref() {
                ui.add_space(10.0);
                ui.colored_label(ui.visuals().error_fg_color, problem);
            }

            // No caption here. There used to be one, as proof that the speech
            // stream reached a surface at all, and now that it reaches the
            // headset it has a real home. On a surface showing the conversation
            // it was printing every reply a second time in a fainter color — a
            // caption of what is on the screen you are looking at is not a
            // caption.
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mini_panel_names_the_next_thing_rather_than_counting_them() {
        // A count says there is work and nothing about what it is, so acting on it always costs a
        // look at the big panel - on the one surface whose whole job is to save that look.
        let view = View {
            panels: vec![shown::Shown::List {
                title: "Checklist".to_string(),
                rows: vec![
                    shown::Row {
                        text: "buy tritium at Deciat".to_string(),
                        done: true,
                    },
                    shown::Row {
                        text: "refuel at Jameson".to_string(),
                        done: false,
                    },
                    shown::Row {
                        text: "sell the cargo".to_string(),
                        done: false,
                    },
                ],
            }],
            ..View::default()
        };

        assert_eq!(
            next_up(&view),
            Some("refuel at Jameson"),
            "a finished item is not the next thing, and the second unfinished one is not either"
        );
    }

    #[test]
    fn a_finished_list_has_nothing_to_glance_at() {
        // Nothing rather than "0 things left". An empty row on a surface this small is a line of
        // furniture where a line of news should be.
        let view = View {
            panels: vec![shown::Shown::List {
                title: "Checklist".to_string(),
                rows: vec![shown::Row {
                    text: "buy tritium at Deciat".to_string(),
                    done: true,
                }],
            }],
            ..View::default()
        };

        assert_eq!(next_up(&view), None);
        assert_eq!(
            next_up(&View::default()),
            None,
            "and so does no list at all"
        );
    }

    #[test]
    fn listening_is_what_the_mini_panel_says_even_while_answering() {
        // The Commander is holding a key down. What they are waiting to see
        // confirmed is that Ward heard them start, not what it was doing before.
        let mut view = View {
            listening: true,
            streaming: true,
            ..View::default()
        };
        assert_eq!(state_of(&view), "listening");

        view.listening = false;
        assert_eq!(state_of(&view), "answering");

        view.using = Some("checklist".to_string());
        assert_eq!(state_of(&view), "looking something up");

        view.streaming = false;
        view.using = None;
        assert_eq!(state_of(&view), "ready");
    }
}
