#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! A per-pixel-alpha Win32 overlay. We deliberately avoid a GPU swapchain here:
//! on many Windows 10 drivers an HWND swapchain reports an opaque alpha mode,
//! which turns an otherwise transparent overlay into a black fullscreen window.

use std::{
    env,
    fs,
    net::UdpSocket,
    os::windows::ffi::OsStrExt,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    ptr::null_mut,
    slice,
    process::{Command, Stdio},
    sync::mpsc::{sync_channel, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use image::{imageops::FilterType, RgbaImage};
use regex::Regex;
use serde_json::{json, Value};
use windows::{
    core::{Interface, Result, PCWSTR, PWSTR},
    Win32::{
        Foundation::{
            CloseHandle, COLORREF, ERROR_ACCESS_DENIED, ERROR_CANCELLED, ERROR_NO_MORE_ITEMS,
            HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WAIT_ABANDONED, WAIT_OBJECT_0,
            WIN32_ERROR, WPARAM,
        },
        Graphics::Gdi::{
            CreateBitmap, CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC,
            DeleteObject, DrawTextW, GetMonitorInfoW, MonitorFromPoint, SelectObject, SetBkMode,
            SetTextColor, AC_SRC_ALPHA, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
                ANTIALIASED_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH,
                DIB_RGB_COLORS, DT_CENTER, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, FF_DONTCARE, FONT_FAMILY,
            MONITORINFO,
            MONITOR_DEFAULTTONEAREST, OUT_TT_PRECIS, TRANSPARENT,
        },
        Media::Multimedia::mciSendStringW,
        Storage::FileSystem::GetFileAttributesW,
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
                COINIT_APARTMENTTHREADED, IPersistFile, STGM_READ,
            },
            Environment::ExpandEnvironmentStringsW,
            Registry::{
                RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY,
                HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY,
                KEY_WOW64_64KEY, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
            },
            Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject},
        },
        UI::{
            HiDpi::{GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS},
            Shell::{
                IShellLinkW, SHFileOperationW, ShellExecuteW, ShellLink, FOF_ALLOWUNDO,
                FOF_NO_CONNECTED_ELEMENTS, FOF_NOCONFIRMATION, FOF_NOERRORUI, FO_DELETE,
                SHFILEOPSTRUCTW,
            },
            WindowsAndMessaging::{
                CreateIconIndirect, CreateWindowExW, DefWindowProcW, DestroyCursor, DestroyWindow,
                DispatchMessageW, GetCursorPos, GetMessageW, LoadCursorW, MessageBoxW,
                PostQuitMessage, RegisterClassW, SetCursor, SetTimer, SetWindowLongPtrW,
                SetWindowPos, ShowWindow, TranslateMessage, IDC_ARROW, IDYES, MB_ICONWARNING,
                MB_YESNO,
                UpdateLayeredWindow, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, HCURSOR,
                HWND_TOPMOST, ICONINFO, MSG, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_SHOW,
                SW_SHOWNORMAL, ULW_ALPHA, WM_DESTROY, WM_HOTKEY, WM_KEYDOWN, WM_LBUTTONDOWN,
                WM_NCCREATE, WM_SETCURSOR, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
                WS_EX_TOPMOST, WS_POPUP,
            },
        },
    },
};

const FRAME_RATE: f32 = 8.0;
const TICK_ID: usize = 1;
const ESC_HOTKEY_ID: i32 = 0x4d44;
// Explorer invokes a legacy verb once per selected item.  The first process
// briefly owns this loopback endpoint and collects the sibling invocations,
// so the overlay receives the complete selection exactly once.
const SELECTION_BROKER_ADDR: &str = "127.0.0.1:39618";
const SELECTION_BROKER_MAX_PACKET: usize = 60 * 1024;
const SELECTION_BROKER_MUTEX: &str = "Local\\MonsterDeleter.SelectionBroker.v1";
// Explorer can create legacy-verb processes one after another.  Keep a
// bounded, stable collection window after the first valid path so a slow
// sibling cannot be dropped from the same selection transaction.
const SELECTION_BROKER_COLLECTION_WINDOW: Duration = Duration::from_secs(1);
const SELECTION_BROKER_MAX_WAIT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Select,
    FadeOut,
    Walk,
    Point,
    Ask,
    DetectUninstall,
    UninstallModeAsk,
    SelectUninstallTarget,
    UninstallAsk,
    UninstallUnavailable,
    Kick,
    Leo,
    Fly,
    Elevate,
    Error,
}

