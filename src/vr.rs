//! A safe-ish hold on OpenVR: start it once, get the interfaces, own one overlay.
//!
//! Everything below `openvr_sys` is raw pointers into a function-pointer table, and the job of
//! this module is to be the only place in Ward that touches them.
//!
//! Elite renders VR through OpenVR natively and never calls OpenXR, so this is the only door
//! into the headset that will actually open. An OpenXR layer would sit there and never fire.
//!
//! What is here is what captions and the panel need: start once, own the overlays, put a picture
//! in each and place it — in front of the Commander for a caption, out in the room for a panel
//! that can be pointed at.
//!
//! # Pointing is mouse input, and that is not a compromise
//!
//! SteamVR turns a controller ray into overlay mouse events, so an interactive overlay is handed
//! a cursor position and a button. That is exactly what egui wants, which is what makes one
//! widget tree able to serve a window and a headset without either knowing about the other. The
//! only translation is the Y axis, which OpenVR counts from the bottom and every toolkit counts
//! from the top.
//!
//! # Once, and only once
//!
//! `VR_Init` is called once and repeated calls leak. Nothing in OpenVR refuses a second call, so
//! the refusal has to live here. A process-wide flag does it, which is the right scope because
//! the leak is process-wide.

use std::ffi::{CStr, CString, c_void};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use glam::Affine3A;
use openvr_sys as sys;

/// Set for as long as a `Vr` exists anywhere in this process.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// What can go wrong.
#[derive(Debug)]
pub enum Error {
    /// A `Vr` already exists in this process.
    AlreadyRunning,
    /// Another process already owns this overlay key.
    AlreadyRunningElsewhere,
    /// OpenVR refused to start. Usually SteamVR is not running, or there is no headset and the
    /// null driver is disabled.
    Init(sys::EVRInitError),
    /// Started, but an interface we need is missing. A version mismatch between the vendored
    /// header and the installed runtime looks like this.
    MissingInterface(&'static str),
    /// An overlay call failed.
    Overlay(&'static str, sys::EVROverlayError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => write!(
                f,
                "OpenVR is already running in this process. It is started once; repeated \
                 VR_Init calls leak."
            ),
            Self::AlreadyRunningElsewhere => write!(
                f,
                "another copy of Ward is already running and owns the overlay. Close it. \
                 If nothing is visibly running, a previous copy did not shut down cleanly, and \
                 `Get-Process ward | Stop-Process -Force` will clear it."
            ),
            Self::Init(error) => {
                write!(f, "OpenVR would not start: {}", init_error_text(*error))
            }
            Self::MissingInterface(name) => write!(
                f,
                "OpenVR started but has no {name}. The vendored header and the installed \
                 runtime may disagree about versions."
            ),
            Self::Overlay(what, code) => write!(f, "{what} failed with overlay error {code}"),
        }
    }
}

impl std::error::Error for Error {}

/// A running OpenVR session. Dropping it shuts OpenVR down and frees the process to start again.
pub struct Vr {
    overlay: &'static sys::VR_IVROverlay_FnTable,
    system: &'static sys::VR_IVRSystem_FnTable,
}

/// Something that happened on an overlay, already in the toolkit's terms.
///
/// Translated here rather than passed on raw, so nothing above this file has to know that OpenVR
/// counts Y from the bottom or spells a button as an integer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// Where the ray is pointing, in the overlay's own pixels, measured from the top left.
    Moved { x: f32, y: f32 },
    /// The trigger, down or up.
    Button { down: bool },
    /// A scroll wheel's worth of turn.
    Scrolled { x: f32, y: f32 },
    /// The ray left the overlay entirely, so there is nothing being pointed at.
    Left,
}

/// Where a controller is, and what is held down on it.
#[derive(Clone, Copy, Debug)]
pub struct Controller {
    /// Where it is in the room, in metres.
    pub at: glam::Vec3,
    /// How fast it is moving, in metres per second. What a flick is made of.
    pub speed: glam::Vec3,
    /// Whether the grip is squeezed, which is the only button read here.
    ///
    /// The trigger is deliberately absent. SteamVR already delivers it as an overlay mouse click,
    /// and reading it a second time here would be one press understood as two things.
    pub grip: bool,
}

