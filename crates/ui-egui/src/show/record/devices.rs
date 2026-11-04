//! Camera, microphone and screen-recording controls for the Record view.
//!
//! The controls are thin: they read and write [`DeviceState`] on [`Devices`] and call the hosts
//! that the desktop/web app injects through [`Services`]. No platform API is called here, and a
//! missing host reports [`DeviceStatus::Unavailable`] — the controls never claim a device is live
//! when nothing is capturing.
//!
//! The top bar calls [`Devices::camera_button`], [`Devices::mic_button`] and
//! [`Devices::screen_button`] from its right-to-left group. Each draws a split button: the label
//! toggles capture, the chevron opens the device menu. The screen button opens the region picker,
//! which maps a drag on a monitor diagram to physical pixels on that monitor.

use std::cell::RefCell;
use std::rc::Rc;

use egui::{Color32, CornerRadius, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui, Vec2, pos2, vec2};
use deckcraft_media::{CameraIn, CaptureError, CaptureFrame, DeviceInfo, MicIn, ScreenIn, ScreenInfo, ScreenRegion};

use super::{RecordView, theme};
use crate::Services;

const FG: Color32 = Color32::from_gray(226);
const DIM: Color32 = Color32::from_gray(150);
const FAINT: Color32 = Color32::from_gray(110);
const ACCENT: Color32 = Color32::from_rgb(96, 176, 238);
const LIVE: Color32 = Color32::from_rgb(224, 62, 62);
const WARN: Color32 = Color32::from_rgb(230, 168, 58);
const PANEL: Color32 = Color32::from_rgb(38, 38, 42);
const PANEL_BORDER: Color32 = Color32::from_gray(72);

/// The explicit state of one capture control. `Unavailable`, `Denied` and `Busy` are never
/// collapsed into "Off": what the presenter has to do about them differs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum DeviceStatus {
    /// No host, no device or no permission to list any. The control is disabled.
    #[default]
    Unavailable,
    Off,
    /// Opening was accepted; the first frame or a running stream has not landed yet.
    Starting,
    On,
    /// The OS refused access. Enabling the control is still allowed: the user can change the
    /// system setting and press it again.
    Denied,
    /// Another application holds the device. Pressing again retries.
    Busy,
}

/// One control's state. Camera, microphone and screen each own one.
pub(super) struct DeviceState {
    pub(super) status: DeviceStatus,
    /// `DeviceInfo::id` selected in the menu; `None` is the system default.
    pub(super) selected: Option<String>,
    /// Microphone peak level in `0..=1`, smoothed; zero for the other kinds.
    pub(super) level: f32,
    /// Why the control is not simply Off/On, in words the presenter can act on.
    pub(super) message: Option<String>,
    /// Cached device list; `listed` distinguishes "not asked yet" from "asked, found none".
    devices: Vec<DeviceInfo>,
    listed: bool,
    /// The last frame uploaded as a texture (camera and screen previews).
    texture: Option<TextureHandle>,
    /// Size of the last uploaded frame.
    frame_size: Option<(u32, u32)>,
    /// When opening was accepted, for the "did not start" timeout.
    started_at: Option<f64>,
}

impl Default for DeviceState {
    fn default() -> Self {
        DeviceState {
            status: DeviceStatus::Unavailable,
            selected: None,
            level: 0.0,
            message: None,
            devices: Vec::new(),
            listed: false,
            texture: None,
            frame_size: None,
            started_at: None,
        }
    }
}

impl DeviceState {
    pub(super) fn is_on(&self) -> bool {
        matches!(self.status, DeviceStatus::On | DeviceStatus::Starting)
    }

    /// Whether pressing the label should try to start capture.
    fn can_start(&self) -> bool {
        matches!(self.status, DeviceStatus::Off | DeviceStatus::Denied | DeviceStatus::Busy)
    }

    /// Capture stopped on its own (device gone, stream ended): Off with the reason, so the
    /// presenter can press the button to retry.
    fn stop_failed(&mut self, _kind: &str, why: &str) {
        self.status = DeviceStatus::Off;
        self.message = Some(why.to_string());
        self.started_at = None;
    }

    fn fail(&mut self, kind: &str, error: CaptureError) {
        let message = error.message(kind);
        self.status = match error {
            CaptureError::Unsupported | CaptureError::Unavailable => DeviceStatus::Unavailable,
            CaptureError::Denied => DeviceStatus::Denied,
            CaptureError::Busy => DeviceStatus::Busy,
            CaptureError::Disconnected => DeviceStatus::Off,
            CaptureError::Failed(_) => DeviceStatus::Off,
        };
        self.message = Some(message);
        self.started_at = None;
    }

    /// The one-line status shown in the menu and on hover.
    fn status_text(&self, kind: &str) -> String {
        match self.status {
            DeviceStatus::Unavailable => self.message.clone().unwrap_or_else(|| format!("No {kind} capture is available.")),
            DeviceStatus::Off => self.message.clone().unwrap_or_else(|| "Off".to_string()),
            DeviceStatus::Starting => self.message.clone().unwrap_or_else(|| "Starting…".to_string()),
            DeviceStatus::On => match self.frame_size {
                Some((w, h)) => format!("Live · {w}×{h}"),
                None => "Live".to_string(),
            },
            DeviceStatus::Denied => self.message.clone().unwrap_or_else(|| format!("Access to the {kind} was denied.")),
            DeviceStatus::Busy => self.message.clone().unwrap_or_else(|| format!("The {kind} is in use by another app.")),
        }
    }
}

/// The screen picker's in-progress selection: a monitor and a rectangle in its normalized
/// `0..=1` space.
pub(super) struct Picker {
    screen: usize,
    /// Normalized selection on [`screen`].
    sel: Option<Rect>,
    /// Normalized drag anchor while the pointer is down.
    drag_from: Option<egui::Pos2>,
    message: Option<String>,
}

impl Picker {
    fn new() -> Picker {
        Picker { screen: 0, sel: None, drag_from: None, message: None }
    }
}

/// Camera, microphone and screen-recording state for the Record view.
pub(super) struct Devices {
    pub(super) camera: DeviceState,
    pub(super) mic: DeviceState,
    pub(super) screen: DeviceState,
    /// Hosts are shared with [`Services`] so a host survives leaving and re-entering the view,
    /// and a fake in a test can be inspected after the control used it.
    camera_host: Option<Rc<RefCell<Box<dyn CameraIn>>>>,
    mic_host: Option<Rc<RefCell<Box<dyn MicIn>>>>,
    screen_host: Option<Rc<RefCell<Box<dyn ScreenIn>>>>,
    /// Monitors found when the picker opened.
    screens: Vec<ScreenInfo>,
    picker: Option<Picker>,
    /// Latest captured frames, uploaded once by [`Devices::tick`].
    camera_frame: Option<CaptureFrame>,
    screen_frame: Option<CaptureFrame>,
    last_tick: f64,
}

