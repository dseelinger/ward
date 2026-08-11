//! Putting Ward in the headset, and taking it away again.
//!
//! Two layers, on one thread, from one OpenVR session.
//!
//! - **Captions** are output only and ephemeral. They follow the head, they take no input, and
//!   when there is nothing to say there is nothing over the cockpit at all.
//! - **The panel** is furniture. It is put down in the room, it stays where it was put, and it
//!   can be pointed at and clicked. It draws [`crate::surface`], which is the same widget tree
//!   the window draws, so it cannot fall behind what the window can do.
//!
//! A thread of its own, because everything here runs on somebody else's clock. SteamVR may not be
//! running when Ward starts, may start later, and may stop while Ward carries on — and none of
//! that is allowed to be a problem for the window, the voice or the turn.
//!
//! **It keeps asking.** Trying once and giving up is the specific way this fails: Ward is started
//! before the game, SteamVR comes up two minutes later, and nothing ever appears with nothing to
//! explain why. So the loop below is written around the assumption that the headset is usually
//! *not* there yet.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::captions::Captions;
use crate::engine::Handle;
use crate::listen::{Gate, PushToTalk};
use crate::surface::Mode;

/// How often the caption layer is redrawn while something is on screen.
///
/// Captions are text that changes a few times a minute, not an animation. The compositor holds the
/// last image handed to it, so drawing more often than this buys nothing and takes a slice of the
/// machine Elite is drawing frames with.
const REDRAW: Duration = Duration::from_millis(100);

/// How often the panel is redrawn while it is showing.
///
/// Faster than the captions, and for one reason: a pointer that updates ten times a second reads
/// as broken. This is the rate a cursor has to move at to feel attached to the hand, and it is
/// paid only while the panel is actually on screen — a hidden panel costs nothing at all.
const POINTING: Duration = Duration::from_millis(16);

/// How long to wait before asking SteamVR again.
///
/// Long enough that a Commander who never uses VR is not paying for a failing call every second,
/// short enough that starting SteamVR after Ward feels like it worked rather than like it
/// eventually worked.
const RETRY: Duration = Duration::from_secs(5);

/// How wide the caption layer is, in metres.
///
/// At the distance below this subtends about thirty degrees, which is roughly what a subtitle band
/// occupies on a television watched from a sofa. The first attempt was 1.4 metres and read as
/// enormous: at this distance that is close to fifty degrees, so a caption filled the middle of
/// the view and the cockpit was behind it rather than around it.
const WIDTH: f32 = 0.9;

/// How wide each mode of the panel is, in metres.
///
/// The big one is a screen you turn to look at, near enough arm's length. The mini one is a watch
/// face: it carries four short lines and is meant to be read without moving your head, so it is
/// small enough that the cockpit is still the thing you are looking at.
const BIG_WIDTH: f32 = 1.1;
const MINI_WIDTH: f32 = 0.34;

/// White on black, and neither is decoration.
///
/// A caption sits over a starfield, a station's floodlights and the cockpit's own instruments.
/// Text with nothing behind it is unreadable against half of those, which is why broadcast
/// captioning has always put it on a box.
///
/// Not quite opaque: enough to read against anything, little enough that the Commander can still
/// see what is behind it. Not quite white, because pure white on black rings at this contrast.
const BOX: egui::Color32 = egui::Color32::from_rgba_premultiplied(0, 0, 0, 200);
const INK: egui::Color32 = egui::Color32::from_rgb(240, 240, 240);

/// How large the caption text is drawn, in points.
///
/// Sized so a full forty-two character line fills the layer's width rather than chosen by eye. The
/// layer is [`PIXELS`] wide and drawn at the renderer's own scale, so this is the number that
/// makes a standard line fit exactly.
const TEXT: f32 = 26.0;