impl Vr {
    /// Starts OpenVR as an overlay application.
    ///
    /// # Errors
    ///
    /// Refuses if one is already running in this process. Otherwise fails if SteamVR will not
    /// start a session, or if it starts one without the interfaces we need.
    pub fn start() -> Result<Self, Error> {
        // Claim the slot before doing anything, so two threads racing cannot both get in.
        if RUNNING.swap(true, Ordering::SeqCst) {
            return Err(Error::AlreadyRunning);
        }

        let mut error = sys::EVRInitError_VRInitError_None;
        unsafe {
            sys::VR_InitInternal(
                &raw mut error,
                sys::EVRApplicationType_VRApplication_Overlay,
            );
        }
        if error != sys::EVRInitError_VRInitError_None {
            RUNNING.store(false, Ordering::SeqCst);
            return Err(Error::Init(error));
        }

        // Asked for and then dropped. Nothing here calls it, but a runtime that
        // cannot hand it over is a runtime a headset is not going to appear on,
        // and finding that out at startup beats finding it out as an overlay
        // that silently never draws.
        let system = unsafe { interface::<sys::VR_IVRSystem_FnTable>(sys::IVRSystem_Version) };
        let overlay = unsafe { interface::<sys::VR_IVROverlay_FnTable>(sys::IVROverlay_Version) };
        let compositor =
            unsafe { interface::<sys::VR_IVRCompositor_FnTable>(sys::IVRCompositor_Version) };
        // The compositor is checked for and then dropped. Nothing calls it: the one method that
        // did asked what Vulkan device extensions it wants, and nothing could inject them. Its
        // absence still means no compositing, so this is the honest place to find that out.
        let (system, overlay) = match (system, overlay, compositor) {
            (Some(system), Some(overlay), Some(_)) => (system, overlay),
            (None, ..) => {
                shutdown();
                return Err(Error::MissingInterface("system interface"));
            }
            (_, None, _) => {
                shutdown();
                return Err(Error::MissingInterface("overlay interface"));
            }
            (_, _, None) => {
                shutdown();
                return Err(Error::MissingInterface("compositor interface"));
            }
        };

        Ok(Self { overlay, system })
    }

    /// Where the headset is in the room, or `None` if it has not been found yet.
    ///
    /// This is what a summoned panel is placed against: it appears where the Commander is
    /// looking at the moment they ask for it, and then stays there while they look away. A panel
    /// that followed the head would be one you cannot look away from, which is the opposite of
    /// what a panel is for and the reason captions are the only thing that does follow.
    #[must_use]
    pub fn head(&self) -> Option<Affine3A> {
        self.poses().first().copied().flatten()
    }

    /// Every controller SteamVR currently knows about.
    ///
    /// Returned as a list rather than as a left and a right, because which hand a controller is
    /// in is a question this does not need to ask: a grab is a grab from either.
    #[must_use]
    pub fn controllers(&self) -> Vec<Controller> {
        let poses = self.poses_raw();
        let mut found = Vec::new();

        let Some(class_of) = self.system.GetTrackedDeviceClass else {
            return found;
        };
        let Some(state_of) = self.system.GetControllerState else {
            return found;
        };

        for (index, pose) in poses.iter().enumerate() {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the array is 64 long and indexed by a u32 everywhere in OpenVR"
            )]
            let index = index as u32;

            if !pose.bPoseIsValid
                || unsafe { class_of(index) }
                    != sys::ETrackedDeviceClass_TrackedDeviceClass_Controller
            {
                continue;
            }