#[derive(Clone, Copy)]
struct RectI {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl RectI {
    fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

struct Sprite {
    frames: Vec<RgbaImage>,
    width: i32,
    height: i32,
}

/// An exact registry-backed association between a shortcut target and an
/// installed desktop application. We deliberately retain only the uninstall
/// command published by Windows' installed-program registry entry.
#[derive(Clone)]
struct InstalledApp {
    display_name: String,
    uninstall_command: String,
}

#[derive(Clone)]
struct BcuCandidate {
    /// Exact Explorer item selected by the user.  This is never replaced by
    /// the resolved executable, so shortcut deletion cannot accidentally
    /// recycle its target.
    source: PathBuf,
    /// Resolved executable used only by BCUninstaller matching/launching.
    executable: PathBuf,
    id: String,
    display_name: String,
}

enum BcuProbe {
    Candidate(BcuCandidate),
    ExecutableWithoutUninstaller,
    NotExecutableShortcut,
}

const DEFAULT_UNINSTALL_TARGET_PATTERNS: [&str; 2] = [
    r"(?i)^.*\.lnk$",
    r"(?i)^.*\.exe$",
];
const DEFAULT_BATCH_UNINSTALL_TARGET_PATTERNS: [&str; 1] = [r"(?i)^.*\.lnk$"];

#[derive(Clone)]
struct UninstallConfig {
    enabled: bool,
    mode: String,
    target_patterns: Vec<String>,
    /// When several Explorer items are selected, only matching items enter
    /// uninstall detection.  The conservative default is shortcuts only.
    batch_target_patterns: Vec<String>,
}

impl Default for UninstallConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: "official".to_owned(),
            target_patterns: DEFAULT_UNINSTALL_TARGET_PATTERNS
                .iter()
                .map(|pattern| (*pattern).to_owned())
                .collect(),
            batch_target_patterns: DEFAULT_BATCH_UNINSTALL_TARGET_PATTERNS
                .iter()
                .map(|pattern| (*pattern).to_owned())
                .collect(),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum AfterKick {
    FlyAway,
    AskSingleUninstall,
    SelectUninstallTarget,
}

impl Sprite {
    fn load(path: &Path, height: u32) -> Option<Self> {
        let sheet = image::open(path).ok()?.to_rgba8();
        let frame_width = sheet.width() / 5;
        let frame_height = sheet.height() / 3;
        if frame_width == 0 || frame_height == 0 {
            return None;
        }
        let width = frame_width * height / frame_height;
        let mut frames = Vec::with_capacity(15);
        for row in 0..3 {
            for column in 0..5 {
                let crop = image::imageops::crop_imm(
                    &sheet,
                    column * frame_width,
                    row * frame_height,
                    frame_width,
                    frame_height,
                )
                .to_image();
                frames.push(image::imageops::resize(
                    &crop,
                    width,
                    height,
                    FilterType::Lanczos3,
                ));
            }
        }
        Some(Self {
            frames,
            width: width as i32,
            height: height as i32,
        })
    }
}

struct Audio {
    aliases: Vec<String>,
}

impl Audio {
    fn new(assets: &Path) -> Self {
        let files = [
            (
                "monster_bgm",
                assets.join("音频").join("bgm(1).mp3"),
                "mpegvideo",
                500,
            ),
            (
                "monster_talk",
                assets.join("音频").join("monster-talk.wav"),
                "waveaudio",
                1000,
            ),
            (
                "monster_boom",
                assets.join("音频").join("monster-boom.wav"),
                "waveaudio",
                300,
            ),
        ];
        let mut aliases = Vec::new();
        for (alias, path, device_type, volume) in files {
            if path.exists()
                && mci(&format!(
                    "open \"{}\" type {device_type} alias {alias}",
                    path.display()
                )) == 0
            {
                mci(&format!("setaudio {alias} volume to {volume}"));
                aliases.push(alias.to_owned());
            }
        }
        Self { aliases }
    }
    fn play_loop(&self) {
        mci("play monster_bgm repeat");
    }
    fn play(&self, alias: &str) {
        mci(&format!("stop {alias}"));
        mci(&format!("seek {alias} to start"));
        mci(&format!("play {alias}"));
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        for alias in &self.aliases {
            mci(&format!("stop {alias}"));
            mci(&format!("close {alias}"));
        }
    }
}

struct Surface {
    dc: windows::Win32::Graphics::Gdi::HDC,
    bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
    old: windows::Win32::Graphics::Gdi::HGDIOBJ,
    bits: *mut u8,
}

impl Surface {
    unsafe fn new(width: i32, height: i32) -> Option<Self> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = null_mut();
        let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let dc = CreateCompatibleDC(None);
        let old = SelectObject(dc, bitmap.into());
        Some(Self {
            dc,
            bitmap,
            old,
            bits: bits.cast(),
        })
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Recreates the original 40×40 red targeting cursor as an operating-system
/// cursor. It therefore follows the mouse independently of overlay repainting.
unsafe fn create_target_cursor() -> Option<HCURSOR> {
    // Original Qt geometry is 40px / radius 12px / 2px pen. This is the
    // same proportion at 140% so it stays equally legible on high-DPI screens.
    const SIDE: i32 = 56;
    const CENTER: i32 = 28;
    const RADIUS: i32 = 17;
    const GAP: i32 = 6;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: SIDE,
            biHeight: -SIDE,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = null_mut();
    let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
    let pixels = slice::from_raw_parts_mut(bits.cast::<u8>(), (SIDE * SIDE * 4) as usize);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let dx = x - CENTER;
            let dy = y - CENTER;
            let distance = ((dx * dx + dy * dy) as f32).sqrt();
            let circle_coverage = (2.0 - (distance - RADIUS as f32).abs()).clamp(0.0, 1.0);
            let vertical = if dy.abs() > GAP {
                (2.0 - dx.abs() as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let horizontal = if dx.abs() > GAP {
                (2.0 - dy.abs() as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let coverage = circle_coverage.max(vertical).max(horizontal);
            if coverage > 0.0 {
                let offset = ((y * SIDE + x) * 4) as usize;
                pixels[offset] = 0;
                pixels[offset + 1] = 0;
                pixels[offset + 2] = 255;
                pixels[offset + 3] = (coverage * 255.0) as u8;
            }
        }
    }
    let mask = CreateBitmap(SIDE, SIDE, 1, 1, None);
    let icon = CreateIconIndirect(&ICONINFO {
        fIcon: false.into(),
        xHotspot: CENTER as u32,
        yHotspot: CENTER as u32,
        hbmMask: mask,
        hbmColor: color,
    })
    .ok()?;
    let _ = DeleteObject(mask.into());
    let _ = DeleteObject(color.into());
    Some(HCURSOR(icon.0))
}

struct OverlayApp {
    hwnd: HWND,
    surface: Surface,
    width: i32,
    height: i32,
    screen_x: i32,
    screen_y: i32,
    ui_scale: f32,
    pixels: Vec<u8>,
    assets: PathBuf,
    background: Option<RgbaImage>,
    /// Exact paths received from Explorer.  Keep these separate from resolved
    /// executables: a `.lnk` must never be replaced by its target for recycle.
    targets: Vec<PathBuf>,
    /// Items that still need to be moved to the Recycle Bin.  This is built as
    /// the user answers the uninstall questions, then processed in one shell
    /// operation and one monster sequence.
    delete_targets: Vec<PathBuf>,
    phase: Phase,
    phase_started: Instant,
    target_cursor: HCURSOR,
    target_position: (i32, i32),
    points_left: bool,
    walk: Option<Sprite>,
    point: Option<Sprite>,
    kick: Option<Sprite>,
    leo: Option<Sprite>,
    fly: Option<Sprite>,
    explosion: Option<Sprite>,
    explosion_started: Option<Instant>,
    explosion_positions: Vec<(i32, i32)>,
    deletion_started: bool,
    after_kick: AfterKick,
    audio: Audio,
    error: Option<String>,
    uninstall_candidates: Vec<BcuCandidate>,
    confirmed_uninstall_candidates: Vec<BcuCandidate>,
    current_uninstall_candidate: usize,
    /// A fixed cross is left at every point manually assigned to an
    /// uninstallable item while the remaining items are selected.
    uninstall_markers: Vec<(i32, i32)>,
    uninstall_probe: Option<Receiver<Vec<(PathBuf, BcuProbe)>>>,
}

impl OverlayApp {
    unsafe fn new(targets: Vec<PathBuf>) -> Option<Self> {
        if targets.is_empty() {
            return None;
        }
        let mut cursor_point = POINT::default();
        GetCursorPos(&mut cursor_point).ok()?;
        let monitor = MonitorFromPoint(cursor_point, MONITOR_DEFAULTTONEAREST);
        let mut monitor_info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut monitor_info).as_bool() {
            return None;
        }
        let screen_x = monitor_info.rcMonitor.left;
        let screen_y = monitor_info.rcMonitor.top;
        let width = monitor_info.rcMonitor.right - screen_x;
        let height = monitor_info.rcMonitor.bottom - screen_y;
        if width <= 0 || height <= 0 {
            return None;
        }
        let assets = resource_dir().join("assets");
        let background = image::open(assets.join("选择界面").join("选择界面.png"))
            .ok()
            .map(|image| image.to_rgba8());
        let surface = Surface::new(width, height)?;
        Some(Self {
            hwnd: HWND::default(),
            surface,
            width,
            height,
            screen_x,
            screen_y,
            ui_scale: 1.0,
            pixels: vec![0; width as usize * height as usize * 4],
            assets: assets.clone(),
            background,
            targets,
            delete_targets: Vec::new(),
            phase: Phase::Select,
            phase_started: Instant::now(),
            target_cursor: create_target_cursor().unwrap_or_default(),
            target_position: (width / 2, height / 2),
            points_left: false,
            walk: None,
            point: None,
            kick: None,
            leo: None,
            fly: None,
            explosion: None,
            explosion_started: None,
            explosion_positions: Vec::new(),
            deletion_started: false,
            after_kick: AfterKick::FlyAway,
            audio: Audio::new(&assets),
            error: None,
            uninstall_candidates: Vec::new(),
            confirmed_uninstall_candidates: Vec::new(),
            current_uninstall_candidate: 0,
            uninstall_markers: Vec::new(),
            uninstall_probe: None,
        })
    }

    fn elapsed(&self) -> f32 {
        self.phase_started.elapsed().as_secs_f32()
    }
    fn px(&self, logical_pixels: i32) -> i32 {
        ((logical_pixels as f32 * self.ui_scale).round() as i32).max(1)
    }
    fn enter(&mut self, phase: Phase) {
        self.phase = phase;
        self.phase_started = Instant::now();
    }
    fn frame(&self) -> usize {
        (self.elapsed() * FRAME_RATE) as usize
    }
    fn monster_position(&self, sprite: &Sprite) -> (i32, i32) {
        let x = if self.points_left {
            self.target_position.0 + 30
        } else {
            self.target_position.0 - sprite.width - 30
        };
        (x, self.target_position.1 - sprite.height / 2 + 50)
    }
    fn ensure_walk(&mut self) {
        if self.walk.is_none() {
            self.walk = Sprite::load(
                &self.assets.join("走路动效_spritesheet_transparent.png"),
                self.px(250) as u32,
            );
        }
    }
    fn ensure_point(&mut self) {
        if self.point.is_none() {
            self.point = Sprite::load(
                &self.assets.join("指着文件_spritesheet_transparent.png"),
                self.px(250) as u32,
            );
        }
    }
    fn ensure_kick_sequence(&mut self) {
        if self.kick.is_none() {
            self.kick = Sprite::load(
                &self.assets.join("踹文件动效_spritesheet_transparent.png"),
                self.px(250) as u32,
            );
        }
        if self.explosion.is_none() {
            self.explosion =
                Sprite::load(
                    &self.assets.join("爆炸_spritesheet_transparent.png"),
                    self.px(150) as u32,
                );
        }
        if self.leo.is_none() {
            self.leo = Sprite::load(
                &self.assets.join("雷欧登场_spritesheet_transparent.png"),
                self.px(250) as u32,
            );
        }
        if self.fly.is_none() {
            self.fly = Sprite::load(
                &self.assets.join("出场飞行动效_spritesheet_transparent.png"),
                self.px(250) as u32,
            );
        }
    }
    fn click(&mut self, x: i32, y: i32) {
        match self.phase {
            Phase::Select => {
                self.target_position = (x, y);
                self.points_left = x < self.width / 2;
                // The targeting crosshair belongs only to selection. Restore the
                // normal cursor before the fade/monster sequence begins.
                unsafe { restore_default_cursor() };
                self.ensure_walk();
                self.enter(Phase::FadeOut);
            }
            Phase::SelectUninstallTarget => {
                // The candidates are in a stable order.  A click binds the
                // current one to a visual point and leaves a marker behind;
                // it does not infer a position from Explorer's private view.
                self.target_position = (x, y);
                self.points_left = x < self.width / 2;
                self.uninstall_markers.push((x, y));
                unsafe { restore_default_cursor() };
                self.ensure_point();
                self.enter(Phase::Point);
            }
            Phase::Ask => {
                if self.choice_rects().iter().any(|rect| rect.contains(x, y)) {
                    if uninstall_feature_enabled() && self.begin_uninstall_probe() {
                        self.enter(Phase::DetectUninstall);
                    } else {
                        self.begin_direct_batch_recycle();
                    }
                }
            }
            Phase::UninstallAsk => {
                let choices = self.choice_rects();
                if choices[0].contains(x, y) {
                    self.confirm_current_uninstall_candidate();
                } else if choices[1].contains(x, y) {
                    self.delete_current_uninstall_source();
                }
            }
            Phase::UninstallModeAsk => {
                let choices = self.choice_rects();
                if choices[0].contains(x, y) {
                    // Delete non-uninstall targets once, then let the user
                    // place a crosshair for every detected application.
                    self.begin_manual_uninstall_selection();
                } else if choices[1].contains(x, y) {
                    // The explicit "all" choice is the only non-manual bulk
                    // path.  It still launches BCU once with deduplicated app
                    // ids and never touches resolved executable paths.
                    self.confirmed_uninstall_candidates = self.uninstall_candidates.clone();
                    self.queue_ordinary_targets();
                    let shortcut_sources: Vec<PathBuf> = self
                        .uninstall_candidates
                        .iter()
                        .filter(|candidate| is_shortcut_file(&candidate.source))
                        .map(|candidate| candidate.source.clone())
                        .collect();
                    for source in shortcut_sources {
                        self.queue_delete_path(source);
                    }
                    self.finish_uninstall_selection();
                }
            }
            Phase::UninstallUnavailable => {
                let choices = self.choice_rects();
                if choices[0].contains(x, y) {
                    self.begin_direct_batch_recycle();
                } else if choices[1].contains(x, y) {
                    unsafe {
                        let _ = DestroyWindow(self.hwnd);
                    }
                }
            }
            Phase::Elevate => {
                let modal = self.modal_rect();
                if (RectI {
                    x: modal.x + 90,
                    y: modal.y + 105,
                    w: 140,
                    h: 45,
                })
                .contains(x, y)
                {
                    match request_elevation(&self.delete_targets) {
                        Ok(()) => unsafe {
                            let _ = DestroyWindow(self.hwnd);
                        },
                        Err(error) => {
                            self.error = Some(format!("无法请求管理员权限：{error}"));
                            self.enter(Phase::Error);
                        }
                    }
                } else if (RectI {
                    x: modal.x + 270,
                    y: modal.y + 105,
                    w: 140,
                    h: 45,
                })
                .contains(x, y)
                {
                    unsafe {
                        let _ = DestroyWindow(self.hwnd);
                    }
                }
            }
            Phase::Error => unsafe {
                let _ = DestroyWindow(self.hwnd);
            },
            _ => {}
        }
    }
    fn choice_rects(&self) -> [RectI; 2] {
        let point = self.point.as_ref();
        let (mx, my, mw, mh) = if let Some(sprite) = point {
            let (x, y) = self.monster_position(sprite);
            (x, y, sprite.width, sprite.height)
        } else {
            (self.target_position.0, self.target_position.1, 250, 250)
        };
        let group_width = self.px(297);
        let margin = self.px(12);
        let button_x = (mx + mw / 2 - group_width / 2)
            .clamp(margin, self.width - group_width - margin);
        let below = my + mh + self.px(68) <= self.height - margin;
        let button_y = if below {
            my + mh + self.px(16)
        } else {
            (my - self.px(68)).max(margin)
        };
        [
            RectI {
                x: button_x,
                y: button_y,
                w: self.px(92),
                h: self.px(52),
            },
            RectI {
                x: button_x + self.px(107),
                y: button_y,
                w: self.px(190),
                h: self.px(52),
            },
        ]
    }
    fn modal_rect(&self) -> RectI {
        RectI {
            x: self.width / 2 - 250,
            y: self.height / 2 - 85,
            w: 500,
            h: 170,
        }
    }
    fn tick(&mut self) {
        match self.phase {
            Phase::FadeOut if self.elapsed() >= 0.5 => {
                self.audio.play_loop();
                self.ensure_point();
                self.enter(Phase::Walk);
            }
            // Match the original Qt sequence: the question SFX starts exactly
            // when the four-frame pointing animation starts and continues into
            // the question bubble; it is not restarted when the bubble appears.
            Phase::Walk if self.elapsed() >= 4.5 => {
                self.audio.play("monster_talk");
                self.enter(Phase::Point);
            }
            Phase::Point if self.elapsed() >= 0.5 => {
                self.ensure_kick_sequence();
                if self.current_uninstall_candidate < self.uninstall_candidates.len()
                    && !self.uninstall_markers.is_empty()
                {
                    self.enter(Phase::UninstallAsk);
                } else {
                    self.enter(Phase::Ask);
                }
            }
            Phase::DetectUninstall => {
                let result = self.uninstall_probe.as_ref().and_then(|receiver| match receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => Some(Vec::new()),
                });
                if let Some(result) = result {
                    self.uninstall_probe = None;
                    self.uninstall_candidates = result
                        .into_iter()
                        .filter_map(|(source, probe)| match probe {
                            BcuProbe::Candidate(mut candidate) => {
                                candidate.source = source;
                                Some(candidate)
                            }
                            BcuProbe::ExecutableWithoutUninstaller
                            | BcuProbe::NotExecutableShortcut => None,
                        })
                        .collect();
                    self.current_uninstall_candidate = 0;
                    self.confirmed_uninstall_candidates.clear();
                    self.uninstall_markers.clear();
                    if self.uninstall_candidates.is_empty() {
                        // A batch that has no verified uninstall association
                        // is just a normal batch deletion: never show a
                        // spurious second question for text files/folders.
                        self.begin_direct_batch_recycle();
                    } else if self.uninstall_candidates.len() == 1 {
                        // The initial target click already provides the
                        // location for a one-item operation.  Requiring a
                        // second crosshair here made the completed probe look
                        // like a stalled overlay.
                        self.begin_single_uninstall_flow();
                    } else {
                        self.enter(Phase::UninstallModeAsk);
                    }
                }
            }
            Phase::Kick => {
                if self.frame() >= 5 && !self.deletion_started {
                    self.trigger_delete();
                }
                if self.phase == Phase::Kick && self.elapsed() >= 15.0 / FRAME_RATE {
                    self.enter(Phase::Leo);
                }
            }
            Phase::Leo if self.elapsed() >= 15.0 / FRAME_RATE => match self.after_kick {
                AfterKick::FlyAway => self.enter(Phase::Fly),
                AfterKick::AskSingleUninstall => self.enter(Phase::UninstallAsk),
                AfterKick::SelectUninstallTarget => self.enter_uninstall_target_selection(),
            },
            Phase::Fly if self.elapsed() >= 2.0 => unsafe {
                let _ = DestroyWindow(self.hwnd);
            },
            _ => {}
        }
        self.render();
    }
    fn begin_uninstall_probe(&mut self) -> bool {
        let is_multi_target = self.targets.len() > 1;
        let probe_targets: Vec<(PathBuf, PathBuf)> = self
            .targets
            .iter()
            .filter(|target| {
                if is_multi_target {
                    batch_uninstall_target_matches_config(target)
                } else {
                    uninstall_target_matches_config(target)
                }
            })
            .filter_map(|target| uninstall_probe_target(target).map(|executable| (target.clone(), executable)))
            .collect();
        if probe_targets.is_empty() {
            return false;
        }
        let bridge = bcu_bridge_path();
        // The overlay never probes folders/normal files.  Every tuple keeps
        // the original selected source next to its verified executable.
        if !bridge.is_file() {
            return false;
        }
        let index = uninstall_index_path();
        let (sender, receiver) = sync_channel(1);
        thread::spawn(move || {
            let result = probe_targets
                .into_iter()
                .map(|(source, executable)| {
                    let probe = probe_bcu_executable(&bridge, &executable, &index);
                    (source, probe)
                })
                .collect();
            let _ = sender.send(result);
        });
        self.uninstall_probe = Some(receiver);
        true
    }
    fn queue_delete_path(&mut self, path: PathBuf) {
        if !self.delete_targets.iter().any(|existing| existing == &path) {
            self.delete_targets.push(path);
        }
    }
    fn queue_delete_paths(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            self.queue_delete_path(path);
        }
    }
    /// A multi-selection with no verified uninstall candidate is one ordinary
    /// recycle operation over every originally selected path.  In particular,
    /// no resolved shortcut target is substituted into this list.
    fn begin_direct_batch_recycle(&mut self) {
        self.queue_delete_paths(self.targets.clone());
        self.start_delete_sequence(AfterKick::FlyAway);
    }
    fn start_delete_sequence(&mut self, after_kick: AfterKick) {
        self.after_kick = after_kick;
        self.deletion_started = false;
        self.explosion_started = None;
        self.ensure_kick_sequence();
        self.enter(Phase::Kick);
    }
    fn begin_manual_uninstall_selection(&mut self) {
        self.queue_ordinary_targets();
        if self.delete_targets.is_empty() {
            self.enter_uninstall_target_selection();
        } else {
            self.start_delete_sequence(AfterKick::SelectUninstallTarget);
        }
    }
    fn begin_single_uninstall_flow(&mut self) {
        self.queue_ordinary_targets();
        if self.delete_targets.is_empty() {
            self.enter(Phase::UninstallAsk);
        } else {
            self.start_delete_sequence(AfterKick::AskSingleUninstall);
        }
    }
    fn queue_ordinary_targets(&mut self) {
        let candidate_sources: std::collections::HashSet<PathBuf> = self
            .uninstall_candidates
            .iter()
            .map(|candidate| candidate.source.clone())
            .collect();
        let ordinary_targets = self
            .targets
            .iter()
            .filter(|target| !candidate_sources.contains(*target))
            .cloned()
            .collect();
        self.queue_delete_paths(ordinary_targets);
    }
    fn enter_uninstall_target_selection(&mut self) {
        self.current_uninstall_candidate = self.current_uninstall_candidate.min(self.uninstall_candidates.len());
        self.enter(Phase::SelectUninstallTarget);
    }
    fn confirm_current_uninstall_candidate(&mut self) {
        if let Some(candidate) = self.uninstall_candidates.get(self.current_uninstall_candidate).cloned() {
            // The application uninstaller owns an `.exe`; a selected shortcut
            // is still recycled afterwards so it cannot become stale.
            if is_shortcut_file(&candidate.source) {
                self.queue_delete_path(candidate.source.clone());
            }
            self.confirmed_uninstall_candidates.push(candidate);
        }
        self.advance_uninstall_selection();
    }
    fn delete_current_uninstall_source(&mut self) {
        if let Some(candidate) = self.uninstall_candidates.get(self.current_uninstall_candidate) {
            self.queue_delete_path(candidate.source.clone());
        }
        self.advance_uninstall_selection();
    }
    fn advance_uninstall_selection(&mut self) {
        self.current_uninstall_candidate += 1;
        if self.current_uninstall_candidate < self.uninstall_candidates.len() {
            self.enter_uninstall_target_selection();
        } else {
            self.finish_uninstall_selection();
        }
    }
    fn finish_uninstall_selection(&mut self) {
        if !self.confirmed_uninstall_candidates.is_empty() {
            if let Err(error) = launch_bcu_uninstaller(
                &self.confirmed_uninstall_candidates,
                uninstall_silent_execution_enabled(),
            ) {
                self.error = Some(format!("无法启动软件的官方卸载程序：{error}"));
                self.enter(Phase::Error);
                return;
            }
        }
        if self.delete_targets.is_empty() {
            self.enter(Phase::Fly);
        } else {
            self.start_delete_sequence(AfterKick::FlyAway);
        }
    }
    fn trigger_delete(&mut self) {
        self.deletion_started = true;
        self.explosion_started = Some(Instant::now());
        self.explosion_positions = self.explosion_positions_for_delete();
        self.audio.play("monster_boom");
        match recycle(&self.delete_targets, false) {
            Ok(()) => self.delete_targets.clear(),
            Err(error) if error.code() == ERROR_ACCESS_DENIED.into() => {
                self.enter(Phase::Elevate);
            }
            Err(error) => {
                self.error = Some(format!("删除失败：{error}"));
                self.enter(Phase::Error);
            }
        }
    }
    fn explosion_positions_for_delete(&self) -> Vec<(i32, i32)> {
        if !self.uninstall_markers.is_empty() {
            return self.uninstall_markers.clone();
        }
        // Explorer does not expose icon coordinates in a shell-verb command.
        // Keep separate explosion instances instead of spawning overlays; fan
        // them around the user-selected point for multi-item operations.
        let count = self.delete_targets.len().max(1);
        (0..count)
            .map(|index| {
                if count == 1 {
                    self.target_position
                } else {
                    let angle = std::f32::consts::TAU * index as f32 / count as f32;
                    let radius = self.px(42 + ((index % 3) as i32 * 18)) as f32;
                    (
                        self.target_position.0 + (angle.cos() * radius) as i32,
                        self.target_position.1 + (angle.sin() * radius) as i32,
                    )
                }
            })
            .collect()
    }
    fn clear(&mut self) {
        self.pixels.fill(0);
    }
    fn blend(&mut self, x: i32, y: i32, r: u8, g: u8, b: u8, alpha: u8) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height || alpha == 0 {
            return;
        }
        let index = ((y * self.width + x) * 4) as usize;
        let a = alpha as u32;
        let inverse = 255 - a;
        self.pixels[index] = ((b as u32 * a + self.pixels[index] as u32 * inverse) / 255) as u8;
        self.pixels[index + 1] =
            ((g as u32 * a + self.pixels[index + 1] as u32 * inverse) / 255) as u8;
        self.pixels[index + 2] =
            ((r as u32 * a + self.pixels[index + 2] as u32 * inverse) / 255) as u8;
        self.pixels[index + 3] = (a + self.pixels[index + 3] as u32 * inverse / 255) as u8;
    }
    fn image(&mut self, image: &RgbaImage, rect: RectI, opacity: u8, mirrored: bool) {
        if rect.w <= 0 || rect.h <= 0 {
            return;
        }
        let source_w = image.width() as i32;
        let source_h = image.height() as i32;
        for dy in 0..rect.h {
            let sy = (dy * source_h / rect.h).clamp(0, source_h - 1) as u32;
            for dx in 0..rect.w {
                let raw_x = (dx * source_w / rect.w).clamp(0, source_w - 1);
                let sx = if mirrored {
                    source_w - 1 - raw_x
                } else {
                    raw_x
                } as u32;
                let pixel = image.get_pixel(sx, sy).0;
                self.blend(
                    rect.x + dx,
                    rect.y + dy,
                    pixel[0],
                    pixel[1],
                    pixel[2],
                    ((pixel[3] as u16 * opacity as u16) / 255) as u8,
                );
            }
        }
    }
    fn rect(&mut self, rect: RectI, color: (u8, u8, u8, u8), radius: i32) {
        for y in rect.y - 1..rect.y + rect.h + 1 {
            for x in rect.x - 1..rect.x + rect.w + 1 {
                let coverage = (0.5 - Self::rounded_distance(rect, radius, x as f32 + 0.5, y as f32 + 0.5))
                    .clamp(0.0, 1.0);
                if coverage > 0.0 {
                    self.blend(
                        x,
                        y,
                        color.0,
                        color.1,
                        color.2,
                        (color.3 as f32 * coverage) as u8,
                    );
                }
            }
        }
    }
    fn rounded_distance(rect: RectI, radius: i32, x: f32, y: f32) -> f32 {
        let radius = radius.clamp(0, rect.w.min(rect.h) / 2) as f32;
        let center_x = rect.x as f32 + rect.w as f32 / 2.0;
        let center_y = rect.y as f32 + rect.h as f32 / 2.0;
        let qx = (x - center_x).abs() - (rect.w as f32 / 2.0 - radius);
        let qy = (y - center_y).abs() - (rect.h as f32 / 2.0 - radius);
        let outside = qx.max(0.0).hypot(qy.max(0.0));
        outside + qx.max(qy).min(0.0) - radius
    }
    fn shadow(&mut self, rect: RectI, radius: i32, offset_y: i32, blur: i32, alpha: u8) {
        let shifted = RectI {
            y: rect.y + offset_y,
            ..rect
        };
        let extent = blur * 2;
        for y in shifted.y - extent..shifted.y + shifted.h + extent {
            for x in shifted.x - extent..shifted.x + shifted.w + extent {
                let distance = Self::rounded_distance(shifted, radius, x as f32 + 0.5, y as f32 + 0.5);
                let opacity = if distance <= 0.0 {
                    1.0
                } else {
                    (1.0 - distance / blur.max(1) as f32).clamp(0.0, 1.0).powi(2)
                };
                if opacity > 0.0 {
                    self.blend(x, y, 0, 0, 0, (alpha as f32 * opacity) as u8);
                }
            }
        }
    }
    fn triangle(&mut self, a: (i32, i32), b: (i32, i32), c: (i32, i32), color: (u8, u8, u8, u8)) {
        let min_x = a.0.min(b.0).min(c.0) - 1;
        let max_x = a.0.max(b.0).max(c.0) + 1;
        let min_y = a.1.min(b.1).min(c.1) - 1;
        let max_y = a.1.max(b.1).max(c.1) + 1;
        let edge = |from: (i32, i32), to: (i32, i32), point: (f32, f32)| {
            (to.0 - from.0) as f32 * (point.1 - from.1 as f32)
                - (to.1 - from.1) as f32 * (point.0 - from.0 as f32)
        };
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let mut inside = 0u8;
                for (sx, sy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                    let point = (x as f32 + sx, y as f32 + sy);
                    let e1 = edge(a, b, point);
                    let e2 = edge(b, c, point);
                    let e3 = edge(c, a, point);
                    if !((e1 < 0.0 || e2 < 0.0 || e3 < 0.0)
                        && (e1 > 0.0 || e2 > 0.0 || e3 > 0.0))
                    {
                        inside += 1;
                    }
                }
                if inside > 0 {
                    self.blend(x, y, color.0, color.1, color.2, color.3 * inside / 4);
                }
            }
        }
    }
    fn card(&mut self, rect: RectI, radius: i32, shadow_offset: i32, shadow_blur: i32) {
        self.shadow(rect, radius, shadow_offset, shadow_blur, 34);
        self.rect(rect, (229, 229, 234, 215), radius);
        let inset = self.px(1);
        self.rect(
            RectI {
                x: rect.x + inset,
                y: rect.y + inset,
                w: rect.w - inset * 2,
                h: rect.h - inset * 2,
            },
            (255, 255, 255, 240),
            (radius - inset).max(1),
        );
    }
    fn text(&mut self, text: &str, rect: RectI, size: i32, color: (u8, u8, u8, u8)) {
        unsafe {
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: rect.w,
                    biHeight: -rect.h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = null_mut();
            let Ok(bitmap) = CreateDIBSection(None, &mut info, DIB_RGB_COLORS, &mut bits, None, 0)
            else {
                return;
            };
            let dc = CreateCompatibleDC(None);
            let old = SelectObject(dc, bitmap.into());
            let face = wide("Segoe UI");
            let font = CreateFontW(
                -size,
                0,
                0,
                0,
                600,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_TT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                DEFAULT_PITCH.0 as u32 | FONT_FAMILY(FF_DONTCARE.0).0 as u32,
                PCWSTR(face.as_ptr()),
            );
            let previous_font = SelectObject(dc, font.into());
            let _ = SetBkMode(dc, TRANSPARENT);
            let _ = SetTextColor(dc, COLORREF(0x00ff_ffff));
            let mut words = wide(text);
            let mut bounds = RECT {
                left: 0,
                top: 0,
                right: rect.w,
                bottom: rect.h,
            };
            let format = if text.contains('\n') {
                DT_CENTER | DT_VCENTER | DT_WORDBREAK
            } else {
                DT_CENTER | DT_VCENTER | DT_SINGLELINE
            };
            let _ = DrawTextW(
                dc,
                &mut words,
                &mut bounds,
                format,
            );
            let raw = slice::from_raw_parts(bits.cast::<u8>(), (rect.w * rect.h * 4) as usize);
            for y in 0..rect.h {
                for x in 0..rect.w {
                    let index = ((y * rect.w + x) * 4) as usize;
                    let coverage = raw[index].max(raw[index + 1]).max(raw[index + 2]);
                    if coverage > 0 {
                        self.blend(
                            rect.x + x,
                            rect.y + y,
                            color.0,
                            color.1,
                            color.2,
                            ((coverage as u16 * color.3 as u16) / 255) as u8,
                        );
                    }
                }
            }
            let _ = SelectObject(dc, previous_font);
            let _ = DeleteObject(font.into());
            let _ = SelectObject(dc, old);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
        }
    }
    fn draw_selection(&mut self, opacity: f32) {
        if let Some(background) = self.background.clone() {
            let scale = (self.width as f32 / background.width() as f32)
                .max(self.height as f32 / background.height() as f32);
            let w = (background.width() as f32 * scale).round() as i32;
            let h = (background.height() as f32 * scale).round() as i32;
            self.image(
                &background,
                RectI {
                    x: (self.width - w) / 2,
                    y: (self.height - h) / 2,
                    w,
                    h,
                },
                (opacity * 255.0) as u8,
                false,
            );
        } else {
            self.rect(
                RectI {
                    x: 0,
                    y: 0,
                    w: self.width,
                    h: self.height,
                },
                (0, 0, 0, (opacity * 160.0) as u8),
                0,
            );
        }
        let content_alpha = ((opacity / 0.35).min(1.0) * 255.0) as u8;
        let prompt = if self.phase == Phase::SelectUninstallTarget {
            self.uninstall_candidates
                .get(self.current_uninstall_candidate)
                .map(|candidate| {
                    format!(
                        "请用准星标记要卸载的：{}（{}/{}）",
                        candidate.display_name,
                        self.current_uninstall_candidate + 1,
                        self.uninstall_candidates.len()
                    )
                })
                .unwrap_or_else(|| "请选择要卸载的软件目标".to_owned())
        } else {
            "请选择你要摧毁的文件".to_owned()
        };
        self.text(
            &prompt,
            RectI {
                x: self.width / 2 - 300,
                y: self.height / 2 - 28,
                w: 600,
                h: 56,
            },
            30,
            (255, 255, 255, content_alpha),
        );
    }
    fn draw_uninstall_markers(&mut self) {
        let markers = self.uninstall_markers.clone();
        let newest = markers.len().saturating_sub(1);
        for (index, &(x, y)) in markers.iter().enumerate() {
            // The newly selected point grows in over the pointing animation,
            // then remains fixed while the user answers its uninstall prompt
            // and selects the remaining targets.
            let scale = if index == newest && self.phase == Phase::Point {
                (self.elapsed() / 0.20).clamp(0.15, 1.0)
            } else {
                1.0
            };
            let arm = (self.px(15) as f32 * scale).round().max(2.0) as i32;
            let stroke = (self.px(3) as f32 * scale).round().max(1.0) as i32;
            let alpha = (235.0 * scale) as u8;
            // A translucent mask plus a fixed red X makes each manually
            // assigned uninstall target visible while more are selected.
            self.rect(
                RectI { x: x - arm - stroke, y: y - arm - stroke, w: (arm + stroke) * 2, h: (arm + stroke) * 2 },
                (225, 45, 45, (52.0 * scale) as u8),
                self.px(18),
            );
            for offset in -stroke..=stroke {
                for step in -arm..=arm {
                    self.blend(x + step, y + step + offset, 235, 45, 45, alpha);
                    self.blend(x + step, y - step + offset, 235, 45, 45, alpha);
                }
            }
        }
    }
    fn draw_bubble_size(
        &mut self,
        monster_width: i32,
        monster_height: i32,
        position: (i32, i32),
        message: &str,
        first_choice: &str,
        second_choice: &str,
    ) {
        let (mx, my) = position;
        // These are the original Qt layout dimensions in logical pixels. The
        // per-monitor scale is applied once here, together with the monster.
        let bubble_w = self.px(220);
        let bubble_h = if message.contains('\n') {
            self.px(80)
        } else {
            self.px(64)
        };
        let margin = self.px(28);
        let tail = self.px(15);
        let raw_x = if self.points_left {
            mx + monster_width + self.px(18)
        } else {
            mx - bubble_w - self.px(18)
        };
        let x = raw_x.clamp(margin, self.width - bubble_w - margin);
        let side_y = my + monster_height * 30 / 100 - bubble_h / 2;
        let below = side_y < margin;
        let y = if below {
            my + monster_height + self.px(20)
        } else {
            side_y
        }
        .clamp(margin, self.height - bubble_h - margin);
        let bubble = RectI {
            x,
            y,
            w: bubble_w,
            h: bubble_h,
        };
        let target_y = (my + monster_height / 2).clamp(y + tail, y + bubble_h - tail);
        let bubble_tail = if below {
            ((x + bubble_w / 2 - tail, y), (x + bubble_w / 2 + tail, y), (x + bubble_w / 2, y - tail))
        } else if self.points_left {
            ((x, target_y - tail), (x, target_y + tail), (x - tail, target_y))
        } else {
            ((x + bubble_w, target_y - tail), (x + bubble_w, target_y + tail), (x + bubble_w + tail, target_y))
        };
        let shadow_tail = (
            (bubble_tail.0 .0, bubble_tail.0 .1 + self.px(7)),
            (bubble_tail.1 .0, bubble_tail.1 .1 + self.px(7)),
            (bubble_tail.2 .0, bubble_tail.2 .1 + self.px(7)),
        );
        self.triangle(shadow_tail.0, shadow_tail.1, shadow_tail.2, (0, 0, 0, 28));
        self.triangle(bubble_tail.0, bubble_tail.1, bubble_tail.2, (255, 255, 255, 240));
        self.card(bubble, self.px(20), self.px(8), self.px(14));
        self.text(message, bubble, self.px(20), (28, 28, 30, 255));
        if first_choice.is_empty() && second_choice.is_empty() {
            return;
        }
        for rect in self.choice_rects() {
            self.card(rect, self.px(18), self.px(5), self.px(10));
        }
        let choices = self.choice_rects();
        self.text(first_choice, choices[0], self.px(16), (28, 28, 30, 255));
        self.text(second_choice, choices[1], self.px(16), (28, 28, 30, 255));
    }
    fn draw_modal(&mut self, is_error: bool) {
        let rect = if is_error {
            RectI {
                x: self.width / 2 - 280,
                y: self.height / 2 - 75,
                w: 560,
                h: 150,
            }
        } else {
            self.modal_rect()
        };
        self.rect(
            RectI {
                x: rect.x,
                y: rect.y + 8,
                ..rect
            },
            (0, 0, 0, 35),
            20,
        );
        self.rect(rect, (255, 255, 255, 240), 20);
        let message = if is_error {
            self.error.clone().unwrap_or_else(|| "删除失败".to_owned())
        } else {
            "该文件需要管理员权限，继续吗？".to_owned()
        };
        self.text(
            &message,
            RectI {
                x: rect.x + 20,
                y: rect.y + 20,
                w: rect.w - 40,
                h: 70,
            },
            18,
            if is_error {
                (180, 30, 30, 255)
            } else {
                (28, 28, 30, 255)
            },
        );
        if !is_error {
            let yes = RectI {
                x: rect.x + 90,
                y: rect.y + 105,
                w: 140,
                h: 45,
            };
            let no = RectI {
                x: rect.x + 270,
                y: rect.y + 105,
                w: 140,
                h: 45,
            };
            self.rect(yes, (255, 255, 255, 240), 18);
            self.rect(no, (255, 255, 255, 240), 18);
            self.text("继续", yes, 16, (28, 28, 30, 255));
            self.text("取消", no, 16, (28, 28, 30, 255));
        }
    }
    fn render(&mut self) {
        self.clear();
        match self.phase {
            Phase::Select => self.draw_selection((self.elapsed() / 0.8).min(1.0) * 0.35),
            Phase::SelectUninstallTarget => self.draw_selection(0.35),
            Phase::FadeOut => self.draw_selection((1.0 - self.elapsed() / 0.5).max(0.0) * 0.35),
            Phase::Walk => {
                if let Some(sprite) = self.walk.as_ref() {
                    let progress = (self.elapsed() / 4.5).min(1.0);
                    let end = self.monster_position(sprite);
                    let start_x = if self.points_left {
                        self.width
                    } else {
                        -sprite.width
                    };
                    let eased = 1.0 - (1.0 - progress) * (1.0 - progress);
                    let image = sprite.frames[self.frame() % sprite.frames.len()].clone();
                    self.image(
                        &image,
                        RectI {
                            x: (start_x as f32 + (end.0 - start_x) as f32 * eased) as i32,
                            y: end.1,
                            w: sprite.width,
                            h: sprite.height,
                        },
                        255,
                        self.points_left,
                    );
                }
            }
            Phase::Point => {
                if let Some(sprite) = self.point.as_ref() {
                    let position = self.monster_position(sprite);
                    let image =
                        sprite.frames[(11 + self.frame().min(3)) % sprite.frames.len()].clone();
                    self.image(
                        &image,
                        RectI {
                            x: position.0,
                            y: position.1,
                            w: sprite.width,
                            h: sprite.height,
                        },
                        255,
                        self.points_left,
                    );
                }
            }
            Phase::Ask
            | Phase::DetectUninstall
            | Phase::UninstallModeAsk
            | Phase::UninstallAsk
            | Phase::UninstallUnavailable => {
                if let Some(sprite) = self.point.as_ref() {
                    let position = self.monster_position(sprite);
                    let (w, h) = (sprite.width, sprite.height);
                    let image = sprite.frames[14 % sprite.frames.len()].clone();
                    self.image(
                        &image,
                        RectI {
                            x: position.0,
                            y: position.1,
                            w,
                            h,
                        },
                        255,
                        self.points_left,
                    );
                    let (message, first_choice, second_choice) = if self.phase == Phase::UninstallModeAsk {
                        ("检测到多个可卸载软件，\n要怎么处理？", "逐一指定", "全部卸载")
                    } else if self.phase == Phase::UninstallAsk {
                        if uninstall_silent_execution_enabled() {
                            ("居然是软件，\n要悄悄卸载吗？", "静默卸载", "就删除快捷方式好啦")
                        } else {
                            ("居然是软件，\n需要卸载吗？", "卸载", "就删除快捷方式好啦")
                        }
                    } else if self.phase == Phase::DetectUninstall {
                        ("让我查查这是什么……", "", "")
                    } else if self.phase == Phase::UninstallUnavailable {
                        ("这个软件只能删除，\n无法卸载", "只删除", "取消")
                    } else if self.targets.len() > 1 {
                        ("喂，是这些吗？", "是的", "嘤嘤嘤就是这些")
                    } else {
                        ("喂，是这个吗？", "是的", "嘤嘤嘤就是这个")
                    };
                    self.draw_bubble_size(w, h, position, message, first_choice, second_choice);
                }
            }
            Phase::Kick => {
                if let Some(sprite) = self.kick.as_ref() {
                    let position = self.monster_position(sprite);
                    let image = sprite.frames[self.frame().min(14) % sprite.frames.len()].clone();
                    self.image(
                        &image,
                        RectI {
                            x: position.0,
                            y: position.1,
                            w: sprite.width,
                            h: sprite.height,
                        },
                        255,
                        self.points_left,
                    );
                }
                self.draw_explosion();
            }
            Phase::Leo => {
                if let Some(sprite) = self.leo.as_ref() {
                    let position = self.monster_position(sprite);
                    let image = sprite.frames[self.frame().min(14) % sprite.frames.len()].clone();
                    self.image(
                        &image,
                        RectI {
                            x: position.0,
                            y: position.1,
                            w: sprite.width,
                            h: sprite.height,
                        },
                        255,
                        self.points_left,
                    );
                }
            }
            Phase::Fly => {
                if let Some(sprite) = self.fly.as_ref() {
                    let progress = (self.elapsed() / 2.0).min(1.0);
                    let start = self.monster_position(sprite);
                    let end_x = if self.points_left {
                        -sprite.width - 200
                    } else {
                        self.width + 200
                    };
                    let image = sprite.frames[self.frame() % sprite.frames.len()].clone();
                    self.image(
                        &image,
                        RectI {
                            x: (start.0 as f32 + (end_x - start.0) as f32 * progress * progress)
                                as i32,
                            y: start.1,
                            w: sprite.width,
                            h: sprite.height,
                        },
                        255,
                        self.points_left,
                    );
                }
            }
            Phase::Elevate => self.draw_modal(false),
            Phase::Error => self.draw_modal(true),
        }
        if !self.uninstall_markers.is_empty()
            && matches!(
                self.phase,
                Phase::SelectUninstallTarget | Phase::Point | Phase::UninstallAsk
            )
        {
            self.draw_uninstall_markers();
        }
        unsafe {
            let target = slice::from_raw_parts_mut(self.surface.bits, self.pixels.len());
            target.copy_from_slice(&self.pixels);
            let destination = POINT {
                x: self.screen_x,
                y: self.screen_y,
            };
            let size = SIZE {
                cx: self.width,
                cy: self.height,
            };
            let source = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: 0,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&destination),
                Some(&size),
                Some(self.surface.dc),
                Some(&source),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
        }
    }
    fn draw_explosion(&mut self) {
        if let (Some(started), Some(sprite)) = (self.explosion_started, self.explosion.as_ref()) {
            let elapsed = started.elapsed().as_secs_f32();
            if elapsed <= 15.0 / FRAME_RATE {
                let image =
                    sprite.frames[(elapsed * FRAME_RATE) as usize % sprite.frames.len()].clone();
                let (width, height) = (sprite.width, sprite.height);
                for &(x, y) in &self.explosion_positions.clone() {
                    self.image(
                        &image,
                        RectI {
                            x: x - width / 2,
                            y: y - height / 2 - 40,
                            w: width,
                            h: height,
                        },
                        255,
                        false,
                    );
                }
            }
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lparam.0 as *const CREATESTRUCTW);
        let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        return LRESULT(1);
    }
    let pointer = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWLP_USERDATA)
        as *mut OverlayApp;
    if pointer.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let app = &mut *pointer;
    match message {
        WM_TIMER if wparam.0 == TICK_ID => {
            app.tick();
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            app.click(signed_low_word(lparam.0), signed_high_word(lparam.0));
            LRESULT(0)
        }
        WM_HOTKEY if wparam.0 as i32 == ESC_HOTKEY_ID => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 == 0x1b => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_SETCURSOR if matches!(app.phase, Phase::Select | Phase::SelectUninstallTarget) => {
            let _ = SetCursor(Some(app.target_cursor));
            LRESULT(1)
        }
        WM_DESTROY => {
            let _ = UnregisterHotKey(Some(hwnd), ESC_HOTKEY_ID);
            if !app.target_cursor.is_invalid() {
                let _ = DestroyCursor(app.target_cursor);
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn run_overlay(targets: Vec<PathBuf>) -> Result<()> {
    unsafe {
        // Keep the bitmap, cursor events, and virtual-screen metrics in the
        // same physical-pixel coordinate space on mixed-DPI displays.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let class = wide("MonsterDeleterOverlay");
        let window_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassW(&window_class);
        let app = Box::into_raw(Box::new(
            OverlayApp::new(targets).ok_or_else(windows::core::Error::from_thread)?,
        ));
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            PCWSTR(class.as_ptr()),
            PCWSTR(class.as_ptr()),
            WS_POPUP,
            (*app).screen_x,
            (*app).screen_y,
            (*app).width,
            (*app).height,
            None,
            None,
            None,
            Some(app.cast()),
        )?;
        (*app).hwnd = hwnd;
        (*app).ui_scale = (GetDpiForWindow(hwnd) as f32 / 96.0).clamp(1.0, 2.0);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            (*app).screen_x,
            (*app).screen_y,
            (*app).width,
            (*app).height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = RegisterHotKey(Some(hwnd), ESC_HOTKEY_ID, HOT_KEY_MODIFIERS(0), 0x1b);
        (*app).render();
        let _ = SetTimer(Some(hwnd), TICK_ID, 16, None);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        drop(Box::from_raw(app));
    }
    Ok(())
}

fn signed_low_word(value: isize) -> i32 {
    (value as i16) as i32
}
fn signed_high_word(value: isize) -> i32 {
    ((value >> 16) as i16) as i32
}

// SHFileOperation is a legacy shell API. Its 0x78 return value is the
// shell-specific DE_ACCESSDENIEDSRC status (not Win32 ERROR_CALL_NOT_IMPLEMENTED
// despite sharing the same numeric value). Normalize it so the regular delete
// flow can offer the existing UAC retry instead of showing a misleading error.
const DE_ACCESSDENIEDSRC: i32 = 0x78;

fn recycle_failure_code(status: i32, aborted: bool) -> Option<WIN32_ERROR> {
    if status == 0 {
        return aborted.then_some(ERROR_CANCELLED);
    }
    Some(if status == DE_ACCESSDENIEDSRC {
        ERROR_ACCESS_DENIED
    } else {
        WIN32_ERROR(status as u32)
    })
}

fn recycle(targets: &[PathBuf], elevated: bool) -> Result<()> {
    if targets.is_empty() {
        return Ok(());
    }
    // SHFileOperation accepts a double-NUL-terminated sequence of paths.  One
    // operation gives a selected batch one recycle action instead of starting
    // one overlay/process per Explorer item.
    let mut from = Vec::new();
    for target in targets {
        let target_wide = wide(target.as_os_str());
        unsafe {
            if GetFileAttributesW(PCWSTR(target_wide.as_ptr())) == u32::MAX {
                return Err(windows::core::Error::from_thread());
            }
        }
        from.extend_from_slice(&target_wide);
    }
    from.push(0);
    unsafe {
        let mut operation = SHFILEOPSTRUCTW {
            wFunc: FO_DELETE,
            pFrom: PCWSTR(from.as_ptr()),
            // Do not let the legacy Shell API follow a .lnk's connected
            // target. Only the exact parsing path supplied by the context
            // menu may be moved to the Recycle Bin.
            fFlags: (FOF_ALLOWUNDO | FOF_NO_CONNECTED_ELEMENTS | FOF_NOCONFIRMATION | FOF_NOERRORUI).0 as u16,
            ..Default::default()
        };
        let status = SHFileOperationW(&mut operation);
        if let Some(code) = recycle_failure_code(status, operation.fAnyOperationsAborted.as_bool()) {
            return Err(windows::core::Error::new(
                code.into(),
                if elevated {
                    "elevated recycle failed"
                } else {
                    "recycle failed"
                },
            ));
        }
    }
    Ok(())
}

fn request_elevation(targets: &[PathBuf]) -> Result<()> {
    if targets.is_empty() {
        return Ok(());
    }
    let exe = env::current_exe().map_err(|_| windows::core::Error::from_thread())?;
    let parameters = std::iter::once("--elevated-delete".to_owned())
        .chain(targets.iter().map(|target| format!("\"{}\"", target.display())))
        .collect::<Vec<_>>()
        .join(" ");
    unsafe {
        let result = ShellExecuteW(
            Some(HWND::default()),
            PCWSTR(wide("runas").as_ptr()),
            PCWSTR(wide(exe.as_os_str()).as_ptr()),
            PCWSTR(wide(&parameters).as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if result.0 as isize <= 32 {
            return Err(windows::core::Error::from_thread());
        }
    }
    Ok(())
}

/// Returns an uninstall entry only when a `.lnk` points at an executable that
/// can be unambiguously tied to one of Windows' registered desktop apps.
/// Ambiguous, stale, or incomplete entries intentionally fall back to normal
/// shortcut deletion.
fn installed_app_for_shortcut(shortcut: &Path) -> Option<InstalledApp> {
    if !shortcut
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
    {
        return None;
    }
    let target = shortcut_target(shortcut)?;
    if !target
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return None;
    }
    let target = normalized_path(&target)?;
    let mut matches = Vec::new();
    matches.extend(uninstall_matches_in_hive(
        HKEY_LOCAL_MACHINE,
        KEY_WOW64_64KEY,
        &target,
    ));
    matches.extend(uninstall_matches_in_hive(
        HKEY_LOCAL_MACHINE,
        KEY_WOW64_32KEY,
        &target,
    ));
    matches.extend(uninstall_matches_in_hive(HKEY_CURRENT_USER, KEY_READ, &target));
    matches.sort_by(|left, right| left.uninstall_command.cmp(&right.uninstall_command));
    matches.dedup_by(|left, right| left.uninstall_command == right.uninstall_command);
    (matches.len() == 1).then(|| matches.remove(0))
}

const MAX_SHORTCUT_RESOLUTION_HOPS: usize = 4;

fn shortcut_target(shortcut: &Path) -> Option<PathBuf> {
    if !is_shortcut_file(shortcut) {
        return None;
    }
    let mut current = shortcut.to_path_buf();
    let mut visited = std::collections::HashSet::new();
    for _ in 0..MAX_SHORTCUT_RESOLUTION_HOPS {
        if !current.is_file() || !visited.insert(current.clone()) {
            return None;
        }
        let target = shortcut_target_once(&current)?;
        if is_shortcut_file(&target) {
            current = target;
        } else {
            return Some(target);
        }
    }
    None
}

fn shortcut_target_once(shortcut: &Path) -> Option<PathBuf> {
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let result = (|| {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            let persist: IPersistFile = link.cast().ok()?;
            let shortcut_wide = wide(shortcut.as_os_str());
            persist.Load(PCWSTR(shortcut_wide.as_ptr()), STGM_READ).ok()?;
            let mut target = [0u16; 32_768];
            link.GetPath(&mut target, null_mut(), 0).ok()?;
            let end = target.iter().position(|unit| *unit == 0)?;
            (end > 0).then(|| PathBuf::from(String::from_utf16_lossy(&target[..end])))
        })();
        if initialized {
            CoUninitialize();
        }
        result
    }
}

fn uninstall_matches_in_hive(root: HKEY, view: windows::Win32::System::Registry::REG_SAM_FLAGS, target: &str) -> Vec<InstalledApp> {
    const UNINSTALL_KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
    unsafe {
        let base = wide(UNINSTALL_KEY);
        let mut uninstall_key = HKEY::default();
        if RegOpenKeyExW(root, PCWSTR(base.as_ptr()), None, KEY_READ | view, &mut uninstall_key)
            != WIN32_ERROR(0)
        {
            return Vec::new();
        }
        let mut matches = Vec::new();
        let mut index = 0;
        loop {
            let mut name = [0u16; 512];
            let mut name_len = (name.len() - 1) as u32;
            let status = RegEnumKeyExW(
                uninstall_key,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                None,
                None,
                None,
            );
            if status == ERROR_NO_MORE_ITEMS {
                break;
            }
            index += 1;
            if status != WIN32_ERROR(0) {
                continue;
            }
            let child_name = wide(String::from_utf16_lossy(&name[..name_len as usize]));
            let mut child = HKEY::default();
            if RegOpenKeyExW(uninstall_key, PCWSTR(child_name.as_ptr()), None, KEY_READ, &mut child)
                != WIN32_ERROR(0)
            {
                continue;
            }
            let display_name = registry_string(child, "DisplayName");
            let uninstall_command = registry_string(child, "UninstallString");
            let install_location = registry_string(child, "InstallLocation");
            let display_icon = registry_string(child, "DisplayIcon");
            let _ = RegCloseKey(child);
            let (Some(display_name), Some(uninstall_command)) = (display_name, uninstall_command)
            else {
                continue;
            };
            if display_name.trim().is_empty()
                || parse_uninstall_command(&uninstall_command).is_none()
            {
                continue;
            }
            let in_install_location = install_location
                .as_deref()
                .and_then(normalized_path_string)
                .is_some_and(|location| path_is_within(target, &location));
            let is_display_icon = display_icon
                .as_deref()
                .and_then(display_icon_path)
                .and_then(|path| normalized_path(&path))
                .is_some_and(|path| path == target);
            if in_install_location || is_display_icon {
                matches.push(InstalledApp {
                    display_name,
                    uninstall_command,
                });
            }
        }
        let _ = RegCloseKey(uninstall_key);
        matches
    }
}

unsafe fn registry_string(key: HKEY, name: &str) -> Option<String> {
    let name = wide(name);
    let mut value_type = REG_VALUE_TYPE(0);
    let mut bytes = 0u32;
    if RegQueryValueExW(
        key,
        PCWSTR(name.as_ptr()),
        None,
        Some(&mut value_type),
        None,
        Some(&mut bytes),
    ) != WIN32_ERROR(0)
        || !(value_type == REG_SZ || value_type == REG_EXPAND_SZ)
        || bytes < 2
        || bytes > 65_536
    {
        return None;
    }
    let mut raw = vec![0u8; bytes as usize + 2];
    if RegQueryValueExW(
        key,
        PCWSTR(name.as_ptr()),
        None,
        Some(&mut value_type),
        Some(raw.as_mut_ptr()),
        Some(&mut bytes),
    ) != WIN32_ERROR(0)
    {
        return None;
    }
    let units = slice::from_raw_parts(raw.as_ptr().cast::<u16>(), bytes as usize / 2);
    let end = units.iter().position(|unit| *unit == 0).unwrap_or(units.len());
    Some(expand_environment(&String::from_utf16_lossy(&units[..end])))
}

fn expand_environment(value: &str) -> String {
    unsafe {
        let source = wide(value);
        let required = ExpandEnvironmentStringsW(PCWSTR(source.as_ptr()), None);
        if required == 0 || required > 32_768 {
            return value.to_owned();
        }
        let mut expanded = vec![0u16; required as usize];
        let written = ExpandEnvironmentStringsW(PCWSTR(source.as_ptr()), Some(&mut expanded));
        if written == 0 || written > required {
            return value.to_owned();
        }
        String::from_utf16_lossy(&expanded[..written.saturating_sub(1) as usize])
    }
}

fn normalized_path(path: &Path) -> Option<String> {
    let canonical = std::fs::canonicalize(path).ok()?;
    normalized_path_string(&canonical.to_string_lossy())
}

fn normalized_path_string(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('"').replace('/', "\\");
    if value.is_empty() {
        return None;
    }
    Some(value.trim_end_matches('\\').to_ascii_lowercase())
}

fn path_is_within(path: &str, directory: &str) -> bool {
    path == directory || path.starts_with(&(directory.to_owned() + "\\"))
}

fn display_icon_path(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    let path = if let Some(rest) = value.strip_prefix('"') {
        rest.split_once('"')?.0
    } else {
        value.split_once(',').map_or(value, |(path, _)| path).trim()
    };
    (!path.is_empty()).then(|| PathBuf::from(path))
}

fn parse_uninstall_command(command: &str) -> Option<(String, String)> {
    let command = command.trim();
    let (program, arguments) = if let Some(rest) = command.strip_prefix('"') {
        let (program, arguments) = rest.split_once('"')?;
        (program.trim(), arguments.trim())
    } else {
        let mut parts = command.splitn(2, char::is_whitespace);
        (parts.next()?.trim(), parts.next().unwrap_or("").trim())
    };
    if program.is_empty() || !is_allowed_uninstall_host(program) {
        return None;
    }
    Some((program.to_owned(), arguments.to_owned()))
}

fn is_allowed_uninstall_host(program: &str) -> bool {
    if Path::new(program).is_file() {
        return true;
    }
    matches!(
        program.rsplit(['\\', '/']).next().unwrap_or(program).to_ascii_lowercase().as_str(),
        "msiexec" | "msiexec.exe" | "rundll32" | "rundll32.exe"
    )
}

fn launch_official_uninstaller(command: &str) -> Result<()> {
    let (program, arguments) = parse_uninstall_command(command)
        .ok_or_else(windows::core::Error::from_thread)?;
    let verb = wide("open");
    let program = wide(program);
    let arguments = wide(arguments);
    unsafe {
        // ShellExecuteW receives the executable and arguments separately: no
        // command interpreter is involved, so registry text cannot be treated
        // as a shell expression.
        let result = ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(program.as_ptr()),
            PCWSTR(arguments.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if result.0 as isize <= 32 {
            return Err(windows::core::Error::from_thread());
        }
    }
    Ok(())
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn bcu_bridge_path() -> PathBuf {
    resource_dir()
        .join("assets")
        .join("tools")
        .join("bcu-bridge")
        .join("bcu-bridge.exe")
}

fn probe_bcu_executable(bridge: &Path, executable: &Path, index: &Path) -> BcuProbe {
    if !bridge.is_file() || !executable.is_file() || !is_executable_file(executable) {
        return BcuProbe::NotExecutableShortcut;
    }
    let Ok(mut child) = Command::new(bridge)
        .arg("resolve")
        .arg(executable)
        .arg(index)
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return BcuProbe::NotExecutableShortcut;
    };
    // Registry/MSI enumeration can be slow, but an interactive overlay must
    // never spin forever if a third-party uninstall entry blocks the bridge.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return BcuProbe::NotExecutableShortcut;
            }
            Err(_) => return BcuProbe::NotExecutableShortcut,
        }
    }
    let Ok(output) = child.wait_with_output() else {
        return BcuProbe::NotExecutableShortcut;
    };
    if !output.status.success() {
        return BcuProbe::NotExecutableShortcut;
    }
    let output_text = String::from_utf8_lossy(&output.stdout);
    let mut fields = output_text.trim().split('\t');
    match fields.next() {
        Some("EXECUTABLE") if fields.next().is_none() => BcuProbe::ExecutableWithoutUninstaller,
        Some("MATCH") => {
            let Some(id) = fields.next().map(str::trim) else {
                return BcuProbe::NotExecutableShortcut;
            };
            let Some(display_name) = fields.next().map(str::trim) else {
                return BcuProbe::NotExecutableShortcut;
            };
            if id.is_empty() || display_name.is_empty() || fields.next().is_some() {
                return BcuProbe::NotExecutableShortcut;
            }
            let Some(display_name) = base64_decode(display_name)
                .and_then(|value| String::from_utf8(value).ok())
            else {
                return BcuProbe::NotExecutableShortcut;
            };
            BcuProbe::Candidate(BcuCandidate {
                source: PathBuf::new(),
                executable: executable.to_path_buf(),
                id: id.to_owned(),
                display_name,
            })
        }
        _ => BcuProbe::NotExecutableShortcut,
    }
}