/// Where the captions sit relative to the Commander's head, in metres.
///
/// Below the middle and a little way out. Captions belong under what you are looking at rather
/// than across it — this is a cockpit with a station in it, and text over the middle of the view
/// is text in the way of flying.
const AHEAD: f32 = 1.6;
const BELOW: f32 = 0.45;

/// How far away the panel is put when it is summoned, in metres.
///
/// Further than the captions, because it is read deliberately rather than glanced at, and a large
/// surface close to the face is one you have to move your head across to read.
const PANEL_AHEAD: f32 = 1.5;

/// The size of the image captions are drawn into.
///
/// Wide and short, because two lines of text is that shape. Drawing a square and using a quarter
/// of it would cost memory and sharpness for nothing.
const PIXELS: (u32, u32) = (1600, 260);

/// The size of the image the panel is drawn into.
///
/// The same shape as the window's own minimum, so the one widget tree lays out the same way in
/// both places. A panel narrower than this puts the checklist and the conversation into a fight
/// for the width, which is the thing the window's minimum size exists to prevent.
const PANEL_PIXELS: (u32, u32) = (1900, 1200);

/// The size of the image the mini panel is drawn into.
///
/// Not a crop of the big one and not the big one scaled. An overlay's text size in the headset
/// comes out of its pixels and its width in metres together, and these two numbers are chosen so
/// that a line on the mini panel subtends about what a line on the big one does — which is the
/// whole point of a surface meant to be read without looking directly at it.
const MINI_PIXELS: (u32, u32) = (640, 280);

/// How close a controller has to be to count as touching the panel, in metres.
///
/// Generous, because this is a grab rather than a pinch: the Commander is reaching for a surface
/// they can see, and being told they missed it by four centimetres is not useful feedback.
const REACH: f32 = 0.35;

/// How long a mode change is left alone before another gesture can be read.
///
/// A grab is held across many frames and a flick is a hand that stays fast for a tenth of a
/// second. Without a pause after each change, one movement reads as several.
const SETTLING: Duration = Duration::from_millis(400);

/// How fast a hand has to be moving to count as a flick, in metres per second.
///
/// Above what a hand does while holding something still, and below what it takes to throw
/// something. Set too low, putting the panel down dismisses it.
const FLICK: f32 = 1.6;

/// Starts the caption layer and the panel.
///
/// Returns immediately. Everything after this happens on its own thread, and failure there costs
/// the headset and nothing else: the window, the voice and the turn carry on with no idea this
/// exists.
pub fn spawn(captions: Arc<Mutex<Captions>>, engine: Handle) {
    let started = std::thread::Builder::new()
        .name("ward-overlay".to_string())
        .spawn(move || run(&captions, &engine));

    if let Err(e) = started {
        tracing::warn!(target: "ward::vr", error = %e, "could not start the headset layers");
    }
}

fn run(captions: &Arc<Mutex<Captions>>, engine: &Handle) {
    // Said once. A Commander who does not use VR would otherwise get a line every five seconds for
    // the whole session saying so.
    let mut complained = false;

    loop {
        let session = match crate::vr::Vr::start() {
            Ok(session) => session,
            Err(e) => {
                if !complained {
                    tracing::info!(
                        target: "ward::vr",
                        reason = %e,
                        "no headset yet, and Ward will keep looking"
                    );
                    complained = true;
                }
                std::thread::sleep(RETRY);
                continue;
            }
        };

        tracing::info!(target: "ward::vr", "headset found");
        complained = false;

        if let Err(e) = serve(&session, captions, engine) {
            tracing::warn!(target: "ward::vr", error = %e, "the headset layers stopped");
        }

        // Dropping the session shuts OpenVR down, which is what makes going round again a fresh
        // start rather than a second init on a runtime that already has one.
        drop(session);
        std::thread::sleep(RETRY);
    }
}