            let mut state = sys::VRControllerState_t::default();
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the struct is far smaller than a u32 can count"
            )]
            let size = std::mem::size_of::<sys::VRControllerState_t>() as u32;

            if !unsafe { state_of(index, &raw mut state, size) } {
                continue;
            }

            let held = |button: sys::EVRButtonId| state.ulButtonPressed & (1u64 << button) != 0;
            let placed = from_openvr(pose.mDeviceToAbsoluteTracking);
            let velocity = pose.vVelocity.v;

            found.push(Controller {
                at: placed.translation.into(),
                speed: glam::Vec3::new(velocity[0], velocity[1], velocity[2]),
                grip: held(sys::EVRButtonId_k_EButton_Grip),
            });
        }

        found
    }

    /// Every device's pose, as a transform, with the invalid ones blanked out.
    fn poses(&self) -> Vec<Option<Affine3A>> {
        self.poses_raw()
            .iter()
            .map(|pose| {
                pose.bPoseIsValid
                    .then(|| from_openvr(pose.mDeviceToAbsoluteTracking))
            })
            .collect()
    }

    fn poses_raw(&self) -> Vec<sys::TrackedDevicePose_t> {
        let mut poses = vec![sys::TrackedDevicePose_t::default(); DEVICES as usize];

        if let Some(get) = self.system.GetDeviceToAbsoluteTrackingPose {
            unsafe {
                // No prediction. A panel is furniture rather than a thing being aimed, and asking
                // for where a device will be by the time the photons land buys nothing but jitter
                // when the answer is used to decide whether somebody grabbed something.
                get(STANDING, 0.0, poses.as_mut_ptr(), DEVICES);
            }
        }

        poses
    }

    /// Creates an overlay and shows it.
    ///
    /// # Errors
    ///
    /// Fails if OpenVR will not create or show the overlay.
    pub fn create_overlay(&self, key: &str, name: &str) -> Result<Overlay<'_>, Error> {
        let key = CString::new(key).unwrap_or_default();
        let name = CString::new(name).unwrap_or_default();
        let mut handle: sys::VROverlayHandle_t = 0;

        let create = self
            .overlay
            .CreateOverlay
            .ok_or(Error::MissingInterface("CreateOverlay"))?;
        // Valve's header types these as *mut even though CreateOverlay only reads them, so
        // the const has to be cast away. OpenVR copies both strings and never writes through
        // the pointers.
        // KeyInUse is not a generic failure. It means a second copy of this app is holding the
        // key, which is worth saying rather than reporting as overlay error 17.
        let code = unsafe {
            create(
                key.as_ptr().cast_mut(),
                name.as_ptr().cast_mut(),
                &raw mut handle,
            )
        };
        if code == sys::EVROverlayError_VROverlayError_KeyInUse {
            return Err(Error::AlreadyRunningElsewhere);
        }
        check("CreateOverlay", code)?;

        let show = self
            .overlay
            .ShowOverlay
            .ok_or(Error::MissingInterface("ShowOverlay"))?;
        check("ShowOverlay", unsafe { show(handle) })?;

        Ok(Overlay {
            table: self.overlay,
            handle,
        })
    }
}

impl Drop for Vr {
    fn drop(&mut self) {
        shutdown();
    }
}

/// One overlay, borrowed from the session that made it.
pub struct Overlay<'vr> {
    table: &'vr sys::VR_IVROverlay_FnTable,
    handle: sys::VROverlayHandle_t,
}