fn is_shortcut_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
}

fn is_executable_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
}

fn uninstall_probe_target(target: &Path) -> Option<PathBuf> {
    let executable = if is_shortcut_file(target) {
        shortcut_target(target)?
    } else {
        target.to_path_buf()
    };
    (executable.is_file() && is_executable_file(&executable)).then_some(executable)
}

fn launch_bcu_uninstaller(candidates: &[BcuCandidate], quiet: bool) -> Result<()> {
    let bridge = bcu_bridge_path();
    if !bridge.is_file() || candidates.is_empty() {
        return Err(windows::core::Error::from_thread());
    }
    let mut command = Command::new(bridge);
    command
        .arg("uninstall-batch")
        .creation_flags(CREATE_NO_WINDOW);
    if quiet {
        command.arg("--quiet");
    }
    for candidate in candidates {
        command.arg(&candidate.executable).arg(&candidate.id);
    }
    command.spawn().map_err(|_| windows::core::Error::from_thread())?;
    Ok(())
}

fn base64_decode(value: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(value.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for byte in value.bytes().take_while(|byte| *byte != b'=') {
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        buffer = (buffer << 6) | digit;
        bits += 6;
        while bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(output)
}

unsafe fn restore_default_cursor() {
    if let Ok(cursor) = LoadCursorW(None, IDC_ARROW) {
        let _ = SetCursor(Some(cursor));
    }
}

fn mci(command: &str) -> u32 {
    unsafe {
        mciSendStringW(PCWSTR(wide(command).as_ptr()), None, None)
    }
}
fn resource_dir() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn user_config_directory() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| resource_dir())
        .join("MonsterDeleter")
}

