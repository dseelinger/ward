//! Drawing the overlay into an image, with no window and nothing to present to.
//!
//! wgpu on Vulkan, egui on top of it, and a texture as the only output. There is
//! no window here and there is not going to be one: in VR the image goes to SteamVR, and the
//! desktop window is drawn by the toolkit itself and has nothing to do with this path.
//!
//! # Two constraints that come from OpenVR rather than from wgpu
//!
//! The compositor demands a list of Vulkan instance extensions, including `VK_KHR_surface` and
//! `VK_KHR_win32_surface`, which wgpu only requests when it has a surface to present to. Nothing
//! in `wgpu::InstanceDescriptor` can add them, so the handoff has to build the instance through
//! `wgpu-hal` rather than through the public API. That is a known and deliberate seam: everything
//! below is the plain path, and the handoff replaces instance creation alone.
//!
//! The device extensions the compositor asks for are not injected either, and nothing here reads
//! the list. `request_device` takes a `wgpu::DeviceDescriptor`, which cannot name a Vulkan
//! extension, so injecting them means building the device through `wgpu-hal` as well. The handoff
//! works without it because `wgpu-hal` enables `VK_KHR_external_memory_win32` whenever the
//! physical device supports it, and Vulkan promoted most of the rest of the usual list into core
//! long before 1.4. What that leaves exposed is anything the compositor demands that is neither
//! core nor in wgpu's own set, and a wgpu upgrade that drops that opt-in. Nothing in the tier
//! structure can notice either, which is the part worth knowing.

use std::fmt;

use egui_wgpu::{RendererOptions, ScreenDescriptor};

/// How many pixels one egui point occupies.
///
/// Captions are read at a metre, not at desk distance, so egui's desktop defaults come out
/// around 0.6 degrees of arc — small enough that a headset session called them unreadable. This
/// scales everything at once rather than resizing each text style by hand.
pub const SCALE: f32 = 2.5;

/// The format the overlay is drawn in.
///
/// sRGB because that is what OpenVR expects to be told about through
/// `SetOverlayTextureColorSpace`, and because egui's own output assumes it.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// The same format as `FORMAT`, spelled the way Vulkan spells it.
///
/// OpenVR is told the raw `VkFormat` rather than anything of wgpu's, so the two have to be kept
/// in step by hand. They are next to each other for that reason.
const VULKAN_FORMAT: ash::vk::Format = ash::vk::Format::R8G8B8A8_SRGB;

/// What can go wrong bringing the renderer up.
#[derive(Debug)]
pub enum Error {
    /// No Vulkan adapter at all. Either the driver is missing or the backend is unavailable.
    NoVulkanAdapter,
    /// An adapter exists but refused to give us a device.
    NoDevice(wgpu::RequestDeviceError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoVulkanAdapter => write!(
                f,
                "no Vulkan adapter. The overlay hands SteamVR a Vulkan texture, so there is no \
                 fallback backend worth taking."
            ),
            Self::NoDevice(error) => write!(f, "the Vulkan adapter refused a device: {error}"),
        }
    }
}

impl std::error::Error for Error {}

/// One Vulkan device, shared by every layer in the headset.
///
/// **There is exactly one of these per process, and that is not a saving.** OpenVR keeps its
/// Vulkan state per process rather than per texture, so handing the compositor images from two
/// devices corrupts it — and it does not fail politely. Two renderers, each with a device of its
/// own, ran perfectly until the first caption appeared while the panel was also up, and then took
/// `vrclient_x64.dll` down with an access violation inside OpenVR's own code.
///
/// That is the specific reason this type exists. The layers above own their own image, their own
/// widget tree and their own input; they borrow the one device underneath.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_info: wgpu::AdapterInfo,
}