impl Overlay<'_> {
    /// Hands the compositor a Vulkan image to display.
    ///
    /// The image is not copied. OpenVR reads it out of our device, which is what makes this the
    /// cheap path and also what makes the handles below have to be exactly right.
    ///
    /// # Errors
    ///
    /// Fails if the compositor rejects the image.
    ///
    /// # Safety
    ///
    /// Every handle in `image` must be live and belong to the same Vulkan device, and the image
    /// must stay alive until the compositor is finished with it.
    pub unsafe fn set_texture(&self, image: &VulkanImage) -> Result<(), Error> {
        let mut data = sys::VRVulkanTextureData_t {
            m_nImage: image.image,
            m_pDevice: image.device as *mut sys::VkDevice_T,
            m_pPhysicalDevice: image.physical_device as *mut sys::VkPhysicalDevice_T,
            m_pInstance: image.instance as *mut sys::VkInstance_T,
            m_pQueue: image.queue as *mut sys::VkQueue_T,
            m_nQueueFamilyIndex: image.queue_family_index,
            m_nWidth: image.width,
            m_nHeight: image.height,
            m_nFormat: image.format,
            m_nSampleCount: 1,
        };

        let mut texture = sys::Texture_t {
            handle: (&raw mut data).cast::<c_void>(),
            eType: sys::ETextureType_TextureType_Vulkan,
            eColorSpace: sys::EColorSpace_ColorSpace_Auto,
        };

        let set = self
            .table
            .SetOverlayTexture
            .ok_or(Error::MissingInterface("SetOverlayTexture"))?;
        check("SetOverlayTexture", unsafe {
            set(self.handle, &raw mut texture)
        })
    }

    /// Sets how wide the overlay is, in metres. Height follows from the image's aspect ratio.
    ///
    /// # Errors
    ///
    /// Fails if the call is refused.
    pub fn set_width(&self, meters: f32) -> Result<(), Error> {
        let set = self
            .table
            .SetOverlayWidthInMeters
            .ok_or(Error::MissingInterface("SetOverlayWidthInMeters"))?;
        check("SetOverlayWidthInMeters", unsafe {
            set(self.handle, meters)
        })
    }

    /// Pins the overlay in front of the Commander, wherever they are looking.
    ///
    /// Relative to the headset rather than placed in the room, and that is the
    /// whole design of a caption layer: it is not furniture you turn to look at,
    /// it is text that appears where you are already looking and then goes away.
    /// A world-locked caption is one you can turn away from mid-sentence.
    ///
    /// # Errors
    ///
    /// Fails if the call is refused.
    pub fn follow_head(&self, pose: Affine3A) -> Result<(), Error> {
        let set =
            self.table
                .SetOverlayTransformTrackedDeviceRelative
                .ok_or(Error::MissingInterface(
                    "SetOverlayTransformTrackedDeviceRelative",
                ))?;
        let mut matrix = to_openvr(pose);
        check("SetOverlayTransformTrackedDeviceRelative", unsafe {
            set(self.handle, HMD, &raw mut matrix)
        })
    }

    /// Places the overlay in the room, rather than on the Commander.
    ///
    /// The other half of [`Self::follow_head`], and the difference between a caption and a panel.
    /// A caption appears where you are already looking and goes away; a panel is put somewhere and
    /// stays there, so you can look at the station and then look back at it.
    ///
    /// # Errors
    ///
    /// Fails if the call is refused.
    pub fn place(&self, pose: Affine3A) -> Result<(), Error> {
        let set = self
            .table
            .SetOverlayTransformAbsolute
            .ok_or(Error::MissingInterface("SetOverlayTransformAbsolute"))?;
        let mut matrix = to_openvr(pose);
        check("SetOverlayTransformAbsolute", unsafe {
            set(self.handle, STANDING, &raw mut matrix)
        })
    }

    /// Lets the Commander point at this overlay and click it.
    ///
    /// `width` and `height` are the pixel size of the image being drawn, and they are what the
    /// mouse scale is set to — so the positions that come back out of [`Self::events`] are in the
    /// same pixels the picture was drawn in, and nothing downstream has to convert between two
    /// coordinate systems it did not choose.
    ///
    /// # Errors
    ///
    /// Fails if any of the three calls is refused.
    pub fn take_input(&self, width: u32, height: u32) -> Result<(), Error> {
        let method = self
            .table
            .SetOverlayInputMethod
            .ok_or(Error::MissingInterface("SetOverlayInputMethod"))?;
        check("SetOverlayInputMethod", unsafe {
            method(self.handle, sys::VROverlayInputMethod_Mouse)
        })?;

        let scale = self
            .table
            .SetOverlayMouseScale
            .ok_or(Error::MissingInterface("SetOverlayMouseScale"))?;
        #[expect(
            clippy::cast_precision_loss,
            reason = "an overlay wider than 16 million pixels is not a case"
        )]
        let mut size = sys::HmdVector2_t {
            v: [width as f32, height as f32],
        };
        check("SetOverlayMouseScale", unsafe {
            scale(self.handle, &raw mut size)
        })?;

        let flag = self
            .table
            .SetOverlayFlag
            .ok_or(Error::MissingInterface("SetOverlayFlag"))?;

        // Without this the panel is a picture. SteamVR only sends mouse events to an overlay it
        // has been told is worth pointing at, and the failure is silent: the ray goes through it
        // and nothing ever arrives.
        check("SetOverlayFlag", unsafe {
            flag(
                self.handle,
                sys::VROverlayFlags_MakeOverlaysInteractiveIfVisible,
                true,
            )
        })?;

        // A settings page and a conversation both scroll, and without this the wheel does nothing
        // at all in the headset while working perfectly in the window — which is exactly the kind
        // of quiet divergence between the two surfaces that D4 exists to prevent.
        check("SetOverlayFlag", unsafe {
            flag(
                self.handle,
                sys::VROverlayFlags_SendVRDiscreteScrollEvents,
                true,
            )
        })
    }

    /// Everything that has happened on this overlay since the last time it was asked.
    ///
    /// Drained rather than sampled. SteamVR queues these, and reading one per frame at the panel's
    /// redraw rate would put the pointer behind the ray by however many events were skipped —
    /// which reads as a cursor that lags and then catches up in a jump.
    #[must_use]
    pub fn events(&self, height: u32) -> Vec<Event> {
        let mut out = Vec::new();

        let Some(poll) = self.table.PollNextOverlayEvent else {
            return out;
        };

        #[expect(
            clippy::cast_possible_truncation,
            reason = "the event struct is far smaller than a u32 can count"
        )]
        let size = std::mem::size_of::<sys::VREvent_t>() as u32;

        #[expect(
            clippy::cast_precision_loss,
            reason = "an overlay taller than 16 million pixels is not a case"
        )]
        let tall = height as f32;

        // OpenVR declares the event kinds as signed and reports them as unsigned in the same
        // struct. Narrowed here, once, rather than at each of the five comparisons below.
        const MOVED: u32 = sys::EVREventType_VREvent_MouseMove.cast_unsigned();
        const DOWN: u32 = sys::EVREventType_VREvent_MouseButtonDown.cast_unsigned();
        const UP: u32 = sys::EVREventType_VREvent_MouseButtonUp.cast_unsigned();
        const DISCRETE: u32 = sys::EVREventType_VREvent_ScrollDiscrete.cast_unsigned();
        const SMOOTH: u32 = sys::EVREventType_VREvent_ScrollSmooth.cast_unsigned();
        const LEFT: u32 = sys::EVREventType_VREvent_FocusLeave.cast_unsigned();
        const TRIGGER: u32 = sys::EVRMouseButton_VRMouseButton_Left.cast_unsigned();

        let mut event = sys::VREvent_t::default();

        while unsafe { poll(self.handle, &raw mut event, size) } {
            // SAFETY: the union is read according to the event type that named it, which is the
            // only thing that says which arm is live.
            let translated = match event.eventType {
                MOVED => {
                    let mouse = unsafe { event.data.mouse };
                    // OpenVR counts Y from the bottom of the overlay and every toolkit counts it
                    // from the top. Left unflipped, the panel works perfectly upside down: the
                    // pointer is on the settings tab while the ray is on the compose box.
                    Some(Event::Moved {
                        x: mouse.x,
                        y: tall - mouse.y,
                    })
                }
                DOWN | UP => {
                    let mouse = unsafe { event.data.mouse };

                    // Only the trigger. A controller reports its other buttons through the same
                    // event, and treating them all as a click means the grip that grabs the panel
                    // also presses whatever the ray happened to be over.
                    (mouse.button == TRIGGER).then_some(Event::Button {
                        down: event.eventType == DOWN,
                    })
                }
                DISCRETE | SMOOTH => {
                    let scroll = unsafe { event.data.scroll };
                    Some(Event::Scrolled {
                        x: scroll.xdelta,
                        y: scroll.ydelta,
                    })
                }
                LEFT => Some(Event::Left),
                _ => None,
            };

            out.extend(translated);
        }

        out
    }

    /// Puts the overlay on screen.
    ///
    /// # Errors
    ///
    /// Fails if the call is refused.
    pub fn show(&self) -> Result<(), Error> {
        let show = self
            .table
            .ShowOverlay
            .ok_or(Error::MissingInterface("ShowOverlay"))?;
        check("ShowOverlay", unsafe { show(self.handle) })
    }

    /// Takes it off screen.
    ///
    /// Captions are ephemeral, and this is the half of that which matters: when
    /// there is nothing to say there is nothing over the cockpit at all.
    ///
    /// # Errors
    ///
    /// Fails if the call is refused.
    pub fn hide(&self) -> Result<(), Error> {
        let hide = self
            .table
            .HideOverlay
            .ok_or(Error::MissingInterface("HideOverlay"))?;
        check("HideOverlay", unsafe { hide(self.handle) })
    }
}