fn user_config_path() -> PathBuf {
    user_config_directory().join("config.json")
}

fn legacy_user_config_path() -> PathBuf {
    user_config_directory().join("config.toml")
}

fn uninstall_index_path() -> PathBuf {
    user_config_directory().join("uninstall-index.tsv")
}

fn uninstall_feature_enabled() -> bool {
    read_uninstall_config().enabled
}

fn uninstall_silent_execution_enabled() -> bool {
    let config = read_uninstall_config();
    config.enabled && matches!(config.mode.as_str(), "silent" | "force_silent")
}

fn write_user_config(uninstall_enabled: bool, silent_execution: bool) -> std::io::Result<()> {
    let mut config = read_uninstall_config();
    config.enabled = uninstall_enabled;
    config.mode = if uninstall_enabled && silent_execution {
        "silent".to_owned()
    } else {
        "official".to_owned()
    };
    write_uninstall_config(&config)
}

fn write_uninstall_config(config: &UninstallConfig) -> std::io::Result<()> {
    let path = user_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let document = json!({
        "uninstall": {
            "enabled": config.enabled,
            "mode": config.mode,
            "target_patterns": config.target_patterns,
            "batch_target_patterns": config.batch_target_patterns,
            "cleanup_after_uninstall": false,
        }
    });
    let contents = serde_json::to_string_pretty(&document)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    fs::write(path, format!("{contents}\n"))
}