/// Draws both layers until something goes wrong or nobody is listening any more.
fn serve(
    session: &crate::vr::Vr,
    captions: &Arc<Mutex<Captions>>,
    engine: &Handle,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut caption_art = crate::render::Renderer::new(PIXELS.0, PIXELS.1)?;
    let mut panel_art = crate::render::Renderer::new(PANEL_PIXELS.0, PANEL_PIXELS.1)?;

    tracing::info!(
        target: "ward::vr",
        adapter = %panel_art.adapter_info().name,
        "drawing captions and the panel"
    );

    // The key is what SteamVR knows an overlay by, and a second copy of Ward claiming it is how we
    // learn one is already running.
    let caption_layer = session.create_overlay("dev.ward.captions", "Ward captions")?;

    caption_layer.set_width(WIDTH)?;
    caption_layer.follow_head(glam::Affine3A::from_translation(glam::Vec3::new(
        0.0, -BELOW, -AHEAD,
    )))?;
    caption_layer.hide()?;

    // Input is set up per mode rather than here, because the mouse scale is the drawn image's own
    // pixels and the two modes are drawn at different sizes.
    let panel_layer = session.create_overlay("dev.ward.panel", "Ward")?;
    panel_layer.hide()?;

    let mut showing_caption = false;
    let mut panel = Panel::default();

    loop {
        let now = crate::diag::since_start();

        // Held only long enough to read. The window thread writes to this whenever Ward says
        // something, and it must never wait on a frame.
        let showing = {
            let Ok(mut captions) = captions.lock() else {
                return Ok(());
            };
            captions.tick(now);
            captions.showing().cloned()
        };

        match showing {
            Some(caption) => {
                caption_art.draw(|ui| draw_caption(ui, &caption));

                let Some(image) = caption_art.vulkan_image() else {
                    return Err("the renderer produced no image OpenVR could read".into());
                };

                // SAFETY: the image belongs to the renderer above, which outlives this loop, and
                // the compositor reads it rather than taking ownership. It is handed over after
                // `draw` and before the next one starts, so nothing is writing to it while OpenVR
                // reads.
                unsafe { caption_layer.set_texture(&image) }?;

                if !showing_caption {
                    caption_layer.show()?;
                    showing_caption = true;
                }
            }
            None => {
                if showing_caption {
                    caption_layer.hide()?;
                    showing_caption = false;
                }
            }
        }

        panel.frame(session, &panel_layer, &mut panel_art, engine)?;

        // Idle at the caption's pace and wake up for the panel. A hidden panel is the usual state,
        // and paying sixty frames a second for it would be paying for a picture nobody is looking
        // at while Elite is drawing the one they are.
        std::thread::sleep(match panel.mode.showing() {
            true => POINTING,
            false => REDRAW,
        });
    }
}

/// Everything the panel is, between frames.
#[derive(Default)]
struct Panel {
    mode: Mode,
    local: crate::surface::Local,
    /// Where it is in the room. Absent until it has been summoned once.
    placed: Option<glam::Affine3A>,
    /// Whether SteamVR currently believes it is on screen, so show and hide are called on the
    /// change rather than every frame.
    visible: bool,
    /// What SteamVR has already been told about size and position, so the calls that change them
    /// are made when something changes rather than sixty times a second.
    applied_mode: Option<Mode>,
    applied_place: Option<glam::Affine3A>,
    /// Whether the trigger is down, which the renderer needs as a level and OpenVR reports as two
    /// edges.
    pressed: bool,
    /// A hand that is holding the panel, and where it was when it took hold.
    held: Option<Grab>,
    /// The key that summons the panel, and what it last read — so the press is acted on and the
    /// hold is not. Absent when the setting names a key Ward cannot resolve, which is a hotkey
    /// that does nothing rather than one that fires constantly.
    summon: Option<PushToTalk>,
    /// Which key name `summon` was built from, so a Commander who changes the setting gets the new
    /// key without restarting. In a headset a restart means taking it off.
    summon_named: String,
    summon_down: bool,
    /// When the mode last changed on its own, so one gesture cannot fire twice.
    settled: Option<Instant>,
}

