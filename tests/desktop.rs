//! Run with cargo test --test desktop -- --ignored --nocapture in a Windows desktop session.
use std::{
    fs,
    io::{BufRead, BufReader, Read},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::*,
    UI::{
        Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_DELETE, VK_F1, VK_F4, VK_F5, VK_INSERT},
        WindowsAndMessaging::*,
    },
};

const DEMO: &str = env!("CARGO_BIN_EXE_rustgine");

fn capture_warp(label: &str, time: &str, grass: &str) -> (String, Vec<u8>) {
    capture_warp_scene(label, &["--grass-path", grass, "--time", time])
}

fn capture_warp_scene(label: &str, args: &[&str]) -> (String, Vec<u8>) {
    capture_with_keys(label, args, &[])
}

fn capture_with_keys(label: &str, args: &[&str], keys: &[VIRTUAL_KEY]) -> (String, Vec<u8>) {
    let path = std::env::temp_dir().join(format!("rustgine-{}-{label}.ppm", std::process::id()));
    let mut child = Command::new(DEMO)
        .arg("--warp")
        .args(args)
        .arg("--capture")
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut log = String::new();
    if !keys.is_empty() {
        let hwnd = wait_for_window(&mut child, false, Instant::now() + Duration::from_secs(20));
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert_ne!(
                stdout.read_line(&mut line).unwrap(),
                0,
                "demo exited before presentation"
            );
            log.push_str(&line);
            if line.starts_with("Presentation:") {
                break;
            }
        }
        for &key in keys {
            press_key(hwnd, key);
        }
        stdout.read_to_string(&mut log).unwrap();
    }
    let result = child.wait_with_output().unwrap();
    log.push_str(&String::from_utf8(result.stdout).unwrap());
    assert!(
        result.status.success(),
        "{log}\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let raw = fs::read(&path).unwrap();
    fs::remove_file(path).unwrap();
    (log, raw)
}

fn ppm_image(raw: &[u8]) -> (usize, usize, &[[u8; 3]]) {
    let mut ppm = raw.splitn(4, |&b| b == b'\n');
    assert_eq!(ppm.next(), Some(&b"P6"[..]));
    let mut dimensions = std::str::from_utf8(ppm.next().unwrap())
        .unwrap()
        .split_whitespace()
        .map(|n| n.parse::<usize>().unwrap());
    let (width, height) = (dimensions.next().unwrap(), dimensions.next().unwrap());
    assert_eq!(ppm.next(), Some(&b"255"[..]));
    let rgb = ppm.next().unwrap();
    assert_eq!(rgb.len(), width * height * 3);
    (width, height, rgb.as_chunks::<3>().0)
}

// Captures exercise both shader passes and the real swap-chain readback on WARP.
#[test]
#[ignore = "requires D3D12 WARP and a Windows desktop session"]
fn captures_show_a_lit_cube_rotating_above_the_terrain() {
    let mut images = Vec::new();
    for time in ["0", "2"] {
        let (log, raw) = capture_warp(time, time, "off");
        if time == "0" {
            let peak: f32 = log
                .split("Scene-linear RGB peak ")
                .nth(1)
                .unwrap()
                .split(',')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            assert!(peak > 1.0, "HDR scene was clipped, peak {peak}");
        }
        let (w, h, rgb) = ppm_image(&raw);
        let [red, green, _] = rgb[w * (h / 2) + w / 2];
        assert!(green.saturating_sub(red) > 35, "missing teal cube");
        assert!(
            log.contains("5000 m x 5000 m"),
            "terrain missing from cube capture"
        );
        images.push(rgb.as_flattened().to_vec());
    }
    let changed = images[0]
        .iter()
        .zip(&images[1])
        .filter(|(a, b)| a.abs_diff(**b) > 10)
        .count();
    assert!(
        changed > images[0].len() / 100,
        "rotation did not change the image"
    );
}

