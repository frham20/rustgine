use crate::{
    camera::Camera,
    fps::{DEFAULT_FPS_LIMIT, MAX_FPS_LIMIT, MIN_FPS_LIMIT},
    grass::{GrassSettings, MAX_DENSITY_PERCENT, MAX_DRAW_DISTANCE, MIN_DRAW_DISTANCE},
    renderer::{Renderer, SHADOW_MAP_SIZES},
    scene::{Animation, CloudQuality, SunShaftQuality},
};
use imgui::{Condition, ConfigFlags, Context, DrawData, Key, MouseButton, SliderFlags, Ui};
use std::cell::{Cell, RefCell};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

#[derive(Default)]
pub struct PanelInput {
    open: Cell<bool>,
    events: RefCell<Vec<(u32, WPARAM, LPARAM)>>,
}

impl PanelInput {
    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    pub unsafe fn set_open(&self, hwnd: HWND, open: bool) {
        self.open.set(open);
        if !open {
            if GetCapture() == hwnd {
                let _ = ReleaseCapture();
            }
            self.events.borrow_mut().clear();
            self.events
                .borrow_mut()
                .push((WM_KILLFOCUS, WPARAM(0), LPARAM(0)));
        }
    }

    pub unsafe fn handle_message(
        &self,
        hwnd: HWND,
        message: u32,
        w: WPARAM,
        l: LPARAM,
    ) -> Option<LRESULT> {
        match message {
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                if GetCapture() != hwnd {
                    SetCapture(hwnd);
                }
            }
            WM_LBUTTONUP => {
                if GetCapture() == hwnd {
                    let _ = ReleaseCapture();
                }
            }
            WM_CANCELMODE | WM_KILLFOCUS => {
                if GetCapture() == hwnd {
                    let _ = ReleaseCapture();
                }
            }
            WM_MOUSEMOVE | WM_MOUSEWHEEL | WM_MOUSEHWHEEL | WM_CAPTURECHANGED | WM_SETFOCUS
            | WM_KEYDOWN | WM_KEYUP | WM_CHAR => {}
            _ => return None,
        }
        self.events.borrow_mut().push((message, w, l));
        match message {
            WM_SETFOCUS | WM_KILLFOCUS | WM_CANCELMODE | WM_CAPTURECHANGED => None,
            _ => Some(LRESULT(0)),
        }
    }

    fn feed(&self, context: &mut Context, dpi_scale: f32) -> bool {
        let mut events = self.events.borrow_mut();
        let had_events = !events.is_empty();
        let io = context.io_mut();
        for (message, w, l) in events.drain(..) {
            match message {
                WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_LBUTTONUP => {
                    let x = l.0 as i16;
                    let y = (l.0 >> 16) as i16;
                    io.add_mouse_pos_event([f32::from(x) / dpi_scale, f32::from(y) / dpi_scale]);
                    if message != WM_MOUSEMOVE {
                        io.add_mouse_button_event(MouseButton::Left, message != WM_LBUTTONUP);
                    }
                }
                WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                    let delta = f32::from((w.0 >> 16) as i16) / 120.0;
                    io.add_mouse_wheel_event(if message == WM_MOUSEWHEEL {
                        [0.0, delta]
                    } else {
                        [delta, 0.0]
                    });
                }
                WM_CAPTURECHANGED => io.add_mouse_button_event(MouseButton::Left, false),
                WM_KEYDOWN | WM_KEYUP => {
                    if let Some(key) = panel_key(VIRTUAL_KEY(w.0 as u16)) {
                        io.add_key_event(key, message == WM_KEYDOWN);
                    }
                }
                WM_CHAR if w.0 >= 32 => unsafe {
                    imgui::sys::ImGuiIO_AddInputCharacterUTF16(imgui::sys::igGetIO(), w.0 as u16);
                },
                WM_KILLFOCUS | WM_SETFOCUS | WM_CANCELMODE => unsafe {
                    imgui::sys::ImGuiIO_AddFocusEvent(
                        imgui::sys::igGetIO(),
                        message == WM_SETFOCUS,
                    );
                },
                _ => {}
            }
        }
        had_events
    }
}

fn panel_key(key: VIRTUAL_KEY) -> Option<Key> {
    Some(match key {
        VK_TAB => Key::Tab,
        VK_LEFT => Key::LeftArrow,
        VK_RIGHT => Key::RightArrow,
        VK_UP => Key::UpArrow,
        VK_DOWN => Key::DownArrow,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_DELETE => Key::Delete,
        VK_BACK => Key::Backspace,
        VK_SPACE => Key::Space,
        VK_RETURN => Key::Enter,
        VK_ESCAPE => Key::Escape,
        VK_SHIFT => Key::ModShift,
        VK_CONTROL => Key::ModCtrl,
        VK_A => Key::A,
        VK_Z => Key::Z,
        VK_Y => Key::Y,
        _ => return None,
    })
}

