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
const WIDTH: f32 = 1.4;

/// Where the captions sit relative to the Commander's head, in metres.
///
/// Below the middle and a little way out. Captions belong under what you are
/// looking at rather than across it — this is a cockpit with a station in it,
/// and text over the middle of the view is text in the way of flying.
const AHEAD: f32 = 1.6;
const BELOW: f32 = 0.45;

/// The size of the image captions are drawn into.
///
/// Wide and short, because three lines of text is that shape. Drawing a square
/// and using a third of it would cost memory and sharpness for nothing.
const PIXELS: (u32, u32) = (1600, 400);

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

    let mut showing = false;

    loop {
        let now = crate::diag::since_start();

        // Held only long enough to read. The window thread writes to this
        // whenever Ward says something, and it must never wait on a frame.
        let (quiet, lines) = {
            let Ok(mut captions) = captions.lock() else {
                return Ok(());
            };
            captions.tick(now);
            (
                captions.quiet(),
                captions.lines().cloned().collect::<Vec<_>>(),
            )
        };

        if quiet {
            if showing {
                overlay.hide()?;
                showing = false;
            }
            std::thread::sleep(REDRAW);
            continue;
        }

        renderer.draw(|ui| draw_captions(ui, &lines));

        let Some(image) = renderer.vulkan_image() else {
            return Err("the renderer produced no image OpenVR could read".into());
        };

        // SAFETY: the image belongs to the renderer above, which outlives this
        // loop, and the compositor reads it rather than taking ownership. It is
        // handed over after `draw` and before the next one starts, so nothing
        // is writing to it while OpenVR reads.
        unsafe { overlay.set_texture(&image) }?;

        if !showing {
            overlay.show()?;
            showing = true;
        }

        std::thread::sleep(REDRAW);
    }
}

/// One frame of captions.
///
/// Deliberately plain. This is read at a metre, over a moving starfield, by
/// somebody whose attention is on flying — so it is light text on a dark slab
/// and nothing else. No border, no title, no controls: there is nothing here to
/// interact with, and anything that looks interactive in a caption layer is a
/// lie, because the layer does not take input at all.
fn draw_captions(ui: &mut egui::Ui, lines: &[crate::captions::Line]) {
    ui.vertical(|ui| {
        for line in lines {
            let text = match line.speaker {
                Some(who) => format!("{who}: {}", line.text),
                None => line.text.clone(),
            };

            ui.label(
                egui::RichText::new(text)
                    .size(34.0)
                    .color(egui::Color32::from_rgb(235, 235, 240)),
            );
            ui.add_space(6.0);
        }
    });
}