#[test]
#[ignore = "requires D3D12 WARP and a Windows desktop session"]
fn grass_capture_repeats_and_wind_moves_blades() {
    let (log, first) = capture_warp("grass", "0", "instanced");
    let (_, repeat) = capture_warp("grass-repeat", "0", "instanced");
    let (_, later) = capture_warp("grass-later", "2", "instanced");
    let (_, original) = capture_warp("grass-off", "0", "off");
    assert!(log.contains("Grass path: instanced"), "{log}");
    assert!(first == repeat, "fixed-time grass capture changed");

    let (w, h, first) = ppm_image(&first);
    let (_, _, later) = ppm_image(&later);
    let (_, _, original) = ppm_image(&original);
    let center = (h / 2) * w + w / 2;
    assert_eq!(first[center], original[center], "grass occluded the cube");
    // Sample the exposed left peak above the mixed-material foothills.
    for y in h * 9 / 64..h * 7 / 32 {
        let strip = y * w + w * 11 / 48..y * w + w * 14 / 48;
        assert_eq!(
            first[strip.clone()],
            original[strip],
            "grass grew on exposed rock"
        );
    }
    // This foreground region sits outside the cube and its shadow.
    let mut green_pixels = 0;
    let mut moved_pixels = 0;
    for y in (h * 2 / 3)..h {
        for x in 0..(w / 5) {
            let i = y * w + x;
            green_pixels += usize::from(
                first[i] != original[i] && first[i][1].saturating_sub(first[i][2]) > 16,
            );
            moved_pixels += usize::from(first[i][1].abs_diff(later[i][1]) > 10);
        }
    }
    assert!(green_pixels > w * h / 50, "missing foreground grass");
    assert!(moved_pixels > w * h / 200, "wind did not move the grass");
}

#[test]
#[ignore = "requires D3D12 WARP and a Windows desktop session"]
fn terrain_capture_contains_a_refined_heightfield_and_sky() {
    let (log, raw) = capture_warp_scene("terrain", &["--terrain"]);
    assert!(log.contains("5000 m x 5000 m, 513 x 513"), "{log}");
    let active: u32 = log
        .lines()
        .find_map(|line| {
            line.strip_prefix("Terrain: ")?
                .split_once(" active triangles")
        })
        .unwrap()
        .0
        .parse()
        .unwrap();
    assert!((128..131_072).contains(&active), "{log}");
    let (w, h, rgb) = ppm_image(&raw);
    let [r, g, b] = rgb[w * (h / 16) + w / 2];
    assert!(b > g && g > r, "missing blue sky");
    // The left hillside is viewed at a grazing angle, where the studio reflection washed it out.
    let hillside = rgb.chunks_exact(w).skip(h * 5 / 16).take(h / 8);
    let pale = hillside
        .flat_map(|row| &row[w / 16..w * 5 / 16])
        .filter(|pixel| pixel.iter().all(|&channel| channel > 175))
        .count();
    assert!(
        pale < w * h / 640,
        "the hillside should retain its ground colors"
    );
    let meadow = rgb[w * (h / 2)..]
        .iter()
        .filter(|&&[r, g, b]| g > r && g.saturating_sub(b) > 15)
        .count();
    assert!(meadow > w * h / 10, "missing rendered terrain");
}

