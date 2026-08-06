#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! A per-pixel-alpha Win32 overlay. We deliberately avoid a GPU swapchain here:
//! on many Windows 10 drivers an HWND swapchain reports an opaque alpha mode,
//! which turns an otherwise transparent overlay into a black fullscreen window.

use std::{
    env,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr::null_mut,
    slice,
    time::Instant,
};

use image::{imageops::FilterType, RgbaImage};
use windows::{
    core::{Result, PCWSTR},
    Win32::{
        Foundation::{
            COLORREF, ERROR_ACCESS_DENIED, ERROR_CANCELLED, HWND, LPARAM, LRESULT, POINT, RECT,
            SIZE, WIN32_ERROR, WPARAM,
        },
        Graphics::Gdi::{
            CreateBitmap, CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC,
            DeleteObject, DrawTextW, GetMonitorInfoW, MonitorFromPoint, SelectObject, SetBkMode,
            SetTextColor, AC_SRC_ALPHA, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION,
            CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS,
            DT_CENTER, DT_SINGLELINE, DT_VCENTER, FF_DONTCARE, FONT_FAMILY, FW_BOLD, MONITORINFO,
            MONITOR_DEFAULTTONEAREST, OUT_TT_PRECIS, TRANSPARENT,
        },
        Media::Multimedia::mciSendStringW,
        Storage::FileSystem::GetFileAttributesW,
        UI::{
            HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS},
            Shell::{
                SHFileOperationW, ShellExecuteW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI,
                FO_DELETE, SHFILEOPSTRUCTW,
            },
            WindowsAndMessaging::{
                CreateIconIndirect, CreateWindowExW, DefWindowProcW, DestroyCursor, DestroyWindow,
                DispatchMessageW, GetCursorPos, GetMessageW, PostQuitMessage, RegisterClassW,
                SetCursor, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage,
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

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Select,
    FadeOut,
    Walk,
    Point,
    Ask,
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
            ("monster_bgm", assets.join("音频").join("bgm(1).mp3"), 500),
            (
                "monster_talk",
                assets.join("音频").join("怪兽说话.mp3"),
                1000,
            ),
            ("monster_boom", assets.join("音频").join("爆炸.MP4"), 300),
        ];
        let mut aliases = Vec::new();
        for (alias, path, volume) in files {
            if path.exists() {
                mci(&format!(
                    "open \"{}\" type mpegvideo alias {alias}",
                    path.display()
                ));
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
    pixels: Vec<u8>,
    assets: PathBuf,
    background: Option<RgbaImage>,
    target: PathBuf,
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
    deletion_started: bool,
    audio: Audio,
    error: Option<String>,
}

impl OverlayApp {
    unsafe fn new(target: PathBuf) -> Option<Self> {
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
            pixels: vec![0; width as usize * height as usize * 4],
            assets: assets.clone(),
            background,
            target,
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
            deletion_started: false,
            audio: Audio::new(&assets),
            error: None,
        })
    }

    fn elapsed(&self) -> f32 {
        self.phase_started.elapsed().as_secs_f32()
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
                250,
            );
        }
    }
    fn ensure_point(&mut self) {
        if self.point.is_none() {
            self.point = Sprite::load(
                &self.assets.join("指着文件_spritesheet_transparent.png"),
                250,
            );
        }
    }
    fn ensure_kick_sequence(&mut self) {
        if self.kick.is_none() {
            self.kick = Sprite::load(
                &self.assets.join("踹文件动效_spritesheet_transparent.png"),
                250,
            );
        }
        if self.explosion.is_none() {
            self.explosion =
                Sprite::load(&self.assets.join("爆炸_spritesheet_transparent.png"), 150);
        }
        if self.leo.is_none() {
            self.leo = Sprite::load(
                &self.assets.join("雷欧登场_spritesheet_transparent.png"),
                250,
            );
        }
        if self.fly.is_none() {
            self.fly = Sprite::load(
                &self.assets.join("出场飞行动效_spritesheet_transparent.png"),
                250,
            );
        }
    }
    fn click(&mut self, x: i32, y: i32) {
        match self.phase {
            Phase::Select => {
                self.target_position = (x, y);
                self.points_left = x < self.width / 2;
                self.ensure_walk();
                self.enter(Phase::FadeOut);
            }
            Phase::Ask => {
                if self.choice_rects().iter().any(|rect| rect.contains(x, y)) {
                    self.enter(Phase::Kick);
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
                    match request_elevation(&self.target) {
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
        let group_width = 297;
        let button_x = (mx + mw / 2 - group_width / 2).clamp(12, self.width - group_width - 12);
        let below = my + mh + 68 <= self.height - 12;
        let button_y = if below {
            my + mh + 16
        } else {
            (my - 68).max(12)
        };
        [
            RectI {
                x: button_x,
                y: button_y,
                w: 92,
                h: 52,
            },
            RectI {
                x: button_x + 107,
                y: button_y,
                w: 190,
                h: 52,
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
            Phase::Walk if self.elapsed() >= 4.5 => self.enter(Phase::Point),
            Phase::Point if self.elapsed() >= 0.5 => {
                self.ensure_kick_sequence();
                self.enter(Phase::Ask);
                // Keep the original talking sound, but synchronize it with the
                // visible question so it is never lost during the point transition.
                self.audio.play("monster_talk");
            }
            Phase::Kick => {
                if self.frame() >= 5 && !self.deletion_started {
                    self.trigger_delete();
                }
                if self.phase == Phase::Kick && self.elapsed() >= 15.0 / FRAME_RATE {
                    self.enter(Phase::Leo);
                }
            }
            Phase::Leo if self.elapsed() >= 15.0 / FRAME_RATE => self.enter(Phase::Fly),
            Phase::Fly if self.elapsed() >= 2.0 => unsafe {
                let _ = DestroyWindow(self.hwnd);
            },
            _ => {}
        }
        self.render();
    }
    fn trigger_delete(&mut self) {
        self.deletion_started = true;
        self.explosion_started = Some(Instant::now());
        self.audio.play("monster_boom");
        if let Err(error) = recycle(&self.target, false) {
            if error.code() == ERROR_ACCESS_DENIED.into() {
                self.enter(Phase::Elevate);
            } else {
                self.error = Some(format!("删除失败：{error}"));
                self.enter(Phase::Error);
            }
        }
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
        for y in 0..rect.h {
            for x in 0..rect.w {
                let dx = if x < radius {
                    radius - x
                } else if x >= rect.w - radius {
                    x - (rect.w - radius - 1)
                } else {
                    0
                };
                let dy = if y < radius {
                    radius - y
                } else if y >= rect.h - radius {
                    y - (rect.h - radius - 1)
                } else {
                    0
                };
                if dx * dx + dy * dy <= radius * radius {
                    self.blend(rect.x + x, rect.y + y, color.0, color.1, color.2, color.3);
                }
            }
        }
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
            let face = wide("Microsoft YaHei");
            let font = CreateFontW(
                -size,
                0,
                0,
                0,
                FW_BOLD.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_TT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
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
            let _ = DrawTextW(
                dc,
                &mut words,
                &mut bounds,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
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
        self.text(
            "请选择你要摧毁的文件",
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
    fn draw_bubble_size(&mut self, monster_width: i32, monster_height: i32, position: (i32, i32)) {
        let (mx, my) = position;
        let bubble_w = 260;
        let bubble_h = 78;
        let raw_x = if self.points_left {
            mx + monster_width + 18
        } else {
            mx - bubble_w - 18
        };
        let x = raw_x.clamp(12, self.width - bubble_w - 12);
        let side_y = my + monster_height * 30 / 100 - bubble_h / 2;
        let below = side_y < 12;
        let y = if below {
            my + monster_height + 20
        } else {
            side_y.min(self.height - bubble_h - 12)
        };
        let bubble = RectI {
            x,
            y,
            w: bubble_w,
            h: bubble_h,
        };
        self.rect(
            RectI {
                x,
                y: y + 8,
                w: bubble_w,
                h: bubble_h,
            },
            (0, 0, 0, 35),
            20,
        );
        self.rect(bubble, (255, 255, 255, 240), 20);
        self.text("喂，是这个吗？", bubble, 20, (28, 28, 30, 255));
        for rect in self.choice_rects() {
            self.rect(
                RectI {
                    x: rect.x,
                    y: rect.y + 5,
                    ..rect
                },
                (0, 0, 0, 30),
                18,
            );
            self.rect(rect, (255, 255, 255, 240), 18);
        }
        let choices = self.choice_rects();
        self.text("是的", choices[0], 16, (28, 28, 30, 255));
        self.text("嘤嘤嘤就是这个", choices[1], 16, (28, 28, 30, 255));
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
            Phase::Ask => {
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
                    self.draw_bubble_size(w, h, position);
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
                self.image(
                    &image,
                    RectI {
                        x: self.target_position.0 - sprite.width / 2,
                        y: self.target_position.1 - sprite.height / 2 - 40,
                        w: sprite.width,
                        h: sprite.height,
                    },
                    255,
                    false,
                );
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
        WM_SETCURSOR if matches!(app.phase, Phase::Select | Phase::FadeOut) => {
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

fn run_overlay(target: PathBuf) -> Result<()> {
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
            OverlayApp::new(target).ok_or_else(windows::core::Error::from_thread)?,
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

fn recycle(target: &Path, elevated: bool) -> Result<()> {
    let target_wide = wide(target.as_os_str());
    unsafe {
        if GetFileAttributesW(PCWSTR(target_wide.as_ptr())) == u32::MAX {
            return Err(windows::core::Error::from_thread());
        }
        let mut from = target_wide;
        from.push(0);
        let mut operation = SHFILEOPSTRUCTW {
            wFunc: FO_DELETE,
            pFrom: PCWSTR(from.as_ptr()),
            fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI).0 as u16,
            ..Default::default()
        };
        let status = SHFileOperationW(&mut operation);
        if status != 0 || operation.fAnyOperationsAborted.as_bool() {
            let code = if status == 0 {
                ERROR_CANCELLED
            } else {
                WIN32_ERROR(status as u32)
            };
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

fn request_elevation(target: &Path) -> Result<()> {
    let exe = env::current_exe().map_err(|_| windows::core::Error::from_thread())?;
    let parameters = format!("--elevated-delete \"{}\"", target.display());
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

fn mci(command: &str) {
    unsafe {
        let _ = mciSendStringW(PCWSTR(wide(command).as_ptr()), None, None);
    }
}
fn resource_dir() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}
fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

fn main() {
    let mut args = env::args_os();
    let _ = args.next();
    if args
        .next()
        .as_deref()
        .is_some_and(|value| value == "--elevated-delete")
    {
        if let Some(target) = args.next().map(PathBuf::from) {
            let _ = recycle(&target, true);
        }
        return;
    }
    let target = env::args_os().nth(1).map(PathBuf::from).unwrap_or_default();
    let _ = run_overlay(target);
}