impl Drop for Overlay<'_> {
    /// Lets go of the image before destroying the overlay.
    ///
    /// The order matters and it is not bookkeeping. `set_texture` hands the compositor a Vulkan
    /// image it goes on reading out of our device, so an overlay outliving the device that owns
    /// its image leaves SteamVR querying a destroyed physical device. That surfaces as a Vulkan
    /// loader complaint and then as a stack corruption inside the runtime, a long way from the
    /// line that caused it.
    fn drop(&mut self) {
        if let Some(clear) = self.table.ClearOverlayTexture {
            unsafe { clear(self.handle) };
        }
        if let Some(destroy) = self.table.DestroyOverlay {
            unsafe { destroy(self.handle) };
        }
    }
}

/// Every handle OpenVR needs to read one of our images.
///
/// All of these come out of `wgpu-hal`. They are plain integers here so nothing in this module
/// has to name a Vulkan type, which keeps ash out of the signature.
#[derive(Clone, Copy, Debug)]
pub struct VulkanImage {
    pub image: u64,
    pub device: u64,
    pub physical_device: u64,
    pub instance: u64,
    pub queue: u64,
    pub queue_family_index: u32,
    pub width: u32,
    pub height: u32,
    /// A raw `VkFormat`.
    pub format: u32,
}

