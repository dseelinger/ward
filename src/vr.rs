//! A safe-ish hold on OpenVR: start it once, get the interfaces, own one overlay.
//!
//! Everything below `openvr_sys` is raw pointers into a function-pointer table, and the job of
//! this module is to be the only place in Ward that touches them.
//!
//! Elite renders VR through OpenVR natively and never calls OpenXR, so this is the only door
//! into the headset that will actually open. An OpenXR layer would sit there and never fire.
//!
//! What is here is what captions need: start once, own one overlay, put a picture in it and
//! hold it in front of the Commander. Controllers, pointing and anything movable belong to the
//! panel, which is its own piece of work.
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
        let overlay = match (system, overlay, compositor) {
            (Some(_), Some(overlay), Some(_)) => overlay,
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

        Ok(Self { overlay })
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