fn read_uninstall_config() -> UninstallConfig {
    if let Ok(contents) = fs::read_to_string(user_config_path()) {
        if let Ok(document) = serde_json::from_str::<Value>(&contents) {
            let uninstall = document.get("uninstall").and_then(Value::as_object);
            let mut config = UninstallConfig::default();
            if let Some(uninstall) = uninstall {
                config.enabled = uninstall
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(config.enabled);
                config.mode = uninstall
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or(&config.mode)
                    .to_owned();
                if let Some(patterns) = uninstall.get("target_patterns").and_then(Value::as_array) {
                    config.target_patterns = patterns
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect();
                }
                if let Some(patterns) = uninstall.get("batch_target_patterns").and_then(Value::as_array) {
                    config.batch_target_patterns = patterns
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect();
                }
            }
            return config;
        }
    }

    let legacy = fs::read_to_string(legacy_user_config_path()).ok();
    let Some(legacy) = legacy else {
        return UninstallConfig::default();
    };
    let config = UninstallConfig {
        enabled: !legacy.contains("enabled = false"),
        mode: if legacy.contains("mode = \"silent\"") || legacy.contains("mode = \"force_silent\"") {
            "silent".to_owned()
        } else {
            "official".to_owned()
        },
        ..Default::default()
    };
    let _ = write_uninstall_config(&config);
    config
}