pub struct SettingsPanel {
    pub context: Context,
    shadow_resize_failed: bool,
    cloud_resize_failed: bool,
    max_fps: u32,
}

impl SettingsPanel {
    pub fn new() -> Self {
        let mut context = Context::create();
        context.set_ini_filename(None);
        context.io_mut().config_flags |= ConfigFlags::NAV_ENABLE_KEYBOARD;
        let style = context.style_mut();
        style.window_rounding = 6.0;
        style.frame_rounding = 3.0;
        style.window_padding = [12.0, 12.0];
        style.item_spacing = [8.0, 8.0];
        Self {
            context,
            shadow_resize_failed: false,
            cloud_resize_failed: false,
            max_fps: DEFAULT_FPS_LIMIT,
        }
    }

    pub fn max_fps(&self) -> u32 {
        self.max_fps
    }

    pub fn frame(
        &mut self,
        input: &PanelInput,
        display: [f32; 3],
        seconds: f32,
        animation: &mut Animation,
        camera: &mut Camera,
        renderer: &mut Renderer,
    ) -> Option<&DrawData> {
        let [width, height, scale] = display;
        let open = input.is_open();
        let had_events = input.feed(&mut self.context, scale);
        if !open && !had_events {
            return None;
        }
        let io = self.context.io_mut();
        io.display_size = [width / scale, height / scale];
        io.display_framebuffer_scale = [scale; 2];
        io.delta_time = seconds.max(0.000001);
        let ui = self.context.frame();
        if !open {
            self.context.render();
            return None;
        }
        let mut shadow_size = renderer.shadow_size();
        let mut clouds = renderer.cloud_quality();
        ui.window("Settings")
            .position([12.0, 56.0], Condition::FirstUseEver)
            .size([390.0, 350.0], Condition::FirstUseEver)
            .collapsible(false)
            .build(|| {
                let _width = ui.push_item_width(198.0);
                if let Some(_tabs) = ui.tab_bar("Settings sections") {
                    if let Some(_scene) = ui.tab_item("Scene") {
                        scene_settings(ui, animation, camera, &mut shadow_size, &mut self.max_fps);
                        if self.shadow_resize_failed {
                            ui.text_wrapped("Could not allocate the selected shadow map.");
                        }
                    }
                    if let Some(_clouds) = ui.tab_item("Clouds") {
                        cloud_settings(ui, &mut clouds);
                        if self.cloud_resize_failed {
                            ui.text_wrapped("Could not allocate the selected cloud resolution.");
                        }
                    }
                    if let Some(_grass) = ui.tab_item("Grass") {
                        if let Some(grass) = renderer.grass_settings() {
                            grass_settings(ui, grass);
                        } else {
                            ui.text_disabled("Grass rendering is disabled");
                        }
                    }
                    if let Some(_god_rays) = ui.tab_item("God rays") {
                        sun_shaft_settings(ui, renderer.sun_shaft_quality());
                    }
                }
            });
        if shadow_size != renderer.shadow_size() {
            let result = unsafe { renderer.set_shadow_size(shadow_size) };
            self.shadow_resize_failed = result.is_err();
            if let Err(error) = result {
                eprintln!("Shadow map resize failed: {error}");
            }
        }
        if clouds != renderer.cloud_quality() {
            let result = unsafe { renderer.set_cloud_quality(clouds) };
            self.cloud_resize_failed = result.is_err();
            if let Err(error) = result {
                eprintln!("Cloud target resize failed: {error}");
            }
        }
        Some(self.context.render())
    }
}

fn cloud_settings(ui: &Ui, clouds: &mut CloudQuality) {
    ui.text("Sky resolution");
    if ui.is_item_hovered() {
        ui.tooltip_text(
            "Fraction of the screen width and height. Higher values sharpen cloud edges.",
        );
    }
    for (index, (label, divisor)) in [("1/8", 8), ("1/4", 4), ("1/2", 2), ("Full", 1)]
        .into_iter()
        .enumerate()
    {
        if index > 0 {
            ui.same_line();
        }
        ui.radio_button(label, &mut clouds.resolution_divisor, divisor);
    }
    ui.slider_config("Ray samples", 12, 128)
        .flags(SliderFlags::ALWAYS_CLAMP)
        .build(&mut clouds.view_steps);
    if ui.is_item_hovered() {
        ui.tooltip_text("More samples resolve finer shapes in the sky and reflected clouds.");
    }
    ui.slider_config("Lighting samples", 0, 8)
        .flags(SliderFlags::ALWAYS_CLAMP)
        .build(&mut clouds.light_steps);
    if ui.is_item_hovered() {
        ui.tooltip_text("Samples toward the sun. Set to 0 to skip shading within clouds.");
    }
    ui.slider_config("Cloud shadow samples", 0, 48)
        .flags(SliderFlags::ALWAYS_CLAMP)
        .build(&mut clouds.shadow_steps);
    if ui.is_item_hovered() {
        ui.tooltip_text("Samples for cloud shadows on land and water. Set to 0 to disable them.");
    }
    if ui.button("Reset cloud settings") {
        *clouds = CloudQuality::default();
    }
}