impl Default for Devices {
    fn default() -> Self {
        Devices {
            camera: DeviceState::default(),
            mic: DeviceState::default(),
            screen: DeviceState::default(),
            camera_host: None,
            mic_host: None,
            screen_host: None,
            screens: Vec::new(),
            picker: None,
            camera_frame: None,
            screen_frame: None,
            last_tick: f64::NEG_INFINITY,
        }
    }
}

impl Drop for Devices {
    fn drop(&mut self) {
        if let Some(host) = &self.camera_host
            && let Ok(mut host) = host.try_borrow_mut()
        {
            host.close();
        }
        if let Some(host) = &self.mic_host
            && let Ok(mut host) = host.try_borrow_mut()
        {
            host.stop();
        }
        if let Some(host) = &self.screen_host
            && let Ok(mut host) = host.try_borrow_mut()
        {
            host.stop();
        }
    }
}

impl Devices {
    /// Take the hosts the app injected. A no-op when the view already holds them, so the shell
    /// can call it every time the view is entered.
    pub(super) fn connect(&mut self, services: &Services) {
        if self.camera_host.is_none() {
            self.camera_host = services.camera.clone();
        }
        if self.mic_host.is_none() {
            self.mic_host = services.mic.clone();
        }
        if self.screen_host.is_none() {
            self.screen_host = services.screen.clone();
        }
        for (state, present, kind) in [
            (&mut self.camera, self.camera_host.is_some(), "camera"),
            (&mut self.mic, self.mic_host.is_some(), "microphone"),
            (&mut self.screen, self.screen_host.is_some(), "screen"),
        ] {
            if !present {
                state.status = DeviceStatus::Unavailable;
                state.message = Some(CaptureError::Unsupported.message(kind));
            } else if state.status == DeviceStatus::Unavailable && state.message.is_none() {
                state.status = DeviceStatus::Off;
            }
        }
    }

    /// Attach a camera host directly (tests and the web app). The host starts Off.
    pub(super) fn attach_camera(&mut self, host: Box<dyn CameraIn>) {
        self.camera_host = Some(Rc::new(RefCell::new(host)));
        self.camera.status = DeviceStatus::Off;
        self.camera.message = None;
    }

    pub(super) fn attach_mic(&mut self, host: Box<dyn MicIn>) {
        self.mic_host = Some(Rc::new(RefCell::new(host)));
        self.mic.status = DeviceStatus::Off;
        self.mic.message = None;
    }

    pub(super) fn attach_screen(&mut self, host: Box<dyn ScreenIn>) {
        self.screen_host = Some(Rc::new(RefCell::new(host)));
        self.screen.status = DeviceStatus::Off;
        self.screen.message = None;
    }

    /// The latest camera picture, when the preview is visible. The texture is uploaded by
    /// [`Devices::tick`] (called from the top-bar buttons) so a section can paint with `&RecordView`.
    pub(super) fn camera_texture(&self) -> Option<&TextureHandle> {
        self.camera.texture.as_ref()
    }