/// A hand that has taken hold of the panel.
#[derive(Clone, Copy)]
struct Grab {
    /// Where the panel was relative to the hand when it was grabbed, so it can be carried without
    /// snapping its center to the controller.
    offset: glam::Vec3,
}

impl Panel {
    /// One frame of the panel: what was asked for, what is pointed at, and what to draw.
    fn frame(
        &mut self,
        session: &crate::vr::Vr,
        layer: &crate::vr::Overlay<'_>,
        art: &mut crate::render::Renderer,
        engine: &Handle,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let view = engine.view();

        self.summoned(session, &view);
        self.gestured(session);

        if !self.mode.showing() {
            if self.visible {
                layer.hide()?;
                self.visible = false;
                self.applied_mode = None;
                self.applied_place = None;
            }
            return Ok(());
        }

        self.settle(layer, art)?;
        self.pointed(layer, art);

        let mode = self.mode;
        let wants = art.draw_with(|ui| crate::surface::show(ui, &view, &mut self.local, mode));

        for intent in wants {
            engine.tell(intent);
        }

        let Some(image) = art.vulkan_image() else {
            return Err("the renderer produced no image OpenVR could read".into());
        };

        // SAFETY: the same contract as the caption layer above. The image belongs to a renderer
        // that outlives this loop, and it is handed over between draws.
        unsafe { layer.set_texture(&image) }?;

        if !self.visible {
            layer.show()?;
            self.visible = true;
        }

        Ok(())
    }

    /// Tells SteamVR anything about size or position that has changed since last frame.
    ///
    /// Both are set on the change rather than every frame. They are cheap calls and this is not
    /// about the cost — it is that a position pushed every frame would fight the one the
    /// Commander is currently dragging it to, and the panel would stick to where it was summoned.
    fn settle(
        &mut self,
        layer: &crate::vr::Overlay<'_>,
        art: &mut crate::render::Renderer,
    ) -> Result<(), crate::vr::Error> {
        if self.applied_mode != Some(self.mode) {
            // Mini is smaller in the room as well as carrying less. Making it narrower without
            // making it a different picture would be the panel scaled down, which is the thing
            // mini is deliberately not.
            let (pixels, width) = match self.mode {
                Mode::Mini => (MINI_PIXELS, MINI_WIDTH),
                _ => (PANEL_PIXELS, BIG_WIDTH),
            };

            art.resize(pixels.0, pixels.1);
            // Told again, because the mouse scale is the image's own pixels and the image just
            // changed. Skipped, the ray would land somewhere else entirely on the mini panel.
            layer.take_input(pixels.0, pixels.1)?;
            layer.set_width(width)?;
            self.applied_mode = Some(self.mode);
        }

        if let Some(placed) = self.placed
            && self.applied_place != Some(placed)
        {
            layer.place(placed)?;
            self.applied_place = Some(placed);
        }

        Ok(())
    }

    /// Takes what the ray is doing and hands it to the toolkit.
    fn pointed(&mut self, layer: &crate::vr::Overlay<'_>, art: &mut crate::render::Renderer) {
        // The image's own height, because that is what the Y flip is measured against and the two
        // modes are not the same size.
        for event in layer.events(art.size().1) {
            match event {
                crate::vr::Event::Moved { x, y } => art.point_at(Some((x, y))),
                crate::vr::Event::Button { down } => {
                    self.pressed = down;
                    art.press(down);
                }
                crate::vr::Event::Scrolled { x, y } => art.scroll(x, y),
                crate::vr::Event::Left => {
                    art.point_at(None);
                    // The button is not released here. egui keeps it down on purpose when the
                    // pointer leaves, so that dragging a slider survives leaving the viewport, and
                    // saying otherwise would end a drag every time the ray wobbled off the edge.
                }
            }
        }
    }