/// Fetches a flat-API function table.
///
/// The flat API wants a `FnTable:` prefix on the version string. Without it OpenVR hands back a
/// C++ interface pointer, which has a different shape and would be read as garbage.
unsafe fn interface<T>(version: &[u8]) -> Option<&'static T> {
    let version = CStr::from_bytes_with_nul(version).ok()?;
    let name = CString::new(format!("FnTable:{}", version.to_string_lossy())).ok()?;
    let mut error = sys::EVRInitError_VRInitError_None;
    let pointer = unsafe { sys::VR_GetGenericInterface(name.as_ptr(), &raw mut error) };
    if pointer == 0 || error != sys::EVRInitError_VRInitError_None {
        return None;
    }
    Some(unsafe { &*(pointer as *const T) })
}

/// The headset itself, which is always device zero.
const HMD: u32 = sys::k_unTrackedDeviceIndex_Hmd;

/// How many devices OpenVR can track at once, which is the size of every pose array it fills.
const DEVICES: u32 = sys::k_unMaxTrackedDeviceCount;

/// Room scale, so a panel put down stays where it was put.
///
/// The seated origin moves when the Commander recenters, which would take the panel with it.
const STANDING: sys::ETrackingUniverseOrigin =
    sys::ETrackingUniverseOrigin_TrackingUniverseStanding;

fn shutdown() {
    unsafe { sys::VR_ShutdownInternal() };
    RUNNING.store(false, Ordering::SeqCst);
}

fn check(what: &'static str, code: sys::EVROverlayError) -> Result<(), Error> {
    if code == sys::EVROverlayError_VROverlayError_None {
        Ok(())
    } else {
        Err(Error::Overlay(what, code))
    }
}

fn init_error_text(error: sys::EVRInitError) -> String {
    unsafe {
        CStr::from_ptr(sys::VR_GetVRInitErrorAsEnglishDescription(error))
            .to_string_lossy()
            .into_owned()
    }
}

/// OpenVR to glam, the other way round from [`to_openvr`].
///
/// The two are next to each other because a transposition mistake in either is invisible until
/// something is in the wrong place in a headset, and having both in view is most of what stops
/// one of them being written the wrong way round.
fn from_openvr(matrix: sys::HmdMatrix34_t) -> Affine3A {
    let m = matrix.m;
    Affine3A::from_cols(
        glam::Vec3::new(m[0][0], m[1][0], m[2][0]).into(),
        glam::Vec3::new(m[0][1], m[1][1], m[2][1]).into(),
        glam::Vec3::new(m[0][2], m[1][2], m[2][2]).into(),
        glam::Vec3::new(m[0][3], m[1][3], m[2][3]).into(),
    )
}

/// glam to OpenVR. `HmdMatrix34_t` is row major, three rows of four.
fn to_openvr(pose: Affine3A) -> sys::HmdMatrix34_t {
    let columns = pose.matrix3;
    let translation = pose.translation;
    sys::HmdMatrix34_t {
        m: [
            [
                columns.x_axis.x,
                columns.y_axis.x,
                columns.z_axis.x,
                translation.x,
            ],
            [
                columns.x_axis.y,
                columns.y_axis.y,
                columns.z_axis.y,
                translation.y,
            ],
            [
                columns.x_axis.z,
                columns.y_axis.z,
                columns.z_axis.z,
                translation.z,
            ],
        ],
    }
}