impl Gpu {
    /// Brings up Vulkan on the best adapter available.
    ///
    /// # Errors
    ///
    /// Fails if there is no Vulkan adapter, or if the one found will not give up a device.
    pub fn new() -> Result<Self, Error> {
        // Vulkan only. DX12 would work for drawing and then have nothing to hand OpenVR, which
        // is a failure three steps downstream of its cause.
        // No display handle: there is no window and there is never going to be one here.
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::VULKAN;
        // Validation on in a debug build and off in a release one, which is what
        // `from_build_config` means. So `cargo test` watches and the muster run does not pay for
        // it, and the tier 1 test is what a wrong layout has to get past.
        descriptor.flags = wgpu::InstanceFlags::from_build_config();
        let instance = wgpu::Instance::new(descriptor);

        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN));
        let adapter = pick_adapter(adapters).ok_or(Error::NoVulkanAdapter)?;
        let adapter_info = adapter.get_info();

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("ward overlay"),
            ..Default::default()
        }))
        .map_err(Error::NoDevice)?;

        Ok(Self {
            device,
            queue,
            adapter_info,
        })
    }

    /// Which adapter this ended up on, and over which backend.
    #[must_use]
    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }
}

/// Draws egui into an offscreen image.
pub struct Renderer {
    gpu: std::sync::Arc<Gpu>,
    target: wgpu::Texture,
    /// Destination for the one-texel copy that ends each frame. Written, never read.
    settled: wgpu::Buffer,
    size: (u32, u32),
    context: egui::Context,
    painter: egui_wgpu::Renderer,
    pointer: Option<egui::Pos2>,
    pressed: bool,
    was_pressed: bool,
    /// Scrolling that arrived since the last frame, waiting to be delivered.
    scrolled: egui::Vec2,
    /// What was typed since the last frame, already in the toolkit's terms.
    typing: Vec<egui::Event>,
    /// What a cut or a copy asked to be put on the clipboard, waiting to be collected.
    copied: Option<String>,
}

impl Renderer {
    /// Makes an image to draw into, on a device somebody else owns.
    ///
    /// Infallible, because everything that can go wrong went wrong in [`Gpu::new`]. That is the
    /// point of the split: bringing Vulkan up is a thing that happens once and can fail, and
    /// adding a layer to a device that already works cannot.
    pub fn new(gpu: &std::sync::Arc<Gpu>, width: u32, height: u32) -> Self {
        let device = &gpu.device;

        let target = make_target(device, width, height);
        let settled = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay layout settle"),
            size: 4,
            usage: wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let painter = egui_wgpu::Renderer::new(device, FORMAT, RendererOptions::default());

        // Tell egui it is being pointed at rather than moused at.
        //
        // egui rejects a click whose pointer drifted more than `max_click_dist` (6 points) or whose
        // press lasted longer than `max_click_duration` (0.8s). Both are tuned for a hand resting on
        // a desk. A ray at a metre turns ordinary tremor into more drift than that, and a deliberate
        // press outlasts the duration, so roughly half of them were rejected as drags. The distance
        // one is a latch that accumulates over every pointer move while a press is open, so jitter
        // in any single frame is enough to lose the click.
        //
        // Both are public fields, re-read every pass. Infinity disables each check, which leaves the
        // rule egui applies anyway and the one that was always wanted: a click is a press and a
        // release over the same widget.
        let context = egui::Context::default();
        context.options_mut(|options| {
            options.input_options.max_click_dist = f32::INFINITY;
            options.input_options.max_click_duration = f64::INFINITY;
        });

        Self {
            gpu: gpu.clone(),
            target,
            settled,
            size: (width, height),
            context,
            painter,
            pointer: None,
            pressed: false,
            was_pressed: false,
            scrolled: egui::Vec2::ZERO,
            typing: Vec::new(),
            copied: None,
        }
    }

