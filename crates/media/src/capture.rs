//! Capture devices for the Record view: cameras, microphones and screens.
//!
//! The UI never calls a platform API. A host (cpal, Media Foundation, AVFoundation, the browser)
//! implements one of these traits and the record view drives it. When a host is absent — the web
//! build, a headless test, a platform without a backend — the control reports
//! [`CaptureError::Unsupported`] instead of pretending a device works.
//!
//! Device lists come from the host, not from this crate; an empty list and a denied permission are
//! different answers and the view shows them differently. All calls are expected to return
//! promptly: a host that opens a device on a worker thread reports `ok` as soon as the request is
//! accepted, and [`CameraIn::failure`] / [`ScreenIn::failure`] report a start that failed later.

use std::fmt;

/// A camera or microphone the host can open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Stable for the life of the process; passed back to `open` / `select`.
    pub id: String,
    /// The name shown in the device menu.
    pub name: String,
}

impl DeviceInfo {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> DeviceInfo {
        DeviceInfo { id: id.into(), name: name.into() }
    }
}

/// A monitor the screen recorder can capture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenInfo {
    /// Stable for the life of the process.
    pub id: String,
    pub name: String,
    /// Physical pixels.
    pub width: u32,
    pub height: u32,
}

/// A rectangle on a monitor, in physical pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenRegion {
    /// The [`ScreenInfo::id`] of the monitor.
    pub screen: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl ScreenRegion {
    /// Clamp a region to a monitor's bounds and reject an empty one. Coordinates come from a
    /// pointer drag, so they can be outside the screen or inverted.
    pub fn fit(&self, screen: &ScreenInfo) -> Option<ScreenRegion> {
        let x = self.x.min(screen.width);
        let y = self.y.min(screen.height);
        let width = self.width.min(screen.width.saturating_sub(x));
        let height = self.height.min(screen.height.saturating_sub(y));
        (width > 0 && height > 0).then(|| ScreenRegion { screen: screen.id.clone(), x, y, width, height })
    }
}

/// Why a capture device could not be listed, opened or kept running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureError {
    /// This build or platform has no backend for the device kind.
    Unsupported,
    /// The OS reports no device of that kind.
    Unavailable,
    /// The user (or a policy) refused access to the device.
    Denied,
    /// Another application holds the device.
    Busy,
    /// The device went away while it was in use.
    Disconnected,
    /// Anything else the host can describe.
    Failed(String),
}

impl CaptureError {
    /// The sentence the Record view shows under a control. Names the device kind so a denied
    /// camera is not reported as a denied microphone.
    pub fn message(&self, kind: &str) -> String {
        match self {
            CaptureError::Unsupported => format!("{kind} capture isn't available on this platform."),
            CaptureError::Unavailable => format!("No {kind} was found."),
            CaptureError::Denied => format!("Access to the {kind} was denied. Allow it in system settings and try again."),
            CaptureError::Busy => format!("The {kind} is being used by another app."),
            CaptureError::Disconnected => format!("The {kind} was disconnected."),
            CaptureError::Failed(why) => format!("The {kind} couldn't be opened: {why}"),
        }
    }
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CaptureError::Unsupported => f.write_str("capture isn't supported"),
            CaptureError::Unavailable => f.write_str("no device"),
            CaptureError::Denied => f.write_str("permission denied"),
            CaptureError::Busy => f.write_str("device busy"),
            CaptureError::Disconnected => f.write_str("device disconnected"),
            CaptureError::Failed(why) => f.write_str(why),
        }
    }
}

/// One captured picture, RGBA8 straight alpha.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptureFrame {
    /// Seconds since the stream started.
    pub time: f64,
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

impl CaptureFrame {
    /// A frame with the size its pixel buffer implies, or `None` when they disagree (a host bug
    /// must not become a panic or an out-of-bounds read later).
    pub fn new(time: f64, width: u32, height: u32, rgba: Vec<u8>) -> Option<CaptureFrame> {
        let expected = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
        (time.is_finite() && rgba.len() == expected).then_some(CaptureFrame { time, width, height, rgba })
    }
}