fn sun_shaft_settings(ui: &Ui, quality: &mut SunShaftQuality) {
    ui.slider_config("Radial samples", 0, SunShaftQuality::MAX_RADIAL_SAMPLES)
        .flags(SliderFlags::ALWAYS_CLAMP)
        .build(&mut quality.radial_samples);
    if ui.is_item_hovered() {
        ui.tooltip_text(
            "More samples resolve smaller openings in clouds and terrain at a higher GPU cost. Set to 0 to disable god rays.",
        );
    }
    ui.slider_config("Edge refinement", 0, SunShaftQuality::MAX_EDGE_REFINEMENT)
        .flags(SliderFlags::ALWAYS_CLAMP)
        .build(&mut quality.edge_refinement);
    if ui.is_item_hovered() {
        ui.tooltip_text(
            "More steps smooth shafts near silhouettes at a higher GPU cost. Set to 0 for a linear edge estimate.",
        );
    }
    if ui.button("Reset god rays settings") {
        *quality = SunShaftQuality::default();
    }
}

fn scene_settings(
    ui: &Ui,
    animation: &mut Animation,
    camera: &mut Camera,
    shadow_size: &mut u32,
    max_fps: &mut u32,
) {
    ui.text("Atmosphere");
    ui.separator();
    let mut minutes = animation.day_minutes() as i32;
    let time = format!("{:02}:{:02}", minutes / 60, minutes % 60);
    if ui
        .slider_config("Time of day", 0, 1439)
        .flags(SliderFlags::NO_INPUT)
        .display_format(time)
        .build(&mut minutes)
    {
        animation.set_day_minutes(f64::from(minutes));
    }
    ui.checkbox("Animate time of day", animation.time_of_day_running());
    ui.spacing();
    ui.text("Rendering");
    ui.separator();
    ui.slider_config("Max FPS", MIN_FPS_LIMIT, MAX_FPS_LIMIT)
        .flags(SliderFlags::ALWAYS_CLAMP)
        .display_format("%u FPS")
        .build(max_fps);
    let mut scene_km = camera.draw_distance / 1000.0;
    if ui
        .slider_config("Scene draw distance", 0.1, 18.0)
        .flags(SliderFlags::ALWAYS_CLAMP)
        .display_format("%.2f km")
        .build(&mut scene_km)
    {
        camera.draw_distance = scene_km * 1000.0;
    }
    let mut choice = SHADOW_MAP_SIZES
        .iter()
        .position(|&(size, _)| size == *shadow_size)
        .unwrap();
    let resolutions = SHADOW_MAP_SIZES.map(|(_, label)| label);
    if ui.combo_simple_string("Shadow map size", &mut choice, &resolutions) {
        *shadow_size = SHADOW_MAP_SIZES[choice].0;
    }
    ui.spacing();
    ui.text("Camera");
    ui.separator();
    ui.slider_config("Camera speed", 1.0, 500.0)
        .flags(SliderFlags::ALWAYS_CLAMP)
        .display_format("%.0f km/h")
        .build(&mut camera.speed_kmh);
}

fn grass_settings(ui: &Ui, grass: &mut GrassSettings) {
    ui.slider_config("Grass density", 0, MAX_DENSITY_PERCENT)
        .flags(SliderFlags::ALWAYS_CLAMP)
        .display_format("%u%%")
        .build(&mut grass.density_percent);
    ui.slider_config("Grass height", 10, 300)
        .flags(SliderFlags::ALWAYS_CLAMP)
        .display_format("%u%%")
        .build(&mut grass.height_percent);
    ui.color_edit3("Grass color", &mut grass.color);
    ui.slider_config("Grass draw distance", MIN_DRAW_DISTANCE, MAX_DRAW_DISTANCE)
        .flags(SliderFlags::ALWAYS_CLAMP)
        .display_format("%.0f m")
        .build(&mut grass.draw_distance);
    if ui.is_item_hovered() {
        ui.tooltip_text("Caps all grass geometry distances.");
    }
    ui.spacing();
    ui.text("LOD end distances");
    ui.separator();
    for (level, mut end) in grass.lod_ends().into_iter().enumerate() {
        let [min, max] = grass.lod_end_bounds(level);
        if ui
            .slider_config(format!("LOD {level}"), min, max)
            .flags(SliderFlags::ALWAYS_CLAMP)
            .display_format("%.0f m")
            .build(&mut end)
        {
            grass.set_lod_end(level, end);
        }
    }
}
