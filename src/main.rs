#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod camera;
mod capture;
mod display;
mod fps;
mod gpu;
mod grass;
mod pipeline;
mod renderer;
mod scene;
mod settings;
mod terrain;
mod window_input;

use camera::Camera;
use fps::{pace_frame, FpsCounter};
use grass::GrassPath;
use renderer::{Hud, RenderMode, Renderer};
use scene::{Animation, SceneOptions};
use settings::SettingsPanel;
use std::{
    env,
    path::PathBuf,
    time::{Duration, Instant},
};
use window_input::{WindowInput, IDLE_TITLE};
use windows::{
    core::{w, Error, PCWSTR},
    Win32::{
        Foundation::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{
                GetDpiForWindow, SetProcessDpiAwarenessContext,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            },
            Input::KeyboardAndMouse::{
                VK_DELETE, VK_ESCAPE, VK_F1, VK_F3, VK_F4, VK_F5, VK_INSERT, VK_OEM_MINUS,
                VK_OEM_PLUS, VK_SPACE,
            },
            WindowsAndMessaging::*,
        },
    },
};

type AppResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Default)]
struct Options {
    warp: bool,
    sdr: bool,
    capture: Option<PathBuf>,
    time: f32,
    scene: SceneOptions,
}

impl Options {
    fn parse() -> AppResult<Self> {
        let mut options = Self::default();
        let mut args = env::args_os().skip(1);
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--warp") => options.warp = true,
                Some("--sdr") => options.sdr = true,
                Some("--terrain") => {} // Compatibility alias for the default world.
                Some("--grass-path") => {
                    options.scene.grass_path =
                        match args.next().as_deref().and_then(std::ffi::OsStr::to_str) {
                            Some("auto") => GrassPath::Auto,
                            Some("mesh") => GrassPath::Mesh,
                            Some("instanced") => GrassPath::Instanced,
                            Some("off") => GrassPath::Off,
                            _ => {
                                return Err(
                                    "--grass-path expects auto, mesh, instanced, or off".into()
                                )
                            }
                        };
                }
                Some("--grass-full-density") => options.scene.grass_full_density = true,
                Some("--capture") => {
                    options.capture = Some(
                        args.next()
                            .ok_or("--capture requires a PPM file path")?
                            .into(),
                    )
                }
                Some("--time") => {
                    options.time = args
                        .next()
                        .ok_or("--time requires seconds")?
                        .to_string_lossy()
                        .parse()?;
                    if !options.time.is_finite() {
                        return Err("--time requires a finite number".into());
                    }
                }
                Some("--help" | "-h") => {
                    println!("rustgine [--warp] [--sdr] [--capture FILE.ppm] [--time SECONDS]\n         [--grass-path auto|mesh|instanced|off] [--grass-full-density]\nThe alpine overlook faces a mountain lake and conifer forest.\nWASD flies at 50 km/h. Hold Shift for 100 km/h. Click the scene for mouse look.\nF1 cycles Normal, Wireframe Overlay, and Wireframe Only.\nF3 toggles time-of-day animation at two minutes per second. F4 steps forward 15 minutes.\nF5 toggles the settings panel.\nInsert increases grass distance. Delete decreases it.\nEsc releases the mouse, then closes the window. Space pauses the animation. + and - adjust exposure.");
                    std::process::exit(0);
                }
                _ => return Err(format!("Unknown argument {}", arg.to_string_lossy()).into()),
            }
        }
        Ok(options)
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(l.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    // OwnedWindow keeps this allocation alive until DestroyWindow completes.
    let input = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WindowInput).as_ref();
    if let Some(result) = input.and_then(|input| input.handle_message(hwnd, message, w, l)) {
        return result;
    }
    match message {
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, message, w, l)
        }
        WM_DPICHANGED => {
            let r = &*(l.0 as *const RECT);
            let _ = SetWindowPos(
                hwnd,
                None,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, w, l),
    }
}

struct OwnedWindow {
    handle: HWND,
    input: Box<WindowInput>,
}

impl Drop for OwnedWindow {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(self.handle).as_bool() {
                let _ = DestroyWindow(self.handle);
            }
        }
    }
}