/// A camera. The Record view owns the stream; a host may start it on a worker thread.
pub trait CameraIn {
    /// Cameras this host can open. `Unsupported` when there is no backend at all; `Denied` when
    /// the OS hides them behind a permission prompt the host cannot pass.
    fn enumerate(&mut self) -> Result<Vec<DeviceInfo>, CaptureError>;
    /// Open `id`, or the system default when `None`. Opening while running replaces the stream.
    fn open(&mut self, id: Option<&str>) -> Result<(), CaptureError>;
    fn close(&mut self);
    fn running(&self) -> bool;
    /// The newest picture since the last call. Must not block: a missing frame is `None` and the
    /// view keeps drawing the last texture.
    fn frame(&mut self) -> Option<CaptureFrame>;
    /// Why a started stream stopped (unplugged, permission revoked), reported once.
    fn failure(&mut self) -> Option<CaptureError> {
        None
    }
}

/// A screen or monitor capture source.
pub trait ScreenIn {
    /// Monitors this host can capture.
    fn screens(&mut self) -> Result<Vec<ScreenInfo>, CaptureError>;
    /// Start capturing `region`. Starting while running replaces the region.
    fn start(&mut self, region: &ScreenRegion) -> Result<(), CaptureError>;
    fn stop(&mut self);
    fn running(&self) -> bool;
    /// The newest picture since the last call. Must not block.
    fn frame(&mut self) -> Option<CaptureFrame>;
    /// Why a started stream stopped, reported once.
    fn failure(&mut self) -> Option<CaptureError> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> ScreenInfo {
        ScreenInfo { id: "s1".into(), name: "Built-in".into(), width: 1920, height: 1080 }
    }

    #[test]
    fn regions_are_clamped_to_the_screen_and_empty_ones_are_rejected() {
        let s = screen();
        let r = ScreenRegion { screen: "s1".into(), x: 1900, y: 1000, width: 500, height: 500 };
        assert_eq!(r.fit(&s), Some(ScreenRegion { screen: "s1".into(), x: 1900, y: 1000, width: 20, height: 80 }));
        let outside = ScreenRegion { screen: "s1".into(), x: 5000, y: 1080, width: 10, height: 10 };
        assert_eq!(outside.fit(&s), None, "a region entirely outside the screen has no pixels");
        let zero = ScreenRegion { screen: "s1".into(), x: 10, y: 10, width: 0, height: 40 };
        assert_eq!(zero.fit(&s), None);
    }

    #[test]
    fn a_region_is_rebound_to_the_chosen_screen() {
        let s = screen();
        let r = ScreenRegion { screen: "elsewhere".into(), x: 4, y: 4, width: 100, height: 100 };
        assert_eq!(r.fit(&s).map(|r| r.screen), Some("s1".to_string()));
    }

    #[test]
    fn frames_reject_a_buffer_that_does_not_match_the_size() {
        assert!(CaptureFrame::new(0.0, 2, 2, vec![0; 16]).is_some());
        assert!(CaptureFrame::new(0.0, 2, 2, vec![0; 15]).is_none());
        assert!(CaptureFrame::new(0.0, 0, 0, vec![]).is_some(), "an empty frame is well formed");
        assert!(CaptureFrame::new(f64::NAN, 1, 1, vec![0; 4]).is_none());
        assert!(CaptureFrame::new(0.0, u32::MAX, u32::MAX, vec![]).is_none(), "the size computation is checked");
    }

    #[test]
    fn messages_name_the_device_kind() {
        assert!(CaptureError::Denied.message("camera").contains("camera"));
        assert!(CaptureError::Busy.message("microphone").contains("microphone"));
        assert!(CaptureError::Unavailable.message("screen").contains("screen"));
        assert!(CaptureError::Failed("driver".into()).message("camera").contains("driver"));
        assert_eq!(CaptureError::Unsupported.to_string(), "capture isn't supported");
    }
}