    /// Opens or closes the panel because something asked for it.
    ///
    /// Two of the three ways in: what the engine was told by voice, and the hotkey. The third is
    /// the gesture, which is below because it needs the controllers.
    fn summoned(&mut self, session: &crate::vr::Vr, view: &crate::engine::View) {
        // Asked for by voice. The engine counts requests rather than holding a mode, because it
        // does not know whether a headset exists and must not have an opinion about one.
        if view.panel_asks != self.local.answered_asks {
            self.local.answered_asks = view.panel_asks;
            self.change(view.panel_wanted);
        }

        // Rebuilt only when the setting changes, because resolving a key name goes through the
        // keyboard layout and there is no reason to do that sixty times a second.
        let named = view.settings.string("panel key");
        if named != self.summon_named {
            self.summon = PushToTalk::on(&named);

            if self.summon.is_none() {
                tracing::warn!(
                    target: "ward::vr",
                    key = %named,
                    "no such key, so the panel cannot be summoned by hotkey"
                );
            }

            self.summon_named = named;
        }

        let down = self
            .summon
            .as_mut()
            .is_some_and(|key| key.open(crate::diag::since_start()));

        // The press, not the hold. A key read as held would toggle the panel sixty times a second
        // for as long as a finger stayed on it.
        if down && !self.summon_down {
            self.change(self.mode.toggled());
        }
        self.summon_down = down;

        // Placed the moment it is asked for rather than kept from last time, so it always arrives
        // where the Commander is looking now. A panel that came back where it was an hour ago is
        // one behind you.
        if self.mode.showing() && self.placed.is_none() {
            self.placed = Some(in_front_of(session));
        }
    }

    /// Grab from mini to expand, and flick to dismiss.
    fn gestured(&mut self, session: &crate::vr::Vr) {
        let Some(placed) = self.placed else {
            return;
        };

        // A gesture is many frames long. Without this, the frames after the one that opened the
        // panel are still a hand moving fast with the grip held, and the same movement would be
        // read as a grab, an expand and a flick in the space of a fifth of a second.
        if self.settled.is_some_and(|at| at.elapsed() < SETTLING) {
            return;
        }

        let controllers = session.controllers();

        // Whichever hand is holding it, or the nearest one reaching for it with the grip held.
        let grabbing = controllers.iter().find(|hand| {
            hand.grip
                && (self.held.is_some() || hand.at.distance(placed.translation.into()) < REACH)
        });

        match (grabbing, self.held) {
            // Taking hold. The offset is remembered so the panel does not snap its center onto the
            // controller the instant it is touched.
            (Some(hand), None) => {
                self.held = Some(Grab {
                    offset: glam::Vec3::from(placed.translation) - hand.at,
                });
            }
            // Carrying it. Moved rather than reoriented: a panel that rolled with the wrist would
            // be one you cannot put down level.
            (Some(hand), Some(grab)) => {
                let at = hand.at + grab.offset;
                self.placed = Some(glam::Affine3A::from_rotation_translation(
                    facing(session, at),
                    at,
                ));

                // Grabbing the mini panel is how it becomes the big one. It is the same surface
                // being pulled open, which is why this is a mode and not a second overlay.
                if self.mode == Mode::Mini {
                    self.change(Mode::Big);
                }
            }
            // Let go. Fast enough and it was thrown away rather than put down.
            (None, Some(_)) => {
                self.held = None;

                let thrown = controllers.iter().any(|hand| hand.speed.length() > FLICK);

                if thrown {
                    tracing::info!(target: "ward::vr", "panel flicked away");
                    self.change(Mode::Gone);
                }
            }
            (None, None) => {}
        }
    }