#[test]
#[ignore = "requires D3D12 WARP and a Windows desktop session"]
fn f1_cycles_shaded_overlay_and_wireframe_only() {
    let args = ["--grass-path", "instanced"];
    let capture_mode = |label, presses| {
        // At long grass distances, densely overlapping wireframe lines cover the ground.
        let keys: Vec<_> = [VK_DELETE; 21]
            .into_iter()
            .chain([VK_F1; 3][..presses].iter().copied())
            .collect();
        capture_with_keys(label, &args, &keys).1
    };
    let normal = capture_mode("f1-normal", 0);
    let overlay = capture_mode("f1-overlay", 1);
    let wireframe = capture_mode("f1-wireframe", 2);
    let returned = capture_mode("f1-returned", 3);
    assert!(returned == normal, "three presses should return to Normal");

    let (w, h, normal) = ppm_image(&normal);
    let (_, _, overlay) = ppm_image(&overlay);
    let (_, _, wireframe) = ppm_image(&wireframe);
    let sky = w * (h / 16) + w / 2;
    assert_eq!(overlay[sky], normal[sky], "overlay should preserve the sky");
    assert!(
        wireframe[sky][2] < 32,
        "wireframe should have a dark background"
    );
    let ground = w * (h / 2);
    let overlaid = overlay[ground..]
        .iter()
        .zip(&normal[ground..])
        .filter(|(a, b)| a != b)
        .count();
    let wires = wireframe[ground..]
        .iter()
        .filter(|&&[_, _, b]| b > 60)
        .count();
    assert!(overlaid > w * h / 100, "overlay should draw visible edges");
    assert!(
        overlaid < (w * h - ground) / 2,
        "overlay should preserve shaded surfaces"
    );
    assert!(
        wires > w * h / 100,
        "wireframe-only should draw visible edges"
    );
}

#[test]
#[ignore = "requires D3D12 WARP and a Windows desktop session"]
fn grass_distance_keys_change_visible_patches_and_restore_the_default() {
    let args = ["--grass-path", "instanced"];
    let patch_count = |log: &str| -> usize {
        log.lines()
            .find_map(|line| line.strip_prefix("Grass: ")?.split_once(" visible patches"))
            .unwrap()
            .0
            .parse()
            .unwrap()
    };
    let (base, original) = capture_with_keys("grass-distance-default", &args, &[]);
    assert!(base.contains("Grass distance: 1000 m."), "{base}");
    let (shorter, _) = capture_with_keys("grass-distance-shorter", &args, &[VK_DELETE]);
    assert!(shorter.contains("Grass distance: 800 m."), "{shorter}");
    assert!(patch_count(&shorter) < patch_count(&base));

    let (_, restored) = capture_with_keys(
        "grass-distance-restored",
        &args,
        &[VK_DELETE, VK_INSERT, VK_INSERT],
    );
    assert!(
        restored == original,
        "distance should clamp and return to the original patches"
    );
}

#[test]
#[ignore = "requires D3D12 WARP and a Windows desktop session"]
fn f5_toggles_the_settings_overlay() {
    let args = ["--grass-path", "off"];
    let (_, hidden) = capture_with_keys("settings-default", &args, &[]);
    let (_, opened) = capture_with_keys("settings-opened", &args, &[VK_F5]);
    let (_, closed) = capture_with_keys("settings-closed", &args, &[VK_F5; 2]);
    let (w, h, hidden_pixels) = ppm_image(&hidden);
    let (_, _, opened_pixels) = ppm_image(&opened);
    let changed = hidden_pixels
        .iter()
        .zip(opened_pixels)
        .filter(|(a, b)| a != b)
        .count();
    assert!(changed > w * h / 100, "F5 should draw the settings panel");
    assert!(
        changed < w * h / 2,
        "the panel should leave most of the scene visible"
    );
    assert!(closed == hidden, "F5 should restore the original scene");
}

#[test]
#[ignore = "requires D3D12 WARP and a Windows desktop session"]
fn twilight_sky_has_no_hard_shadow_band() {
    let (_, twilight) = capture_with_keys("sky-twilight", &["--grass-path", "off"], &[VK_F4; 25]);
    let (w, h, pixels) = ppm_image(&twilight);

    // At 18:15 the old shadow boundary crossed this open strip of sky east of the peaks.
    let mut peak_jump = 0;
    for y in h * 21 / 100..h * 23 / 100 {
        for x in w * 87 / 100..w * 95 / 100 {
            let above = pixels[y * w + x];
            let below = pixels[(y + 1) * w + x];
            for (a, b) in above.into_iter().zip(below) {
                peak_jump = peak_jump.max(a.abs_diff(b));
            }
        }
    }
    assert!(peak_jump <= 3, "twilight sky channel jumped by {peak_jump}");
}