    /// Paint the camera bubble's picture into `rect`, or a muted placeholder when there is none.
    pub(super) fn paint_camera_preview(&self, ui: &Ui, rect: Rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(8), Color32::from_gray(24));
        if let Some(texture) = &self.camera.texture {
            painter.image(texture.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            return;
        }
        let text = match self.camera.status {
            DeviceStatus::Unavailable | DeviceStatus::Denied | DeviceStatus::Busy => "Camera unavailable",
            DeviceStatus::Starting => "Starting camera…",
            DeviceStatus::Off => "Camera off",
            DeviceStatus::On => "Waiting for camera…",
        };
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, theme::font(12.0), FAINT);
    }

    /// One pass of host polling and texture upload. The three buttons call it; only the first call
    /// with a given `now` does the work.
    fn tick(&mut self, ctx: &egui::Context, now: f64) {
        if !now.is_finite() || now == self.last_tick {
            return;
        }
        self.last_tick = now;
        self.poll(now);
        self.upload(ctx);
    }

    /// Drain host frames and failures. Kept free of egui so it is testable without a UI.
    fn poll(&mut self, now: f64) {
        self.poll_camera(now);
        self.poll_mic(now);
        self.poll_screen(now);
    }

    fn poll_camera(&mut self, now: f64) {
        let Some(host) = &self.camera_host else { return };
        let (failure, frame, running) = match host.try_borrow_mut() {
            Ok(mut host) => (host.failure(), host.frame(), host.running()),
            Err(_) => (None, None, false),
        };
        if let Some(error) = failure {
            self.camera_frame = None;
            self.camera.fail("camera", error);
            return;
        }
        if let Some(frame) = frame {
            if self.camera.status == DeviceStatus::Starting {
                self.camera.status = DeviceStatus::On;
                self.camera.message = None;
            }
            self.camera.frame_size = Some((frame.width, frame.height));
            self.camera_frame = Some(frame);
        }
        match self.camera.status {
            DeviceStatus::Starting if running => {
                self.camera.status = DeviceStatus::On;
                self.camera.message = None;
            }
            DeviceStatus::On | DeviceStatus::Starting => {
                if let Some(started) = self.camera.started_at
                    && now - started > 8.0
                    && !running
                {
                    self.camera.stop_failed("camera", "The camera did not start.");
                }
            }
            _ => {}
        }
    }

    fn poll_mic(&mut self, now: f64) {
        let Some(host) = &self.mic_host else { return };
        let (failure, level, running) = match host.try_borrow_mut() {
            Ok(mut host) => (host.failure(), host.level(), host.running()),
            Err(_) => (None, 0.0, true),
        };
        if let Some(error) = failure {
            self.mic.level = 0.0;
            self.mic.fail("microphone", error);
            return;
        }
        self.mic.level = (self.mic.level + (level.clamp(0.0, 1.0) - self.mic.level) * 0.6).clamp(0.0, 1.0);
        if self.mic.status == DeviceStatus::On
            && !running
            && self.mic.started_at.is_some_and(|started| now - started > 0.3)
        {
            self.mic.stop_failed("microphone", "The microphone stopped.");
        }
    }

    fn poll_screen(&mut self, now: f64) {
        let Some(host) = &self.screen_host else { return };
        let (failure, frame, running) = match host.try_borrow_mut() {
            Ok(mut host) => (host.failure(), host.frame(), host.running()),
            Err(_) => (None, None, false),
        };
        if let Some(error) = failure {
            self.screen_frame = None;
            self.screen.fail("screen", error);
            return;
        }
        if let Some(frame) = frame {
            self.screen.frame_size = Some((frame.width, frame.height));
            self.screen_frame = Some(frame);
        }
        if self.screen.status == DeviceStatus::Starting && running {
            self.screen.status = DeviceStatus::On;
            self.screen.message = None;
        }
        if self.screen.status == DeviceStatus::On
            && !running
            && self.screen.started_at.is_some_and(|started| now - started > 0.3)
        {
            self.screen.stop_failed("screen", "Screen capture stopped.");
        }
    }

    fn upload(&mut self, ctx: &egui::Context) {
        if let Some(frame) = self.camera_frame.take() {
            upload_frame(&mut self.camera, ctx, "record.camera", frame);
        }
        if let Some(frame) = self.screen_frame.take() {
            upload_frame(&mut self.screen, ctx, "record.screen", frame);
        }
    }

    // ---------- camera ----------

    /// The camera split button. Returns whether the on/off state changed (for the shell's bubble).
    pub(super) fn camera_button(&mut self, ui: &mut Ui, now: f64) -> bool {
        self.tick(ui.ctx(), now);
        let mut changed = false;
        let enabled = self.camera_host.is_some();
        let (chev, main) = split(ui, enabled, self.camera.is_on(), "Camera", "Show cameras");
        let main = main.on_hover_text(self.camera.status_text("camera"));
        if main.clicked() && enabled {
            if self.camera.is_on() {
                self.stop_camera();
                changed = true;
            } else if self.camera.can_start() {
                self.start_camera();
                changed = true;
            }
        }
        let chev = chev.on_hover_text("Choose a camera");
        egui::Popup::menu(&chev).width(250.0).show(|ui| {
            ui.label(RichText::new("Camera").strong().size(13.0));
            status_line(ui, &self.camera, "camera");
            ui.separator();
            if !enabled {
                ui.label(RichText::new("Camera capture isn't available on this platform.").size(11.0).color(FAINT));
                return;
            }
            self.ensure_cameras();
            let mut pick: Option<Option<String>> = None;
            if ui.selectable_label(self.camera.selected.is_none(), "System default").clicked() {
                pick = Some(None);
            }
            for device in &self.camera.devices {
                if ui.selectable_label(self.camera.selected.as_deref() == Some(device.id.as_str()), &device.name).clicked() {
                    pick = Some(Some(device.id.clone()));
                }
            }
            if self.camera.devices.is_empty() {
                ui.label(RichText::new("No cameras found.").size(11.0).color(FAINT));
            }
            if ui.button("Refresh").clicked() {
                self.camera.listed = false;
                self.ensure_cameras();
            }
            if let Some(choice) = pick {
                changed |= self.select_camera(choice);
                ui.close();
            }
        });
        // A device menu that was not opened still benefits from a populated list on hover.
        if main.hovered() && !self.camera.listed {
            self.ensure_cameras();
        }
        changed
    }

    fn ensure_cameras(&mut self) {
        if self.camera.listed {
            return;
        }
        self.camera.listed = true;
        let result = self.camera_host.as_ref().and_then(|host| match host.try_borrow_mut() {
            Ok(mut host) => Some(host.enumerate()),
            Err(_) => None,
        });
        if let Some(result) = result {
            match result {
                Ok(devices) => {
                    self.camera.devices = devices;
                    if self.camera.devices.is_empty() && self.camera.status == DeviceStatus::Off {
                        self.camera.status = DeviceStatus::Unavailable;
                        self.camera.message = Some(CaptureError::Unavailable.message("camera"));
                    } else if !self.camera.devices.is_empty() && self.camera.status == DeviceStatus::Unavailable {
                        self.camera.status = DeviceStatus::Off;
                        self.camera.message = None;
                    }
                }
                Err(error) => self.camera.fail("camera", error),
            }
        }
    }

    fn select_camera(&mut self, id: Option<String>) -> bool {
        if self.camera.selected == id {
            return false;
        }
        self.camera.selected = id;
        if self.camera.is_on() {
            self.start_camera();
        }
        true
    }

    fn start_camera(&mut self) {
        let selected = self.camera.selected.clone();
        let result = match self.camera_host.as_ref().and_then(|host| host.try_borrow_mut().ok()) {
            Some(mut host) => host.open(selected.as_deref()),
            None => Err(CaptureError::Unsupported),
        };
        match result {
            Ok(()) => {
                self.camera.status = DeviceStatus::Starting;
                self.camera.message = Some("Waiting for the camera…".to_string());
                self.camera.started_at = Some(self.last_tick);
            }
            Err(error) => self.camera.fail("camera", error),
        }
    }

    fn stop_camera(&mut self) {
        if let Some(host) = &self.camera_host
            && let Ok(mut host) = host.try_borrow_mut()
        {
            host.close();
        }
        self.camera.status = DeviceStatus::Off;
        self.camera.message = None;
        self.camera.frame_size = None;
        self.camera_frame = None;
        self.camera.started_at = None;
    }

    // ---------- microphone ----------

    pub(super) fn mic_button(&mut self, ui: &mut Ui, now: f64) -> bool {
        self.tick(ui.ctx(), now);
        let mut changed = false;
        let on = self.mic.status == DeviceStatus::On;
        let enabled = self.mic_host.is_some();
        meter(ui, self.mic.level, on);
        let (chev, main) = split(ui, enabled, on, "Mic", "Show microphones");
        let main = main.on_hover_text(self.mic.status_text("microphone"));
        if main.clicked() && enabled {
            if on {
                self.stop_mic();
                changed = true;
            } else if self.mic.can_start() {
                self.start_mic();
                changed = true;
            }
        }
        let chev = chev.on_hover_text("Choose a microphone");
        egui::Popup::menu(&chev).width(250.0).show(|ui| {
            ui.label(RichText::new("Microphone").strong().size(13.0));
            status_line(ui, &self.mic, "microphone");
            if self.mic_host.is_some() {
                meter_bar(ui, self.mic.level);
            }
            ui.separator();
            if !enabled {
                ui.label(RichText::new("Microphone capture isn't available on this platform.").size(11.0).color(FAINT));
                return;
            }
            self.ensure_mics();
            let mut pick: Option<Option<String>> = None;
            if ui.selectable_label(self.mic.selected.is_none(), "System default").clicked() {
                pick = Some(None);
            }
            for device in &self.mic.devices {
                if ui.selectable_label(self.mic.selected.as_deref() == Some(device.id.as_str()), &device.name).clicked() {
                    pick = Some(Some(device.id.clone()));
                }
            }
            if self.mic.devices.is_empty() {
                ui.label(RichText::new("No microphones found.").size(11.0).color(FAINT));
            }
            if ui.button("Refresh").clicked() {
                self.mic.listed = false;
                self.ensure_mics();
            }
            if let Some(choice) = pick {
                changed |= self.select_mic(choice);
                ui.close();
            }
        });
        if main.hovered() && !self.mic.listed {
            self.ensure_mics();
        }
        changed
    }

    fn ensure_mics(&mut self) {
        if self.mic.listed {
            return;
        }
        self.mic.listed = true;
        let result = self.mic_host.as_ref().and_then(|host| match host.try_borrow_mut() {
            Ok(mut host) => Some(host.enumerate()),
            Err(_) => None,
        });
        if let Some(result) = result {
            match result {
                Ok(devices) => {
                    self.mic.devices = devices;
                    if self.mic.devices.is_empty() && self.mic.status == DeviceStatus::Off {
                        self.mic.status = DeviceStatus::Unavailable;
                        self.mic.message = Some(CaptureError::Unavailable.message("microphone"));
                    } else if !self.mic.devices.is_empty() && self.mic.status == DeviceStatus::Unavailable {
                        self.mic.status = DeviceStatus::Off;
                        self.mic.message = None;
                    }
                }
                Err(error) => self.mic.fail("microphone", error),
            }
        }
    }

    fn select_mic(&mut self, id: Option<String>) -> bool {
        if self.mic.selected == id {
            return false;
        }
        self.mic.selected = id;
        if self.mic.status == DeviceStatus::On {
            self.start_mic();
        }
        true
    }

    fn start_mic(&mut self) {
        let selected = self.mic.selected.clone();
        let result = match self.mic_host.as_ref().and_then(|host| host.try_borrow_mut().ok()) {
            // `MicIn::start` predates this view and reports a platform string; map the words the
            // OSes actually use to the explicit states, and keep anything else as the message.
            Some(mut host) => host
                .select(selected.as_deref())
                .and_then(|()| host.start().map_err(|why| classify_audio_error(&why))),
            None => Err(CaptureError::Unsupported),
        };
        match result {
            Ok(()) => {
                self.mic.status = DeviceStatus::On;
                self.mic.message = None;
                self.mic.level = 0.0;
                self.mic.started_at = Some(self.last_tick);
            }
            Err(error) => self.mic.fail("microphone", error),
        }
    }

    fn stop_mic(&mut self) {
        if let Some(host) = &self.mic_host
            && let Ok(mut host) = host.try_borrow_mut()
        {
            host.stop();
        }
        self.mic.status = DeviceStatus::Off;
        self.mic.message = None;
        self.mic.level = 0.0;
        self.mic.started_at = None;
    }

    // ---------- screen ----------

    /// The screen-recording split button. The label starts/stops; the chevron opens the region
    /// picker and whole-screen shortcuts.
    pub(super) fn screen_button(&mut self, ui: &mut Ui, now: f64) -> bool {
        self.tick(ui.ctx(), now);
        let mut changed = false;
        let on = self.screen.status == DeviceStatus::On;
        let enabled = self.screen_host.is_some();
        let (chev, main) = split(ui, enabled, on, "Screen", "Show screen recording options");
        let main = main.on_hover_text(self.screen.status_text("screen"));
        if main.clicked() && enabled {
            if on {
                self.stop_screen();
            } else {
                self.open_picker();
            }
            changed = true;
        }
        let chev = chev.on_hover_text("Record part of a screen");
        egui::Popup::menu(&chev).width(250.0).show(|ui| {
            ui.label(RichText::new("Screen recording").strong().size(13.0));
            status_line(ui, &self.screen, "screen");
            ui.separator();
            if !enabled {
                ui.label(RichText::new("Screen capture isn't available on this platform.").size(11.0).color(FAINT));
                return;
            }
            if on {
                if ui.button("Stop recording").clicked() {
                    self.stop_screen();
                    changed = true;
                    ui.close();
                }
            }
            if ui.button("Select area…").clicked() {
                self.open_picker();
                ui.close();
            }
            if ui.button("Entire screen").clicked() {
                self.start_entire_screen();
                changed = true;
                ui.close();
            }
        });
        if main.hovered() && !self.screen.listed {
            self.ensure_screens();
        }
        self.picker_ui(ui);
        changed
    }

    fn ensure_screens(&mut self) {
        if self.screen.listed {
            return;
        }
        self.screen.listed = true;
        let result = self.screen_host.as_ref().and_then(|host| match host.try_borrow_mut() {
            Ok(mut host) => Some(host.screens()),
            Err(_) => None,
        });
        if let Some(result) = result {
            match result {
                Ok(screens) => self.screens = screens,
                Err(error) => self.screen.fail("screen", error),
            }
        }
    }

    fn open_picker(&mut self) {
        self.ensure_screens();
        if self.screens.is_empty() {
            if self.screen.message.is_none() {
                self.screen.status = DeviceStatus::Unavailable;
                self.screen.message = Some(CaptureError::Unavailable.message("screen"));
            }
            return;
        }
        self.picker = Some(Picker::new());
    }

    fn start_entire_screen(&mut self) {
        self.ensure_screens();
        let Some(screen) = self.screens.first() else {
            self.screen.status = DeviceStatus::Unavailable;
            self.screen.message = Some(CaptureError::Unavailable.message("screen"));
            return;
        };
        let region = ScreenRegion { screen: screen.id.clone(), x: 0, y: 0, width: screen.width, height: screen.height };
        self.start_screen_region(region);
    }

    fn start_screen_region(&mut self, region: ScreenRegion) {
        let result = match self.screen_host.as_ref().and_then(|host| host.try_borrow_mut().ok()) {
            Some(mut host) => host.start(&region),
            None => Err(CaptureError::Unsupported),
        };
        match result {
            Ok(()) => {
                self.screen.status = DeviceStatus::Starting;
                self.screen.message = Some("Starting screen capture…".to_string());
                self.screen.started_at = Some(self.last_tick);
                self.screen.frame_size = Some((region.width, region.height));
            }
            Err(error) => {
                self.screen.fail("screen", error);
                if let Some(picker) = &mut self.picker {
                    picker.message = self.screen.message.clone();
                }
            }
        }
    }

    fn stop_screen(&mut self) {
        if let Some(host) = &self.screen_host
            && let Ok(mut host) = host.try_borrow_mut()
        {
            host.stop();
        }
        self.screen.status = DeviceStatus::Off;
        self.screen.message = None;
        self.screen.frame_size = None;
        self.screen_frame = None;
        self.screen.started_at = None;
    }

    /// The region picker panel. It sits on the foreground layer, dims the view behind it and
    /// maps a drag on a monitor diagram to physical pixels on that monitor.
    fn picker_ui(&mut self, ui: &mut Ui) {
        let Some(mut picker) = self.picker.take() else { return };
        if self.screens.is_empty() {
            self.picker = Some(picker);
            return;
        }
        if picker.screen >= self.screens.len() {
            picker.screen = 0;
            picker.sel = None;
            picker.drag_from = None;
        }
        let screens = &self.screens;
        let full = ui.ctx().content_rect();
        let mut close = false;
        let mut start: Option<ScreenRegion> = None;
        egui::Area::new(egui::Id::new("record_screen_picker")).order(egui::Order::Foreground).fixed_pos(full.min).show(ui.ctx(), |ui| {
            let (dim, _) = ui.allocate_exact_size(full.size(), Sense::click());
            ui.painter().rect_filled(dim, CornerRadius::ZERO, Color32::from_black_alpha(170));
            let panel = Rect::from_center_size(full.center(), vec2(full.width().min(520.0), full.height().min(410.0)));
            ui.painter().rect_filled(panel, CornerRadius::same(10), Color32::from_rgb(30, 30, 34));
            ui.painter().rect_stroke(panel, CornerRadius::same(10), Stroke::new(1.0, PANEL_BORDER), StrokeKind::Inside);
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(panel.shrink(18.0)).layout(egui::Layout::top_down(egui::Align::Min)));
            child.spacing_mut().item_spacing = vec2(8.0, 8.0);
            child.visuals_mut().override_text_color = Some(FG);
            child.label(RichText::new("Record screen area").strong().size(14.0));
            child.label(RichText::new("Pick a display, then drag over the part to record.").size(11.0).color(DIM));
            let selected = screens.get(picker.screen).map(|s| s.name.as_str()).unwrap_or("Display");
            craft_ui::ComboBox::from_id_salt("record_screen_pick").width(220.0).selected_text(selected).show_ui(&mut child, |ui| {
                for (i, screen) in screens.iter().enumerate() {
                    if ui.selectable_label(picker.screen == i, format!("{} ({}×{})", screen.name, screen.width, screen.height)).clicked() {
                        picker.screen = i;
                        picker.sel = None;
                        picker.drag_from = None;
                    }
                }
            });
            let Some(screen) = screens.get(picker.screen) else {
                close = true;
                return;
            };
            // A diagram of the monitor, inset in the panel.
            let avail = child.available_size();
            let aspect = if screen.height == 0 { 16.0 / 9.0 } else { screen.width as f32 / screen.height as f32 };
            let mut size = vec2(avail.x, avail.y - 76.0);
            if size.y / size.x > 1.0 / aspect.max(0.01) {
                size.y = size.x / aspect.max(0.01);
            } else {
                size.x = size.y * aspect;
            }
            let (diagram, resp) = child.allocate_exact_size(size.max(vec2(120.0, 68.0)), Sense::click_and_drag());
            ui.painter().rect_filled(diagram, CornerRadius::same(4), Color32::from_gray(52));
            if let Some(sel) = picker.sel {
                let r = Rect::from_min_max(
                    pos2(diagram.min.x + sel.min.x * diagram.width(), diagram.min.y + sel.min.y * diagram.height()),
                    pos2(diagram.min.x + sel.max.x * diagram.width(), diagram.min.y + sel.max.y * diagram.height()),
                );
                ui.painter().rect_filled(r, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(96, 176, 238, 70));
                ui.painter().rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.5, ACCENT), StrokeKind::Inside);
            }
            let norm = |p: egui::Pos2| {
                pos2(
                    ((p.x - diagram.min.x) / diagram.width().max(1.0)).clamp(0.0, 1.0),
                    ((p.y - diagram.min.y) / diagram.height().max(1.0)).clamp(0.0, 1.0),
                )
            };
            if let Some(pointer) = ui.ctx().input(|i| i.pointer.interact_pos()) {
                if resp.drag_started() && diagram.contains(pointer) {
                    picker.drag_from = Some(norm(pointer));
                    picker.sel = Some(Rect::from_min_max(norm(pointer), norm(pointer)));
                } else if resp.dragged() {
                    if let Some(from) = picker.drag_from {
                        let to = norm(pointer);
                        picker.sel = Some(Rect::from_min_max(pos2(from.x.min(to.x), from.y.min(to.y)), pos2(from.x.max(to.x), from.y.max(to.y))));
                    }
                }
                if resp.drag_stopped() {
                    picker.drag_from = None;
                }
            }
            if let Some(sel) = picker.sel
                && (sel.width() * sel.height()) < 0.002
            {
                picker.sel = None;
            }
            if let Some(msg) = &picker.message {
                child.label(RichText::new(msg).size(11.0).color(WARN));
            }
            child.horizontal(|ui| {
                let whole = ScreenRegion { screen: screen.id.clone(), x: 0, y: 0, width: screen.width, height: screen.height };
                if ui.button("Entire screen").clicked() {
                    start = Some(whole);
                }
                if ui.add_enabled(picker.sel.is_some(), egui::Button::new("Start recording")).clicked() {
                    if let Some(sel) = picker.sel {
                        start = region_from_sel(screen, sel);
                    }
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if let Some(region) = start {
            self.start_screen_region(region);
            if self.screen.status == DeviceStatus::Starting || self.screen.status == DeviceStatus::On {
                close = true;
            }
        }
        if close {
            self.picker = None;
        } else {
            self.picker = Some(picker);
        }
    }
}

/// The window-relative normalized selection on a monitor, in physical pixels.
fn region_from_sel(screen: &ScreenInfo, sel: Rect) -> Option<ScreenRegion> {
    let x = (sel.min.x.clamp(0.0, 1.0) * screen.width as f32).round();
    let y = (sel.min.y.clamp(0.0, 1.0) * screen.height as f32).round();
    let max_x = (sel.max.x.clamp(0.0, 1.0) * screen.width as f32).round();
    let max_y = (sel.max.y.clamp(0.0, 1.0) * screen.height as f32).round();
    let x = x.max(0.0).min(screen.width as f32) as u32;
    let y = y.max(0.0).min(screen.height as f32) as u32;
    let width = (max_x - x as f32).max(0.0).min((screen.width - x) as f32) as u32;
    let height = (max_y - y as f32).max(0.0).min((screen.height - y) as f32) as u32;
    let region = ScreenRegion { screen: screen.id.clone(), x, y, width, height };
    region.fit(screen)
}

/// cpal (and the platform under it) reports a string. Recognise the refusal and contention
/// wording so the control shows Denied or Busy instead of a generic failure.
fn classify_audio_error(text: &str) -> CaptureError {
    let lower = text.to_ascii_lowercase();
    if lower.contains("denied") || lower.contains("permission") || lower.contains("privacy") || lower.contains("not authorized") {
        CaptureError::Denied
    } else if lower.contains("busy") || lower.contains("in use") || lower.contains("exclusive") {
        CaptureError::Busy
    } else if lower.contains("no microphone") || lower.contains("no input") || lower.contains("no device") || lower.contains("not found") {
        CaptureError::Unavailable
    } else {
        CaptureError::Failed(text.to_string())
    }
}

fn upload_frame(state: &mut DeviceState, ctx: &egui::Context, name: &str, frame: CaptureFrame) {
    let expected = (frame.width as usize).checked_mul(frame.height as usize).and_then(|n| n.checked_mul(4));
    if expected != Some(frame.rgba.len()) {
        return;
    }
    let image = egui::ColorImage::from_rgba_unmultiplied([frame.width as usize, frame.height as usize], &frame.rgba);
    match &mut state.texture {
        Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
        None => state.texture = Some(ctx.load_texture(name, image, egui::TextureOptions::LINEAR)),
    }
}

/// The two halves of a device split button: the chevron and the label. The parent lays out
/// right-to-left, so the chevron is added first and lands to the right of the label.
fn split(ui: &mut Ui, enabled: bool, on: bool, label: &str, chevron_tip: &str) -> (egui::Response, egui::Response) {
    let color = if on { ACCENT } else { FG };
    let chev = ui.add(egui::Button::new(RichText::new("▾").size(11.0).color(if on { ACCENT } else { DIM })).frame(false));
    chev.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, chevron_tip));
    let chev = chev.on_hover_text(chevron_tip);
    let main = ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(label).size(13.0).color(color)).frame(false),
    );
    main.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    (chev, main)
}