    /// Moves to a mode, and does nothing if it is already there.
    ///
    /// The guard is what keeps one gesture from being read as several. A grab is held across many
    /// frames and a flick is a hand that stays fast for a tenth of a second, so acting on every
    /// frame that matches would open and close the panel repeatedly from one movement.
    fn change(&mut self, to: Mode) {
        if self.mode == to {
            return;
        }

        // Forgotten on the way out, so the next summon places it in front of wherever the
        // Commander is then rather than where they were.
        if to == Mode::Gone {
            self.placed = None;
            self.held = None;
        }

        tracing::info!(target: "ward::vr", from = ?self.mode, to = ?to, "panel");
        self.mode = to;
        self.settled = Some(Instant::now());
    }
}

/// A pose a little way in front of the Commander, turned to face them.
fn in_front_of(session: &crate::vr::Vr) -> glam::Affine3A {
    let Some(head) = session.head() else {
        // No headset pose yet. Put it where a headset would be if there were one, so the panel
        // exists and can be found rather than being nowhere at all.
        return glam::Affine3A::from_translation(glam::Vec3::new(0.0, 1.6, -PANEL_AHEAD));
    };

    // Along the direction being looked, rather than at a fixed point in the room. Negative Z is
    // forward for every tracked device in OpenVR.
    let forward = -glam::Vec3::from(head.matrix3.z_axis);
    let at = glam::Vec3::from(head.translation) + forward * PANEL_AHEAD;

    glam::Affine3A::from_rotation_translation(facing(session, at), at)
}

/// Which way something at `at` has to be turned to face the Commander.
///
/// Upright rather than tilted, deliberately. A panel that pitched to face a head looking down at
/// the radar would be one leaning back at the sky, and text on it reads worse than text on
/// something level.
fn facing(session: &crate::vr::Vr, at: glam::Vec3) -> glam::Quat {
    let Some(head) = session.head() else {
        return glam::Quat::IDENTITY;
    };

    let head_at = glam::Vec3::from(head.translation);
    let away = at - head_at;
    let flat = glam::Vec3::new(away.x, 0.0, away.z);

    // Directly above or below the head, where there is no direction to turn towards. Left as it
    // was rather than snapped to an arbitrary bearing.
    if flat.length_squared() < f32::EPSILON {
        return glam::Quat::IDENTITY;
    }

    // The overlay's own forward is negative Z, so it has to be turned to point back the way it
    // came from.
    glam::Quat::from_rotation_arc(glam::Vec3::NEG_Z, flat.normalize())
}