#[test]
#[ignore = "requires D3D12 WARP and a Windows desktop session"]
fn f4_advances_the_sun_and_wraps_after_a_full_day() {
    let args = ["--grass-path", "off"];
    let (_, noon) = capture_with_keys("time-noon", &args, &[]);
    let (_, afternoon) = capture_with_keys("time-afternoon", &args, &[VK_F4]);
    let (_, midnight) = capture_with_keys("time-midnight", &args, &[VK_F4; 48]);
    let (_, next_noon) = capture_with_keys("time-next-noon", &args, &[VK_F4; 96]);

    assert!(afternoon != noon, "F4 should move the sun");
    let (w, h, noon_sky) = ppm_image(&noon);
    let (_, _, midnight_sky) = ppm_image(&midnight);
    let sky_pixel = w * (h / 16) + w / 2;
    assert!(
        midnight_sky[sky_pixel][2] < noon_sky[sky_pixel][2] / 4,
        "night sky should darken"
    );
    assert!(
        next_noon == noon,
        "96 quarter-hour steps should return to the same time"
    );
}

struct Demo(Child);
impl Drop for Demo {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

unsafe extern "system" fn find_demo(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = &mut *(lparam.0 as *mut (u32, HWND, bool));
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == search.0 && (!search.2 || IsWindowVisible(hwnd).as_bool()) {
        let mut class = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut class);
        if String::from_utf16_lossy(&class[..len as usize]) == "RustgineD3D12Window" {
            search.1 = hwnd;
            return false.into();
        }
    }
    true.into()
}

fn wait_for_window(child: &mut Child, visible: bool, deadline: Instant) -> HWND {
    let mut search = (child.id(), HWND::default(), visible);
    while search.1 == HWND::default() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "demo exited during startup"
        );
        assert!(Instant::now() < deadline, "demo window did not appear");
        let _ = unsafe { EnumWindows(Some(find_demo), LPARAM(&mut search as *mut _ as isize)) };
        thread::sleep(Duration::from_millis(5));
    }
    search.1
}

fn press_key(hwnd: HWND, key: VIRTUAL_KEY) {
    unsafe {
        let key = WPARAM(key.0.into());
        PostMessageW(hwnd, WM_KEYDOWN, key, LPARAM(1)).unwrap();
        PostMessageW(hwnd, WM_KEYDOWN, key, LPARAM((1 << 30) | 1)).unwrap();
        PostMessageW(hwnd, WM_KEYUP, key, LPARAM(1)).unwrap();
    }
}

#[test]
#[ignore = "opens the native demo window in a Windows desktop session"]
fn native_window_resizes_minimizes_restores_and_closes() {
    let mut demo = Demo(Command::new(DEMO).stdin(Stdio::null()).spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(20);
    let hwnd = wait_for_window(&mut demo.0, true, deadline);
    unsafe {
        for (w, h) in [(875, 513), (1121, 661)] {
            press_key(hwnd, VK_F1);
            SetWindowPos(
                hwnd,
                None,
                0,
                0,
                w,
                h,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
            .unwrap();
            thread::sleep(Duration::from_millis(350));
            let mut rect = RECT::default();
            GetWindowRect(hwnd, &mut rect).unwrap();
            assert_eq!((rect.right - rect.left, rect.bottom - rect.top), (w, h));
        }
        let _ = ShowWindow(hwnd, SW_MINIMIZE);
        assert!(IsIconic(hwnd).as_bool());
        thread::sleep(Duration::from_millis(250));
        let _ = ShowWindow(hwnd, SW_RESTORE);
        assert!(!IsIconic(hwnd).as_bool());
        thread::sleep(Duration::from_millis(450));
        press_key(hwnd, VK_F1);
        PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)).unwrap();
    }
    while demo.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "demo did not exit");
        thread::sleep(Duration::from_millis(25));
    }
    assert!(demo.0.wait().unwrap().success());
}