fn status_line(ui: &mut Ui, state: &DeviceState, kind: &str) {
    let color = match state.status {
        DeviceStatus::On => ACCENT,
        DeviceStatus::Denied | DeviceStatus::Busy => WARN,
        DeviceStatus::Unavailable => DIM,
        DeviceStatus::Starting => WARN,
        DeviceStatus::Off => FAINT,
    };
    ui.label(RichText::new(state.status_text(kind)).size(11.0).color(color));
}

/// The live microphone level beside the button: four bars that light up with the peak.
fn meter(ui: &mut Ui, level: f32, on: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(20.0, 12.0), Sense::hover());
    let painter = ui.painter();
    for i in 0..4 {
        let threshold = i as f32 / 4.0 + 0.01;
        let lit = on && level >= threshold;
        let h = 4.0 + i as f32 * 2.2;
        let bar = Rect::from_min_size(pos2(rect.min.x + i as f32 * 5.0, rect.center().y - h / 2.0), vec2(3.0, h));
        painter.rect_filled(bar, CornerRadius::same(1), if lit { if i >= 3 { WARN } else { ACCENT } } else { Color32::from_gray(74) });
    }
}

/// The same level as a full-width bar in the menu.
fn meter_bar(ui: &mut Ui, level: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(3), Color32::from_gray(62));
    let w = rect.width() * level.clamp(0.0, 1.0);
    if w > 0.5 {
        let filled = Rect::from_min_size(rect.min, Vec2::new(w, rect.height()));
        painter.rect_filled(filled, CornerRadius::same(3), if level > 0.85 { WARN } else { ACCENT });
    }
}