fn uninstall_target_matches_config(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches_uninstall_patterns(name, &read_uninstall_config().target_patterns)
}

fn batch_uninstall_target_matches_config(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches_uninstall_patterns(name, &read_uninstall_config().batch_target_patterns)
}

fn matches_uninstall_patterns(name: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| {
        Regex::new(pattern)
            .ok()
            .is_some_and(|regex| regex.is_match(name))
    })
}

fn show_settings() -> Result<()> {
    let enabled = uninstall_feature_enabled();
    let silent_execution = uninstall_silent_execution_enabled();
    let title = wide("Monster Deleter 设置");
    let config_path = user_config_path();
    let uninstall_content = wide(format!(
        "当前：{}。\n\n启用卸载功能吗？\n\n启用后，确认删除软件快捷方式时，会额外询问是否调用该软件的官方卸载程序；关闭后始终执行原有的小怪兽删除逻辑。\n\n设置保存在：{}",
        if enabled { "已启用" } else { "未启用" },
        config_path.display()
    ));
    let uninstall_result = unsafe {
        MessageBoxW(
            None,
            PCWSTR(uninstall_content.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_YESNO | MB_ICONWARNING,
        )
    };
    let uninstall_enabled = uninstall_result == IDYES;
    let silent = if uninstall_enabled {
        let silent_content = wide(format!(
            "当前：{}。\n\n让卸载功能静默执行吗？\n\n选择“是”会尽量跳过软件自身的卸载界面；仍可能显示 UAC 或厂商窗口。选择“否”则使用官方卸载界面。",
            if silent_execution { "静默执行" } else { "正常执行" }
        ));
        unsafe {
            MessageBoxW(
                None,
                PCWSTR(silent_content.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_YESNO | MB_ICONWARNING,
            ) == IDYES
        }
    } else {
        false
    };
    write_user_config(uninstall_enabled, silent).map_err(|_| windows::core::Error::from_thread())?;
    Ok(())
}

fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

fn receive_selection(bytes: &[u8]) -> Vec<PathBuf> {
    // A selection path list is normally a few kilobytes.  Bound this local
    // IPC input so an unrelated local process cannot make the overlay allocate
    // an arbitrary amount of memory.
    if bytes.is_empty() || bytes.len() > SELECTION_BROKER_MAX_PACKET {
        return Vec::new();
    }
    if bytes.len() % 2 != 0 {
        return Vec::new();
    }
    let units = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    units
        .split(|unit| *unit == 0)
        .filter(|path| !path.is_empty())
        .filter_map(|path| String::from_utf16(path).ok())
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect()
}

struct SelectionBrokerMutex(HANDLE);

impl Drop for SelectionBrokerMutex {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

/// Only the process which creates this mutex can ever launch an animation.
/// A second broker instance exits before it can open an overlay.
fn acquire_selection_broker_mutex() -> Result<Option<SelectionBrokerMutex>> {
    unsafe {
        let name = wide(SELECTION_BROKER_MUTEX);
        // Do not infer ownership from GetLastError: it is thread-local and a
        // stale value can report ERROR_ALREADY_EXISTS for a newly created
        // mutex.  A zero-time wait gives an unambiguous ownership result.
        let handle = CreateMutexW(None, false, PCWSTR(name.as_ptr()))?;
        let wait = WaitForSingleObject(handle, 0);
        if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED {
            let _ = CloseHandle(handle);
            return Ok(None);
        }
        Ok(Some(SelectionBrokerMutex(handle)))
    }
}

/// Count the complete group of target messages before any overlay is created.
/// This is deliberately a broker-only operation: the normal animation process
/// never races a sibling Explorer invocation.
fn receive_selection_batch(socket: &UdpSocket) -> Vec<PathBuf> {
    let started = Instant::now();
    let mut first_received = None;
    let mut collected = Vec::new();
    loop {
        let mut packet = [0_u8; SELECTION_BROKER_MAX_PACKET];
        match socket.recv_from(&mut packet) {
            Ok((length, source)) => {
                // Acknowledge every datagram.  The bootstrap only exits after
                // receiving this reply, so a packet sent before the broker is
                // ready cannot silently lose a selected target.
                let _ = socket.send_to(&[1_u8], source);
                let received = receive_selection(&packet[..length]);
                if !received.is_empty() {
                    collected.extend(received);
                    first_received.get_or_insert_with(Instant::now);
                }
            }
            // Windows can report ConnectionReset on the next UDP receive when
            // a short-lived bootstrap has already closed its reply socket.
            // It does not mean that the broker or its bound endpoint failed.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::ConnectionReset
                ) => {
                let now = Instant::now();
                let collection_complete = first_received.is_some_and(|first| {
                    now.duration_since(first) >= SELECTION_BROKER_COLLECTION_WINDOW
                });
                if collection_complete || now.duration_since(started) >= SELECTION_BROKER_MAX_WAIT {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
    collected.sort();
    collected.dedup();
    collected
}

/// The broker holds the exclusive mutex for the entire selection transaction.
/// It receives paths from the tiny Explorer entry program, counts and dedupes
/// them, then launches exactly one animation child process.
fn run_selection_broker() -> Result<()> {
    let Some(_mutex) = acquire_selection_broker_mutex()? else {
        return Ok(());
    };
    // Datagram IPC avoids TCP's TIME_WAIT state.  The previous stream listener
    // could be unavailable for a while after an overlay closed, leaving the
    // Explorer entry point spinning even though no broker was running.
    let socket = UdpSocket::bind(SELECTION_BROKER_ADDR)
        .map_err(|_| windows::core::Error::from_thread())?;
    socket
        .set_nonblocking(true)
        .map_err(|_| windows::core::Error::from_thread())?;
    let targets = receive_selection_batch(&socket);
    if targets.is_empty() {
        return Ok(());
    }
    let exe = env::current_exe().map_err(|_| windows::core::Error::from_thread())?;
    let mut child = Command::new(exe)
        .arg("--run-selection")
        .args(targets)
        .spawn()
        .map_err(|_| windows::core::Error::from_thread())?;
    let _ = child.wait();
    Ok(())
}

fn main() {
    let mut args = env::args_os();
    let _ = args.next();
    let first_arg = args.next();
    if first_arg
        .as_deref()
        .is_some_and(|value| value == "--selection-broker")
    {
        let _ = run_selection_broker();
        return;
    }
    if first_arg.as_deref().is_some_and(|value| value == "--settings") {
        let _ = show_settings();
        return;
    }
    if first_arg
        .as_deref()
        .is_some_and(|value| value == "--elevated-delete")
    {
        let targets: Vec<PathBuf> = args.map(PathBuf::from).collect();
        if !targets.is_empty() {
            let _ = recycle(&targets, true);
        }
        return;
    }
    let run_selection = first_arg
        .as_deref()
        .is_some_and(|value| value == "--run-selection");
    let targets: Vec<PathBuf> = first_arg
        .into_iter()
        .filter(|_| !run_selection)
        .chain(args)
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect();
    if !targets.is_empty() {
        let _ = run_overlay(targets);
    }
}

#[cfg(test)]
mod tests {
    use super::{base64_decode, is_executable_file, is_shortcut_file, matches_uninstall_patterns, parse_uninstall_command, recycle_failure_code, DE_ACCESSDENIEDSRC};
    use std::path::Path;
    use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_CANCELLED};

    #[test]
    fn accepts_the_windows_installer_host_without_a_shell() {
        assert_eq!(
            parse_uninstall_command("msiexec.exe /x {01234567-89AB-CDEF-0123-456789ABCDEF}"),
            Some((
                "msiexec.exe".to_owned(),
                "/x {01234567-89AB-CDEF-0123-456789ABCDEF}".to_owned()
            ))
        );
    }

    #[test]
    fn rejects_command_interpreters() {
        assert_eq!(parse_uninstall_command("cmd.exe /c del C:\\data"), None);
        assert_eq!(parse_uninstall_command("powershell.exe -Command Remove-Item"), None);
    }

    #[test]
    fn decodes_bridge_text_without_a_parser_dependency() {
        assert_eq!(base64_decode("5bCP5oCq5YW9"), Some("小怪兽".as_bytes().to_vec()));
    }

    #[test]
    fn maps_legacy_shell_access_denied_to_a_uac_retry() {
        assert_eq!(
            recycle_failure_code(DE_ACCESSDENIEDSRC, false),
            Some(ERROR_ACCESS_DENIED)
        );
        assert_eq!(recycle_failure_code(0, true), Some(ERROR_CANCELLED));
        assert_eq!(recycle_failure_code(0, false), None);
    }

    #[test]
    fn only_lnk_files_are_considered_for_uninstall_detection() {
        assert!(is_shortcut_file(Path::new("app.LNK")));
        assert!(!is_shortcut_file(Path::new("app.exe")));
        assert!(!is_shortcut_file(Path::new("folder")));
        assert!(is_executable_file(Path::new("app.EXE")));
        assert!(!is_executable_file(Path::new("app.lnk")));
    }

    #[test]
    fn configured_patterns_choose_uninstall_probe_targets() {
        let patterns = vec![r"(?i)^.*\.lnk$".to_owned(), r"(?i)^.*\.exe$".to_owned()];
        assert!(matches_uninstall_patterns("Quota Float.LNK", &patterns));
        assert!(matches_uninstall_patterns("quota-float.exe", &patterns));
        assert!(!matches_uninstall_patterns("notes.txt", &patterns));
    }
}