unsafe fn run(options: &Options) -> AppResult<()> {
    let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    let instance: HINSTANCE = GetModuleHandleW(None)?.into();
    let name = w!("RustgineD3D12Window");
    let cursor = LoadCursorW(None, IDC_ARROW)?;
    let wc = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: name,
        hCursor: cursor,
        ..Default::default()
    };
    if RegisterClassW(&wc) == 0 {
        return Err(Error::from_win32().into());
    }
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 1280,
        bottom: 720,
    };
    AdjustWindowRectEx(
        &mut rect,
        WS_OVERLAPPEDWINDOW,
        false,
        WINDOW_EX_STYLE::default(),
    )?;
    let input = Box::new(WindowInput::new(options.capture.is_none(), cursor));
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        name,
        IDLE_TITLE,
        WS_OVERLAPPEDWINDOW,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        rect.right - rect.left,
        rect.bottom - rect.top,
        None,
        None,
        instance,
        Some(std::ptr::from_ref(input.as_ref()).cast()),
    )?;
    let window = OwnedWindow {
        handle: hwnd,
        input,
    };
    window.input.register_mouse(hwnd)?;
    GetClientRect(hwnd, &mut rect)?;
    let mut panel = SettingsPanel::new();
    let mut renderer = Renderer::new(
        hwnd,
        [rect.right as u32, rect.bottom as u32],
        options.warp,
        options.sdr || options.capture.is_some(),
        options.scene,
        &mut panel.context,
    )?;
    if options.capture.is_none() {
        let _ = ShowWindow(hwnd, SW_SHOW);
    }

    let mut previous = Instant::now();
    let mut display_check = previous;
    let mut fps = FpsCounter::new(previous);
    let mut playing = true;
    let mut camera = Camera::alpine_overlook();
    let mut animation = Animation::new(f64::from(options.time));
    animation.set_day_minutes(9.75 * 60.0);
    let mut exposure = 0.0f32;
    let mut mode = RenderMode::default();
    let mut frame = 0;
    let capture_frame = u64::from(terrain::MAX_DEPTH + 2);
    loop {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                return Ok(());
            }
            if msg.message == WM_KEYDOWN {
                let key = msg.wParam.0 as u16;
                let panel_open = window.input.panel.is_open();
                let first_press = msg.lParam.0 & (1 << 30) == 0;
                if key == VK_F5.0 {
                    if first_press {
                        window.input.set_panel_open(hwnd, !panel_open);
                    }
                    continue;
                }
                if panel_open && panel.context.io().want_text_input {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                    continue;
                }
                if key == VK_ESCAPE.0 {
                    if first_press {
                        if panel_open {
                            window.input.set_panel_open(hwnd, false);
                        } else if !window.input.release_mouse(hwnd) {
                            DestroyWindow(hwnd)?;
                        }
                    }
                    continue;
                }
                if key == VK_SPACE.0 && first_press && !panel_open {
                    playing = !playing;
                }
                if key == VK_F1.0 {
                    if first_press {
                        mode = mode.next();
                    }
                    continue;
                }
                if key == VK_F3.0 || key == VK_F4.0 {
                    if first_press {
                        if key == VK_F3.0 {
                            animation.toggle_time_of_day();
                        } else {
                            animation.step_time_of_day();
                        }
                    }
                    continue;
                }
                if key == VK_INSERT.0 || key == VK_DELETE.0 {
                    if first_press {
                        renderer.adjust_grass_distance(key == VK_INSERT.0);
                    }
                    continue;
                }
                if key == VK_OEM_PLUS.0 {
                    exposure = (exposure + 0.25).min(4.0);
                }
                if key == VK_OEM_MINUS.0 {
                    exposure = (exposure - 0.25).max(-4.0);
                }
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        GetClientRect(hwnd, &mut rect)?;
        if IsIconic(hwnd).as_bool() || rect.right == 0 || rect.bottom == 0 {
            WaitMessage()?;
            previous = Instant::now();
            fps = FpsCounter::new(previous);
            continue;
        }
        let now = Instant::now();
        let check_display = now.duration_since(display_check) >= Duration::from_secs(1);
        if check_display {
            display_check = now;
        }
        renderer.refresh(hwnd, rect.right as u32, rect.bottom as u32, check_display)?;
        let seconds = now.duration_since(previous).as_secs_f64();
        let (movement, mouse, boost) = window.input.sample(hwnd);
        camera.update(movement, mouse, seconds.min(0.1) as f32, boost);
        if options.capture.is_none() {
            if playing {
                animation.advance(seconds);
            }
            animation.update_time_of_day(seconds);
        }
        previous = now;
        let capture = options
            .capture
            .as_deref()
            .filter(|_| frame == capture_frame);
        let dpi = GetDpiForWindow(hwnd);
        let fps_hud = if options.capture.is_none() {
            fps.shader_constants(dpi)
        } else {
            [0; 4]
        };
        let settings = panel.frame(
            &window.input.panel,
            [
                rect.right as f32,
                rect.bottom as f32,
                dpi.max(96) as f32 / 96.0,
            ],
            seconds as f32,
            &mut animation,
            &mut camera,
            &mut renderer,
        );
        renderer.draw(
            &camera,
            animation,
            exposure,
            mode,
            capture,
            Hud {
                fps: fps_hud,
                settings,
            },
        )?;
        fps.frame_presented(Instant::now());
        if capture.is_some() {
            return Ok(());
        }
        if options.capture.is_none() {
            pace_frame(now, panel.max_fps());
        }
        frame += 1u64;
    }
}

fn main() {
    let result = Options::parse().and_then(|options| unsafe { run(&options) });
    if let Err(error) = result {
        eprintln!("Rustgine: {error}");
        // Explorer-launched release builds may have no console.
        if !cfg!(debug_assertions) && env::args_os().len() == 1 {
            let text: Vec<u16> = error.to_string().encode_utf16().chain(Some(0)).collect();
            unsafe {
                MessageBoxW(
                    None,
                    PCWSTR(text.as_ptr()),
                    w!("Rustgine"),
                    MB_ICONERROR | MB_OK,
                );
            }
        }
        std::process::exit(1);
    }
}