/// The top bar's camera slot. The label toggles capture; the chevron opens the device menu.
pub(super) fn camera_button(ui: &mut Ui, rec: &mut RecordView, now: f64) -> bool {
    rec.devices.camera_button(ui, now)
}

/// The top bar's microphone slot: toggle, live level and input-device menu.
pub(super) fn mic_button(ui: &mut Ui, rec: &mut RecordView, now: f64) -> bool {
    rec.devices.mic_button(ui, now)
}

/// The top bar's screen-recording slot: start/stop and the region picker.
pub(super) fn screen_button(ui: &mut Ui, rec: &mut RecordView, now: f64) -> bool {
    rec.devices.screen_button(ui, now)
}

/// Connect the hosts the app injected. The Record view calls this once per entry; without a
/// host every control stays explicitly [`DeviceStatus::Unavailable`].
pub(super) fn connect(rec: &mut RecordView, services: &Services) {
    rec.devices.connect(services);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct CamLog {
        devices: Vec<DeviceInfo>,
        enumerate_error: Option<CaptureError>,
        open_error: Option<CaptureError>,
        opened: Vec<Option<String>>,
        closed: usize,
        running: bool,
        failure: Option<CaptureError>,
        frame: Option<CaptureFrame>,
    }

    struct FakeCam(Rc<RefCell<CamLog>>);

    impl CameraIn for FakeCam {
        fn enumerate(&mut self) -> Result<Vec<DeviceInfo>, CaptureError> {
            let log = self.0.borrow();
            match &log.enumerate_error {
                Some(error) => Err(error.clone()),
                None => Ok(log.devices.clone()),
            }
        }
        fn open(&mut self, id: Option<&str>) -> Result<(), CaptureError> {
            let mut log = self.0.borrow_mut();
            log.opened.push(id.map(str::to_string));
            match &log.open_error {
                Some(error) => Err(error.clone()),
                None => {
                    log.running = true;
                    Ok(())
                }
            }
        }
        fn close(&mut self) {
            let mut log = self.0.borrow_mut();
            log.closed += 1;
            log.running = false;
        }
        fn running(&self) -> bool {
            self.0.borrow().running
        }
        fn frame(&mut self) -> Option<CaptureFrame> {
            self.0.borrow_mut().frame.take()
        }
        fn failure(&mut self) -> Option<CaptureError> {
            self.0.borrow_mut().failure.take()
        }
    }

    #[derive(Default)]
    struct MicLog {
        devices: Vec<DeviceInfo>,
        enumerate_error: Option<CaptureError>,
        select_error: Option<CaptureError>,
        selected: Option<String>,
        start_error: Option<String>,
        running: bool,
        stops: usize,
        level: f32,
        failure: Option<CaptureError>,
    }

    struct FakeMic(Rc<RefCell<MicLog>>);

    impl MicIn for FakeMic {
        fn start(&mut self) -> Result<(), String> {
            let mut log = self.0.borrow_mut();
            match log.start_error.clone() {
                Some(error) => Err(error),
                None => {
                    log.running = true;
                    Ok(())
                }
            }
        }
        fn take(&mut self) -> (u32, Vec<f32>) {
            (48_000, Vec::new())
        }
        fn captured(&self) -> usize {
            0
        }
        fn stop(&mut self) {
            let mut log = self.0.borrow_mut();
            log.stops += 1;
            log.running = false;
        }
        fn enumerate(&mut self) -> Result<Vec<DeviceInfo>, CaptureError> {
            let log = self.0.borrow();
            match &log.enumerate_error {
                Some(error) => Err(error.clone()),
                None => Ok(log.devices.clone()),
            }
        }
        fn select(&mut self, id: Option<&str>) -> Result<(), CaptureError> {
            let mut log = self.0.borrow_mut();
            if let Some(error) = log.select_error.clone() {
                return Err(error);
            }
            log.selected = id.map(str::to_string);
            Ok(())
        }
        fn level(&mut self) -> f32 {
            self.0.borrow().level
        }
        fn failure(&mut self) -> Option<CaptureError> {
            self.0.borrow_mut().failure.take()
        }
        fn running(&self) -> bool {
            self.0.borrow().running
        }
    }

    #[derive(Default)]
    struct ScreenLog {
        screens: Vec<ScreenInfo>,
        screens_error: Option<CaptureError>,
        start_error: Option<CaptureError>,
        started: Vec<ScreenRegion>,
        running: bool,
        stops: usize,
        failure: Option<CaptureError>,
    }

    struct FakeScreen(Rc<RefCell<ScreenLog>>);

    impl ScreenIn for FakeScreen {
        fn screens(&mut self) -> Result<Vec<ScreenInfo>, CaptureError> {
            let log = self.0.borrow();
            match &log.screens_error {
                Some(error) => Err(error.clone()),
                None => Ok(log.screens.clone()),
            }
        }
        fn start(&mut self, region: &ScreenRegion) -> Result<(), CaptureError> {
            let mut log = self.0.borrow_mut();
            match &log.start_error {
                Some(error) => Err(error.clone()),
                None => {
                    log.started.push(region.clone());
                    log.running = true;
                    Ok(())
                }
            }
        }
        fn stop(&mut self) {
            let mut log = self.0.borrow_mut();
            log.stops += 1;
            log.running = false;
        }
        fn running(&self) -> bool {
            self.0.borrow().running
        }
        fn frame(&mut self) -> Option<CaptureFrame> {
            None
        }
        fn failure(&mut self) -> Option<CaptureError> {
            self.0.borrow_mut().failure.take()
        }
    }

    fn frame(width: u32, height: u32) -> CaptureFrame {
        CaptureFrame::new(0.0, width, height, vec![128; (width * height * 4) as usize]).unwrap()
    }

    fn camera_log() -> CamLog {
        CamLog { devices: vec![DeviceInfo::new("cam1", "Built-in"), DeviceInfo::new("cam2", "USB")], ..Default::default() }
    }

    fn mic_log() -> MicLog {
        MicLog { devices: vec![DeviceInfo::new("mic1", "Built-in Mic"), DeviceInfo::new("mic2", "Headset")], ..Default::default() }
    }

    fn screen_log() -> ScreenLog {
        ScreenLog { screens: vec![ScreenInfo { id: "s1".into(), name: "Display 1".into(), width: 1920, height: 1080 }], ..Default::default() }
    }

    #[test]
    fn without_a_host_every_control_is_explicitly_unavailable() {
        let mut devices = Devices::default();
        devices.connect(&Services::default());
        for state in [&devices.camera, &devices.mic, &devices.screen] {
            assert_eq!(state.status, DeviceStatus::Unavailable);
            assert!(!state.is_on());
        }
    }

    #[test]
    fn connecting_a_host_starts_off_and_a_second_connect_is_a_no_op() {
        let log = Rc::new(RefCell::new(camera_log()));
        let host: Rc<RefCell<Box<dyn CameraIn>>> = Rc::new(RefCell::new(Box::new(FakeCam(log.clone()))));
        let services = Services { camera: Some(host.clone()), ..Default::default() };
        let mut devices = Devices::default();
        devices.connect(&services);
        assert_eq!(devices.camera.status, DeviceStatus::Off);
        devices.connect(&services);
        assert_eq!(devices.camera.status, DeviceStatus::Off);
    }

    #[test]
    fn camera_toggle_opens_the_default_and_reports_the_first_frame_live() {
        let log = Rc::new(RefCell::new(camera_log()));
        let mut devices = Devices::default();
        devices.attach_camera(Box::new(FakeCam(log.clone())));
        devices.last_tick = 1.0;
        devices.start_camera();
        assert_eq!(devices.camera.status, DeviceStatus::Starting);
        assert_eq!(log.borrow().opened, vec![None]);
        log.borrow_mut().frame = Some(frame(2, 2));
        devices.poll(1.2);
        assert_eq!(devices.camera.status, DeviceStatus::On);
        assert_eq!(devices.camera.frame_size, Some((2, 2)));
        devices.stop_camera();
        assert_eq!(devices.camera.status, DeviceStatus::Off);
        assert_eq!(log.borrow().closed, 1);
    }

    #[test]
    fn a_denied_or_busy_camera_is_reported_as_such_and_can_be_retried() {
        let log = Rc::new(RefCell::new(CamLog { open_error: Some(CaptureError::Denied), ..camera_log() }));
        let mut devices = Devices::default();
        devices.attach_camera(Box::new(FakeCam(log.clone())));
        devices.last_tick = 1.0;
        devices.start_camera();
        assert_eq!(devices.camera.status, DeviceStatus::Denied);
        assert!(devices.camera.message.as_deref().is_some_and(|m| m.contains("denied")));
        assert!(devices.camera.can_start(), "a permission change can be retried");
        log.borrow_mut().open_error = Some(CaptureError::Busy);
        devices.start_camera();
        assert_eq!(devices.camera.status, DeviceStatus::Busy);
        assert!(devices.camera.message.as_deref().is_some_and(|m| m.contains("another app")));
    }

    #[test]
    fn a_camera_that_disappears_stops_with_the_reason() {
        let log = Rc::new(RefCell::new(camera_log()));
        let mut devices = Devices::default();
        devices.attach_camera(Box::new(FakeCam(log.clone())));
        devices.last_tick = 1.0;
        devices.start_camera();
        log.borrow_mut().failure = Some(CaptureError::Disconnected);
        devices.poll(1.4);
        assert_eq!(devices.camera.status, DeviceStatus::Off);
        assert!(devices.camera.message.as_deref().is_some_and(|m| m.contains("disconnected")));
    }

    #[test]
    fn the_camera_menu_lists_devices_and_selection_reopens_a_live_camera() {
        let log = Rc::new(RefCell::new(camera_log()));
        let mut devices = Devices::default();
        devices.attach_camera(Box::new(FakeCam(log.clone())));
        devices.last_tick = 1.0;
        devices.ensure_cameras();
        assert_eq!(devices.camera.devices.len(), 2);
        assert!(devices.select_camera(Some("cam2".into())));
        devices.start_camera();
        assert_eq!(log.borrow().opened, vec![Some("cam2".to_string())]);
        log.borrow_mut().frame = Some(frame(2, 2));
        devices.poll(1.1);
        assert_eq!(devices.camera.status, DeviceStatus::On);
        devices.select_camera(Some("cam1".into()));
        assert_eq!(log.borrow().opened, vec![Some("cam2".to_string()), Some("cam1".to_string())], "a live camera restarts on its new device");
    }

    #[test]
    fn an_empty_camera_list_is_unavailable_rather_than_off() {
        let log = Rc::new(RefCell::new(CamLog { devices: Vec::new(), ..Default::default() }));
        let mut devices = Devices::default();
        devices.attach_camera(Box::new(FakeCam(log)));
        devices.ensure_cameras();
        assert_eq!(devices.camera.status, DeviceStatus::Unavailable);
        assert!(devices.camera.message.is_some());
    }

    #[test]
    fn a_denied_enumeration_keeps_the_permission_reason() {
        let log = Rc::new(RefCell::new(CamLog { enumerate_error: Some(CaptureError::Denied), ..Default::default() }));
        let mut devices = Devices::default();
        devices.attach_camera(Box::new(FakeCam(log)));
        devices.ensure_cameras();
        assert_eq!(devices.camera.status, DeviceStatus::Denied);
        assert!(devices.camera.message.as_deref().is_some_and(|m| m.contains("denied")));
    }

    #[test]
    fn mic_toggle_starts_the_selected_input_and_the_meter_follows_the_host() {
        let log = Rc::new(RefCell::new(mic_log()));
        let mut devices = Devices::default();
        devices.attach_mic(Box::new(FakeMic(log.clone())));
        devices.last_tick = 1.0;
        devices.ensure_mics();
        assert!(devices.select_mic(Some("mic1".into())));
        devices.start_mic();
        assert_eq!(devices.mic.status, DeviceStatus::On);
        assert_eq!(log.borrow().selected.as_deref(), Some("mic1"));
        assert!(log.borrow().running);
        log.borrow_mut().level = 0.8;
        devices.poll(1.1);
        assert!(devices.mic.level > 0.4, "the meter follows the host peak");
        devices.stop_mic();
        assert_eq!(devices.mic.status, DeviceStatus::Off);
        assert_eq!(devices.mic.level, 0.0);
        assert_eq!(log.borrow().stops, 1);
    }

    #[test]
    fn a_denied_microphone_start_is_explicit() {
        let log = Rc::new(RefCell::new(MicLog { start_error: Some("Permission denied by the system".into()), ..mic_log() }));
        let mut devices = Devices::default();
        devices.attach_mic(Box::new(FakeMic(log)));
        devices.last_tick = 1.0;
        devices.start_mic();
        assert_eq!(devices.mic.status, DeviceStatus::Denied);
        assert!(devices.mic.message.as_deref().is_some_and(|m| m.contains("denied")));
    }

    #[test]
    fn a_busy_microphone_is_explicit() {
        let log = Rc::new(RefCell::new(MicLog { start_error: Some("device is busy (exclusive)".into()), ..mic_log() }));
        let mut devices = Devices::default();
        devices.attach_mic(Box::new(FakeMic(log)));
        devices.last_tick = 1.0;
        devices.start_mic();
        assert_eq!(devices.mic.status, DeviceStatus::Busy);
    }

    #[test]
    fn the_microphone_list_and_selection_come_from_the_host() {
        let log = Rc::new(RefCell::new(mic_log()));
        let mut devices = Devices::default();
        devices.attach_mic(Box::new(FakeMic(log.clone())));
        devices.ensure_mics();
        assert_eq!(devices.mic.devices.len(), 2);
        assert!(devices.select_mic(Some("mic2".into())));
        assert_eq!(log.borrow().selected.as_deref(), Some("mic2"));
    }

    #[test]
    fn the_screen_picker_starts_the_whole_screen_and_reports_live() {
        let log = Rc::new(RefCell::new(screen_log()));
        let mut devices = Devices::default();
        devices.attach_screen(Box::new(FakeScreen(log.clone())));
        devices.last_tick = 1.0;
        devices.open_picker();
        assert!(devices.picker.is_some());
        devices.start_entire_screen();
        assert_eq!(
            log.borrow().started,
            vec![ScreenRegion { screen: "s1".into(), x: 0, y: 0, width: 1920, height: 1080 }]
        );
        assert_eq!(devices.screen.status, DeviceStatus::Starting);
        log.borrow_mut().running = true;
        devices.poll(1.1);
        assert_eq!(devices.screen.status, DeviceStatus::On);
        devices.stop_screen();
        assert_eq!(devices.screen.status, DeviceStatus::Off);
        assert_eq!(log.borrow().stops, 1);
    }

    #[test]
    fn a_denied_screen_start_keeps_the_picker_open_with_the_reason() {
        let log = Rc::new(RefCell::new(ScreenLog { start_error: Some(CaptureError::Denied), ..screen_log() }));
        let mut devices = Devices::default();
        devices.attach_screen(Box::new(FakeScreen(log)));
        devices.last_tick = 1.0;
        devices.open_picker();
        let region = ScreenRegion { screen: "s1".into(), x: 0, y: 0, width: 100, height: 100 };
        devices.start_screen_region(region);
        assert_eq!(devices.screen.status, DeviceStatus::Denied);
        assert!(devices.picker.as_ref().is_some_and(|p| p.message.is_some()));
    }

    #[test]
    fn a_missing_screen_host_leaves_the_picker_closed_and_says_why() {
        let mut devices = Devices::default();
        devices.open_picker();
        assert!(devices.picker.is_none());
        assert_eq!(devices.screen.status, DeviceStatus::Unavailable);
    }

    #[test]
    fn a_normalized_picker_drag_becomes_physical_pixels_on_that_monitor() {
        let screen = ScreenInfo { id: "s1".into(), name: "Display".into(), width: 1920, height: 1080 };
        let sel = Rect::from_min_max(pos2(0.25, 0.5), pos2(0.5, 0.75));
        assert_eq!(
            region_from_sel(&screen, sel),
            Some(ScreenRegion { screen: "s1".into(), x: 480, y: 540, width: 480, height: 270 })
        );
        let tiny = Rect::from_min_max(pos2(0.5, 0.5), pos2(0.5, 0.5));
        assert_eq!(region_from_sel(&screen, tiny), None);
    }

    #[test]
    fn status_texts_name_the_device_and_never_claim_a_missing_one_is_live() {
        let mut state = DeviceState::default();
        assert!(state.status_text("camera").contains("camera"));
        state.fail("camera", CaptureError::Denied);
        assert!(state.status_text("camera").contains("denied"));
        state.status = DeviceStatus::On;
        state.frame_size = Some((1280, 720));
        assert_eq!(state.status_text("camera"), "Live · 1280×720");
    }

    #[test]
    fn platform_error_wording_maps_to_denied_busy_or_unavailable() {
        assert_eq!(classify_audio_error("Permission denied"), CaptureError::Denied);
        assert_eq!(classify_audio_error("the device is busy"), CaptureError::Busy);
        assert_eq!(classify_audio_error("no microphone"), CaptureError::Unavailable);
        assert_eq!(classify_audio_error("unexpected driver fault"), CaptureError::Failed("unexpected driver fault".into()));
    }

    fn harness(devices: Devices) -> egui_kittest::Harness<'static, Devices> {
        use egui_kittest::Harness;
        let mut harness = Harness::builder().with_size(vec2(520.0, 120.0)).build_ui_state(
            |ui, devices: &mut Devices| {
                devices.camera_button(ui, 1.0);
            },
            devices,
        );
        harness.run_steps(2);
        harness
    }

    #[test]
    fn the_camera_label_toggles_only_when_a_host_is_connected() {
        let log = Rc::new(RefCell::new(camera_log()));
        let mut devices = Devices::default();
        devices.attach_camera(Box::new(FakeCam(log.clone())));
        devices.last_tick = 1.0;
        let mut harness = harness(devices);
        harness.get_by_label("Camera").click();
        harness.run_steps(2);
        assert_eq!(harness.state().camera.status, DeviceStatus::Starting);
        assert_eq!(log.borrow().opened, vec![None]);

        let mut harness = harness(Devices::default());
        harness.get_by_label("Camera").click();
        harness.run_steps(2);
        assert_eq!(harness.state().camera.status, DeviceStatus::Unavailable, "a disabled control cannot fake a device");
    }

    #[test]
    fn the_camera_menu_selects_a_device_with_the_mouse() {
        let log = Rc::new(RefCell::new(camera_log()));
        let mut devices = Devices::default();
        devices.attach_camera(Box::new(FakeCam(log)));
        let mut harness = harness(devices);
        harness.get_by_label("Show cameras").click();
        harness.run_steps(2);
        harness.get_by_label("USB").click();
        harness.run_steps(2);
        assert_eq!(harness.state().camera.selected.as_deref(), Some("cam2"));
    }
}
