//! Putting captions in the headset, and taking them away again.
//!
//! A thread of its own, because everything here runs on somebody else's clock.
//! SteamVR may not be running when Ward starts, may start later, and may stop
//! while Ward carries on — and none of that is allowed to be a problem for the
//! window, the voice or the turn.
//!
//! **It keeps asking.** Trying once and giving up is the specific way this
//! fails: Ward is started before the game, SteamVR comes up two minutes later,
//! and nothing ever appears with nothing to explain why. So the loop below is
//! written around the assumption that the headset is usually *not* there yet.
//!
//! **It disappears when there is nothing to say.** The overlay is hidden unless
//! a caption is live, so an empty caption layer is not a rectangle floating over
//! the cockpit — it is nothing at all.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::captions::Captions;

/// How often the caption layer is redrawn while something is on screen.
///
/// Captions are text that changes a few times a minute, not an animation. The
/// compositor holds the last image handed to it, so drawing more often than
/// this buys nothing and takes a slice of the machine Elite is drawing frames
/// with.
const REDRAW: Duration = Duration::from_millis(100);

/// How long to wait before asking SteamVR again.
///
/// Long enough that a Commander who never uses VR is not paying for a failing
/// call every second, short enough that starting SteamVR after Ward feels like
/// it worked rather than like it eventually worked.
const RETRY: Duration = Duration::from_secs(5);

/// How wide the caption layer is, in metres.
///
/// At the distance below this subtends about thirty degrees, which is roughly
/// what a subtitle band occupies on a television watched from a sofa. The first
/// attempt was 1.4 metres and read as enormous: at this distance that is
/// close to fifty degrees, so a caption filled the middle of the view and the
/// cockpit was behind it rather than around it.
const WIDTH: f32 = 0.9;

/// White on black, and neither is decoration.
///
/// A caption sits over a starfield, a station's floodlights and the cockpit's
/// own instruments. Text with nothing behind it is unreadable against half of
/// those, which is why broadcast captioning has always put it on a box.
///
/// Not quite opaque: enough to read against anything, little enough that the
/// Commander can still see what is behind it. Not quite white, because pure
/// white on black rings at this contrast.
const BOX: egui::Color32 = egui::Color32::from_rgba_premultiplied(0, 0, 0, 200);
const INK: egui::Color32 = egui::Color32::from_rgb(240, 240, 240);

/// How large the caption text is drawn, in points.
///
/// Sized so a full forty-two character line fills the layer's width rather than
/// chosen by eye. The layer is [`PIXELS`] wide and drawn at the renderer's own
/// scale, so this is the number that makes a standard line fit exactly.
const TEXT: f32 = 26.0;

/// Where the captions sit relative to the Commander's head, in metres.
///
/// Below the middle and a little way out. Captions belong under what you are
/// looking at rather than across it — this is a cockpit with a station in it,
/// and text over the middle of the view is text in the way of flying.
const AHEAD: f32 = 1.6;
const BELOW: f32 = 0.45;

/// The size of the image captions are drawn into.
///
/// Wide and short, because two lines of text is that shape. Drawing a square
/// and using a quarter of it would cost memory and sharpness for nothing.
const PIXELS: (u32, u32) = (1600, 260);

/// Starts the caption layer.
///
/// Returns immediately. Everything after this happens on its own thread, and
/// failure there costs the headset and nothing else: the window, the voice and
/// the turn carry on with no idea this exists.
pub fn spawn(captions: Arc<Mutex<Captions>>) {
    let started = std::thread::Builder::new()
        .name("ward-overlay".to_string())
        .spawn(move || run(&captions));

    if let Err(e) = started {
        tracing::warn!(target: "ward::vr", error = %e, "could not start the caption layer");
    }
}

fn run(captions: &Arc<Mutex<Captions>>) {
    // Said once. A Commander who does not use VR would otherwise get a line
    // every five seconds for the whole session saying so.
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

        if let Err(e) = show_captions(&session, captions) {
            tracing::warn!(target: "ward::vr", error = %e, "the caption layer stopped");
        }

        // Dropping the session shuts OpenVR down, which is what makes going
        // round again a fresh start rather than a second init on a runtime that
        // already has one.
        drop(session);
        std::thread::sleep(RETRY);
    }
}

/// Draws until something goes wrong or nobody is listening any more.
fn show_captions(
    session: &crate::vr::Vr,
    captions: &Arc<Mutex<Captions>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut renderer = crate::render::Renderer::new(PIXELS.0, PIXELS.1)?;

    tracing::info!(
        target: "ward::vr",
        adapter = %renderer.adapter_info().name,
        "drawing captions"
    );

    // The key is what SteamVR knows this overlay by, and a second copy of Ward
    // claiming it is how we learn one is already running.
    let overlay = session.create_overlay("dev.ward.captions", "Ward captions")?;

    overlay.set_width(WIDTH)?;
    overlay.follow_head(glam::Affine3A::from_translation(glam::Vec3::new(
        0.0, -BELOW, -AHEAD,
    )))?;
    overlay.hide()?;

    let mut visible = false;

    loop {
        let now = crate::diag::since_start();

        // Held only long enough to read. The window thread writes to this
        // whenever Ward says something, and it must never wait on a frame.
        let showing = {
            let Ok(mut captions) = captions.lock() else {
                return Ok(());
            };
            captions.tick(now);
            captions.showing().cloned()
        };

        let Some(caption) = showing else {
            if visible {
                overlay.hide()?;
                visible = false;
            }
            std::thread::sleep(REDRAW);
            continue;
        };

        renderer.draw(|ui| draw_caption(ui, &caption));

        let Some(image) = renderer.vulkan_image() else {
            return Err("the renderer produced no image OpenVR could read".into());
        };

        // SAFETY: the image belongs to the renderer above, which outlives this
        // loop, and the compositor reads it rather than taking ownership. It is
        // handed over after `draw` and before the next one starts, so nothing
        // is writing to it while OpenVR reads.
        unsafe { overlay.set_texture(&image) }?;

        if !visible {
            overlay.show()?;
            visible = true;
        }

        std::thread::sleep(REDRAW);
    }
}

/// One frame of caption.
///
/// White text on a black box, which is not a style choice. A caption sits over
/// a starfield, a station's floodlights and a cockpit's own instruments, and
/// text with nothing behind it is unreadable against half of them. The box is
/// what broadcast captioning has always used, for the same reason.
///
/// The box is drawn around the text rather than filling the layer, so an
/// overlay with one short line is one short bar rather than a black slab with a
/// sentence in the corner of it.
///
/// No border, no title, no controls. There is nothing here to interact with,
/// and anything that looks interactive in a caption layer is a lie — the layer
/// does not take input at all.
fn draw_caption(ui: &mut egui::Ui, caption: &crate::captions::Caption) {
    // Sits at the bottom of the layer, so a two-line caption grows upward and
    // the last line stays where the eye already is.
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
        egui::Frame::new()
            .fill(BOX)
            .inner_margin(egui::Margin::symmetric(14, 8))
            .corner_radius(2)
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;

                    for (at, line) in caption.lines.iter().enumerate() {
                        // The speaker is named once, on the first line, and
                        // never repeated down a caption that happens to wrap.
                        let text = match (at, caption.speaker) {
                            (0, Some(who)) => format!("{who}: {line}"),
                            _ => line.clone(),
                        };

                        ui.label(egui::RichText::new(text).size(TEXT).color(INK));
                    }
                });
            });
    });
}