/// One frame of caption.
///
/// White text on a black box, which is not a style choice. A caption sits over a starfield, a
/// station's floodlights and a cockpit's own instruments, and text with nothing behind it is
/// unreadable against half of them. The box is what broadcast captioning has always used, for the
/// same reason.
///
/// The box is drawn around the text rather than filling the layer, so an overlay with one short
/// line is one short bar rather than a black slab with a sentence in the corner of it.
///
/// No border, no title, no controls. There is nothing here to interact with, and anything that
/// looks interactive in a caption layer is a lie — the layer does not take input at all.
pub(crate) fn draw_caption(ui: &mut egui::Ui, caption: &crate::captions::Caption) {
    // An area rather than a layout, and that is the whole of the fix for a box that was drawing
    // four lines tall for two lines of words. A layout is handed the space it sits in - the entire
    // layer, here - and a frame around it grows to match, so the black box was the overlay rather
    // than the caption. An area is measured by what is put in it.
    //
    // Anchored to the bottom, so a two line caption grows upward and the last line stays where the
    // eye already is.
    egui::Area::new(egui::Id::new("ward-caption"))
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -4.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(BOX)
                .inner_margin(egui::Margin {
                    left: 14,
                    right: 14,
                    top: 2,
                    bottom: 2,
                })
                .corner_radius(2)
                .show(ui, |ui| {
                    // No gap between the two lines of one caption. The box is as tall as the words
                    // and no taller.
                    ui.spacing_mut().item_spacing.y = 0.0;

                    // Never wrap. The lines arrive already broken to the caption standard, so
                    // there is nothing left to decide - and letting the toolkit decide it again is
                    // a loop that eats itself: an area is as wide as its contents, a narrower area
                    // wraps the text, wrapped text is more lines, more lines is a narrower and
                    // taller box, and the caption reflows in front of the Commander until it is
                    // four lines of ribbon down the middle of the cockpit.
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

                    for (at, line) in caption.lines.iter().enumerate() {
                        // The speaker is named once, on the first line, and never repeated down a
                        // caption that happens to wrap.
                        let text = match (at, caption.speaker) {
                            (0, Some(who)) => format!("{who}: {line}"),
                            _ => line.clone(),
                        };

                        ui.label(egui::RichText::new(text).size(TEXT).color(INK));
                    }
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draws a caption and measures the box it drew.
    ///
    /// Permanently ignored: it needs a Vulkan device. Run it when something about the caption
    /// looks wrong. "The box is too tall" cannot be measured through a headset and can be measured
    /// exactly here - it was four lines tall for two lines of words, and the layout was the
    /// reason.
    ///
    /// The bitmap is for geometry, not for color: the transparent parts are filled with a flat
    /// grey that stands in for a cockpit, so contrast in the file is not contrast in the headset.
    ///
    /// ```text
    /// cargo test caption_to_look_at -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs a Vulkan device"]
    fn caption_to_look_at() {
        let mut renderer = crate::render::Renderer::new(PIXELS.0, PIXELS.1).expect("no renderer");

        let caption = crate::captions::Caption {
            lines: vec![
                "The Thargoids are an ancient alien species".to_string(),
                "humanity first clashed with centuries ago.".to_string(),
            ],
            speaker: None,
        };

        // Settled rather than drawn twice. Two passes are enough to place an anchored area from
        // the size it had last frame, and not enough to outlast the toolkit's fade - so a
        // two-frame read measures the geometry correctly and lies about every color in it.
        for _ in 0..SETTLE_FRAMES {
            renderer.draw(|ui| draw_caption(ui, &caption));
        }

        let pixels = renderer.read_back();
        let (width, height) = (PIXELS.0 as usize, PIXELS.1 as usize);

        // Where the box actually is, measured rather than guessed: the first and last row with
        // anything drawn on it.
        let opaque = |row: usize| (0..width).any(|x| pixels[(row * width + x) * 4 + 3] > 8);
        let top = (0..height).find(|r| opaque(*r));
        let bottom = (0..height).rev().find(|r| opaque(*r));

        if let (Some(top), Some(bottom)) = (top, bottom) {
            let tall = bottom - top + 1;
            println!(
                "box is {tall} of {height} pixels tall, rows {top}..{bottom}, \
                 which is {:.1} lines of {}pt text",
                tall as f32 / (TEXT * crate::render::SCALE),
                TEXT
            );
        } else {
            println!("nothing was drawn at all");
        }

        let out = std::env::temp_dir().join("ward-caption.bmp");
        write_bitmap(&out, &pixels, PIXELS.0, PIXELS.1);
        println!("written to {}", out.display());
    }

    /// Draws the panel and writes it out, so what the headset shows can be looked at flat.
    ///
    /// Permanently ignored for the same reason as the caption above. This is the only way to see
    /// the panel's layout without putting a headset on, and layout is most of what goes wrong: a
    /// checklist that pushes the conversation into a strip looks fine in a window at 980 points
    /// and wrong at [`PANEL_PIXELS`].
    ///
    /// ```text
    /// cargo test panel_to_look_at -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs a Vulkan device"]
    fn panel_to_look_at() {
        let mut renderer =
            crate::render::Renderer::new(PANEL_PIXELS.0, PANEL_PIXELS.1).expect("no renderer");

        let view = crate::engine::View {
            key_stored: true,
            history: vec![
                crate::anthropic::Message {
                    role: crate::anthropic::Role::User,
                    text: "how far to Colonia".to_string(),
                },
                crate::anthropic::Message {
                    role: crate::anthropic::Role::Assistant,
                    text: "Twenty two thousand light years, Commander. About four \
                           hundred jumps in this ship."
                        .to_string(),
                },
            ],
            ..crate::engine::View::default()
        };

        // A list with something left on it, because the next thing to do is one of the three
        // things the mini panel carries and an empty one would show nothing at all.
        let view = crate::engine::View {
            panels: vec![crate::shown::Shown::List {
                title: "Checklist".to_string(),
                rows: vec![
                    crate::shown::Row {
                        text: "buy tritium at Deciat".to_string(),
                        done: true,
                    },
                    crate::shown::Row {
                        text: "refuel at Jameson Memorial".to_string(),
                        done: false,
                    },
                ],
            }],
            ..view
        };

        let mut local = crate::surface::Local::default();

        // Enough frames for the toolkit to settle, not two. An anchored area is placed from the
        // size it had last frame, which takes two - but egui also fades a new area in over about
        // a tenth of a second, and a two-frame render catches the panel at the start of that. It
        // reads as a contrast bug that is not in the panel at all.
        for _ in 0..SETTLE_FRAMES {
            renderer.draw(|ui| {
                crate::surface::show(ui, &view, &mut local, Mode::Big);
            });
        }

        let out = std::env::temp_dir().join("ward-panel.bmp");
        write_bitmap(&out, &renderer.read_back(), PANEL_PIXELS.0, PANEL_PIXELS.1);
        println!("written to {}", out.display());

        // The mini mode, at its own size. Drawn from the same picture and the same tree, so what
        // this shows is the reduction rather than a second design - and the thing worth looking
        // at is whether four short lines still read at a third of a metre.
        renderer.resize(MINI_PIXELS.0, MINI_PIXELS.1);

        for _ in 0..SETTLE_FRAMES {
            renderer.draw(|ui| {
                crate::surface::show(ui, &view, &mut local, Mode::Mini);
            });
        }

        let small = std::env::temp_dir().join("ward-panel-mini.bmp");
        write_bitmap(&small, &renderer.read_back(), MINI_PIXELS.0, MINI_PIXELS.1);
        println!("written to {}", small.display());
    }

    /// How many frames to draw before reading one back.
    ///
    /// At the toolkit's default of a sixtieth of a second per frame this is a third of a second,
    /// which outlasts every fade and settles every anchored area.
    const SETTLE_FRAMES: usize = 20;

    /// A bitmap, because it needs no library and any viewer opens one.
    #[cfg(test)]
    fn write_bitmap(path: &std::path::Path, rgba: &[u8], width: u32, height: u32) {
        let row = (width * 3).div_ceil(4) * 4;
        let size = 54 + row * height;

        let mut out: Vec<u8> = Vec::with_capacity(size as usize);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(width as i32).to_le_bytes());
        out.extend_from_slice(&(height as i32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        for _ in 0..6 {
            out.extend_from_slice(&0u32.to_le_bytes());
        }

        // Bitmaps run bottom to top, and the caption is drawn over whatever is behind it - so the
        // transparent parts are filled with a mid grey to stand in for a cockpit rather than
        // reading as white.
        for y in (0..height).rev() {
            let mut written = 0;
            for x in 0..width {
                let at = ((y * width + x) * 4) as usize;
                let (r, g, b, a) = (
                    rgba[at] as u32,
                    rgba[at + 1] as u32,
                    rgba[at + 2] as u32,
                    rgba[at + 3] as u32,
                );
                let over = |c: u32| ((c * a + 90 * (255 - a)) / 255) as u8;
                out.extend_from_slice(&[over(b), over(g), over(r)]);
                written += 3;
            }
            while written < row {
                out.push(0);
                written += 1;
            }
        }

        std::fs::write(path, out).expect("could not write the bitmap");
    }
}