    /// The image as bytes, so a caption can be looked at without a headset.
    #[cfg(test)]
    pub fn read_back(&self) -> Vec<u8> {
        const BYTES_PER_PIXEL: u32 = 4;
        let (width, height) = self.size;

        // wgpu wants each row of a copy aligned, so the buffer is usually wider than the image
        // and the padding is stripped after mapping.
        let aligned = width * BYTES_PER_PIXEL;
        let padded = aligned.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

        let buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("read back"),
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("read back"),
            });
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.gpu.queue.submit(std::iter::once(encoder.finish()));

        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.gpu
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the queue drains");

        let mapped = slice
            .get_mapped_range()
            .expect("the buffer was just mapped");
        let mut pixels = Vec::with_capacity((aligned * height) as usize);
        for row in 0..height {
            let start = (row * padded) as usize;
            pixels.extend_from_slice(&mapped[start..start + aligned as usize]);
        }
        drop(mapped);
        buffer.unmap();
        pixels
    }

    /// How big the image currently is, in pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Draws into an image of a different size from now on.
    ///
    /// A texture cannot change size, so this makes a new one and lets the old go. That is why it
    /// is called on a mode change rather than every frame.
    ///
    /// The mini panel needs this rather than being the big one shrunk. An overlay is scaled from
    /// its image to its width in metres, so drawing four lines into a full-size image and hanging
    /// it at a third of a metre gives text a third of the size — which is the one thing a surface
    /// meant to be read at a glance cannot be.
    pub fn resize(&mut self, width: u32, height: u32) {
        if self.size == (width, height) {
            return;
        }

        self.target = make_target(&self.gpu.device, width, height);
        self.size = (width, height);
    }

    /// Where the Commander is pointing, in the image's own pixels, or `None` when the ray is not
    /// on the panel at all.
    ///
    /// Pixels in and points out. The caller has an overlay measured in pixels and no reason to
    /// know that egui lays out in something smaller, and [`SCALE`] is the conversion — it lives
    /// here, so there is one place that knows it rather than two that have to agree.
    pub fn point_at(&mut self, at: Option<(f32, f32)>) {
        self.pointer = at.map(|(x, y)| egui::pos2(x / SCALE, y / SCALE));
    }

    /// Whether the trigger is held.
    ///
    /// Recorded rather than delivered. The edge is what egui is told about, and [`Self::draw`]
    /// works out where the edge is — because a press and a release that both happened between two
    /// frames still have to arrive as two events in the right order.
    pub fn press(&mut self, down: bool) {
        self.pressed = down;
    }

    /// Takes what SteamVR's keyboard reported, and puts it in whatever has focus.
    ///
    /// Characters rather than key presses, because that is what the keyboard sends and what a
    /// text field wants. The two exceptions are the two that are not characters at all: a
    /// backspace arrives as a control byte and has to become the key, or it would go into the box
    /// as an invisible character and the Commander could never correct a mistake.
    pub fn typed(&mut self, text: &str) {
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        };

        for character in text.chars() {
            match character {
                // Backspace, and delete, which some layouts send instead.
                '\u{8}' | '\u{7f}' => self.typing.push(key(egui::Key::Backspace)),
                '\r' | '\n' => self.typing.push(key(egui::Key::Enter)),
                // Escape, which is how somebody gets out of a text box that will not let go of
                // them. The toolkit surrenders focus on it, and focus is what decides whether Ward
                // is holding the Commander's keyboard — so this is the way out of that as well.
                '\u{1b}' => self.typing.push(key(egui::Key::Escape)),
                // Everything else that is not a control code. A stray one typed into a folder
                // path is a setting that will never resolve, with nothing on screen to say why.
                other if !other.is_control() => {
                    self.typing.push(egui::Event::Text(other.to_string()));
                }
                _ => {}
            }
        }
    }

    /// Hands the toolkit one of the clipboard shortcuts.
    ///
    /// A cut or a copy is a question rather than an instruction: egui answers it by putting the
    /// selected text into its output, which [`Self::copied`] hands back for whoever owns the real
    /// clipboard to write. A paste already carries the text, because reading the clipboard is the
    /// caller's job and not the toolkit's.
    pub fn clipboard(&mut self, what: &crate::keys::Typed) {
        // Text goes through `typed` rather than becoming an event here. Writing it out again was
        // the same three lines with the interesting part left off: a backspace arrives as a
        // control byte and has to become the key, and the rest of the control codes have to be
        // dropped. Handed to the toolkit as text, a backspace puts an invisible character into the
        // box instead of taking one out of it.
        let event = match what {
            crate::keys::Typed::Text(text) => return self.typed(text),
            crate::keys::Typed::Copy => egui::Event::Copy,
            crate::keys::Typed::Cut => egui::Event::Cut,
            crate::keys::Typed::Paste(text) => egui::Event::Paste(text.clone()),
        };

        self.typing.push(event);
    }

    /// Text the toolkit wants put on the clipboard, if a cut or a copy asked for any.
    #[must_use]
    pub fn copied(&mut self) -> Option<String> {
        self.copied.take()
    }

    /// Whether anything on the panel is waiting to be typed into.
    ///
    /// Asked of the toolkit rather than tracked here, so a box that takes focus because something
    /// else asked for it — the compose field re-focusing itself after a question — opens the
    /// keyboard the same way a box the Commander pointed at does.
    #[must_use]
    pub fn wants_typing(&self) -> bool {
        self.context.egui_wants_keyboard_input()
    }

    /// Adds a wheel's worth of scrolling, to be delivered on the next frame.
    ///
    /// Accumulated rather than replaced. Several scroll events routinely arrive between two of
    /// this panel's frames, and keeping only the last one would throw away most of a flick.
    pub fn scroll(&mut self, x: f32, y: f32) {
        self.scrolled += egui::vec2(x, y);
    }

    /// Runs one frame of egui, draws the result into the image, and hands back whatever the
    /// widget tree decided.
    ///
    /// The panel's tree reports what the Commander asked for as its return value, and there is
    /// nowhere else for that to come out: egui hands back the shapes it drew, not what the closure
    /// thought.
    pub fn draw_with<T>(&mut self, mut build: impl FnMut(&mut egui::Ui) -> T) -> T {
        let mut decided = None;
        self.draw(|ui| decided = Some(build(ui)));

        // egui runs the closure exactly once per pass. If that ever stops being true this is a
        // panic rather than a wrong answer, which is the right way round for a frame that was
        // never drawn.
        decided.expect("egui runs the widget tree once per frame")
    }

    /// Runs one frame of egui and draws the result into the image.
    pub fn draw(&mut self, build: impl FnMut(&mut egui::Ui)) {
        let screen = ScreenDescriptor {
            size_in_pixels: [self.size.0, self.size.1],
            pixels_per_point: SCALE,
        };

        let mut events = Vec::new();
        match self.pointer {
            Some(position) => {
                events.push(egui::Event::PointerMoved(position));
                if self.pressed != self.was_pressed {
                    events.push(egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed: self.pressed,
                        modifiers: egui::Modifiers::default(),
                    });
                }
                // Only once the edge has actually been delivered. Updating it unconditionally
                // consumed any press or release that happened while the ray was off the panel,
                // and egui does not treat `PointerGone` as a release: it keeps the button down on
                // purpose, so that dragging a slider survives leaving the viewport. A release
                // swallowed off-panel therefore left egui believing the trigger was still held,
                // with a stuck widget and every motion reading as a drag until a fresh press and
                // release both landed on the panel.
                self.was_pressed = self.pressed;
            }
            None => events.push(egui::Event::PointerGone),
        }

        // Taken rather than read, so a wheel's turn is delivered once. Left in place it would be
        // reapplied every frame and the panel would scroll forever from one flick.
        // Drained for the same reason the scrolling below is: delivered once, rather than
        // retyped every frame for as long as nothing else arrives.
        events.append(&mut self.typing);

        let scrolled = std::mem::replace(&mut self.scrolled, egui::Vec2::ZERO);
        if scrolled != egui::Vec2::ZERO {
            events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: scrolled,
                modifiers: egui::Modifiers::default(),
                // Discrete wheel turns rather than a touchpad's stream, so there is no gesture
                // with a beginning and an end for egui to track. `Move` is what egui documents
                // for a phase that is not known.
                phase: egui::TouchPhase::Move,
            });
        }

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "an overlay wider than 16 million pixels is not a case"
                )]
                egui::vec2(self.size.0 as f32 / SCALE, self.size.1 as f32 / SCALE),
            )),
            events,
            ..Default::default()
        };

        let mut output = self.context.run_ui(input, build);
        let jobs = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);

        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.painter
                    .update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
            }
        }

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("overlay frame"),
            });
        let staged = self.painter.update_buffers(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &jobs,
            &screen,
        );

        {
            let view = self.target.create_view(&wgpu::TextureViewDescriptor {
                label: Some("overlay target"),
                ..Default::default()
            });
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        // Transparent, so the cockpit shows through everywhere the panel does
                        // not paint.
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            self.painter
                .render(&mut pass.forget_lifetime(), &jobs, &screen);
        }

        // Leave the image in the layout OpenVR documents for a submitted texture.
        //
        // Valve's Vulkan notes say a texture handed to the compositor should be in
        // `VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL`, and that the runtime hands it back in the same
        // one. A render pass ends in `COLOR_ATTACHMENT_OPTIMAL`, and wgpu inserts a transition
        // only ahead of a next use it knows about, which the compositor is not. Submitting in the
        // wrong layout makes the image's contents undefined by the Vulkan spec, and this NVIDIA
        // driver tolerates it, which is the least useful way for a driver to behave.
        //
        // One texel into a buffer nothing reads is what asks wgpu for the transition through its
        // own tracker, so the tracker still agrees with the real layout next frame. A barrier
        // poked in through `wgpu-hal` would leave it believing the render pass's layout instead.
        // It also puts the app on the same layout sequence as `read_back`, which tier 1 uses: the
        // tests were exercising a friendlier path than the running app ever took.
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.settled,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: None,
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );

        self.gpu
            .queue
            .submit(staged.into_iter().chain(std::iter::once(encoder.finish())));

        // What the toolkit wants on the clipboard. It is an answer to the cut or copy handed in
        // above, and it is the only way out: egui has no clipboard of its own and never will.
        for command in output.platform_output.commands.drain(..) {
            let egui::OutputCommand::CopyText(text) = command else {
                continue;
            };
            self.copied = Some(text);
        }

        for id in &output.textures_delta.free {
            self.painter.free_texture(id);
        }

        // epaint asserts on drop if a delta went unapplied, which is a real guard rather than
        // bookkeeping: a missed one is a font atlas that never uploads and text that renders as
        // nothing. Everything above is applied, so saying so is the honest way to satisfy it.
        output.textures_delta.clear();
    }

    /// Every raw Vulkan handle OpenVR needs to read this image, or `None` if wgpu is not on
    /// Vulkan after all.
    ///
    /// Reaching through `as_hal` is the whole reason the backend is pinned to Vulkan. The
    /// compositor reads the image out of our device rather than being handed a copy, which is
    /// what makes the handoff cheap and what makes every handle here have to be exactly right.
    #[must_use]
    pub fn vulkan_image(&self) -> Option<crate::vr::VulkanImage> {
        use wgpu::hal::api::Vulkan;

        // SAFETY: nothing here destroys a resource or outlives the guards. Every handle is
        // copied out as an integer while the guard is alive, and the wgpu objects that own them
        // live as long as this Renderer.
        unsafe {
            let texture = self.target.as_hal::<Vulkan>()?;
            let device = self.gpu.device.as_hal::<Vulkan>()?;
            let queue = self.gpu.queue.as_hal::<Vulkan>()?;

            Some(crate::vr::VulkanImage {
                image: ash::vk::Handle::as_raw(texture.raw_handle()),
                device: ash::vk::Handle::as_raw(device.raw_device().handle()),
                physical_device: ash::vk::Handle::as_raw(device.raw_physical_device()),
                instance: ash::vk::Handle::as_raw(device.shared_instance().raw_instance().handle()),
                queue: ash::vk::Handle::as_raw(queue.as_raw()),
                queue_family_index: device.queue_family_index(),
                width: self.size.0,
                height: self.size.1,
                format: VULKAN_FORMAT.as_raw().cast_unsigned(),
            })
        }
    }
}

/// Picks the discrete card over anything integrated.
///
/// This machine reports two Vulkan devices, the 5080 and the Ultra 9's built-in graphics. Taking
/// whichever turns up first reads as terrible performance rather than as an error, and nothing
/// anywhere would say which one was chosen.
fn pick_adapter(adapters: Vec<wgpu::Adapter>) -> Option<wgpu::Adapter> {
    adapters
        .into_iter()
        .max_by_key(|adapter| match adapter.get_info().device_type {
            wgpu::DeviceType::DiscreteGpu => 3,
            wgpu::DeviceType::IntegratedGpu => 2,
            wgpu::DeviceType::VirtualGpu => 1,
            _ => 0,
        })
}

/// The image everything is drawn into.
///
/// `COPY_SRC` is for reading it back in tests, and `TEXTURE_BINDING` is what OpenVR wants of a
/// texture it is going to sample. `RENDER_ATTACHMENT` is what makes it drawable at all.
fn make_target(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("overlay image"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}
