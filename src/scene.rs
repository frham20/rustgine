use crate::{
    camera::{frustum_planes, Camera},
    terrain,
};
use glam::{Mat4, Vec3};
use std::f64::consts::TAU;

const CUBE_HALF_SIZE: f32 = 1.0;
const CUBE_CLEARANCE: f32 = 2.0;
const ROTATION_SPEED: f64 = 0.55;
// Both wind oscillations in grass.hlsl repeat at this period.
const WIND_PERIOD: f64 = TAU / 0.65;
const MINUTES_PER_DAY: f64 = 24.0 * 60.0;
const CAMERA_NEAR: f32 = 0.12;
pub const SHADOW_CASCADES: usize = 4;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CloudQuality {
    pub resolution_divisor: u32,
    pub view_steps: u32,
    pub light_steps: u32,
    pub shadow_steps: u32,
}

impl Default for CloudQuality {
    fn default() -> Self {
        Self {
            resolution_divisor: 4,
            view_steps: 40,
            light_steps: 4,
            shadow_steps: 16,
        }
    }
}

#[derive(Clone, Copy)]
pub struct SceneOptions {
    pub alpine: bool,
    pub grass_path: crate::grass::GrassPath,
    pub grass_full_density: bool,
}

impl Default for SceneOptions {
    fn default() -> Self {
        Self {
            alpine: true,
            grass_path: Default::default(),
            grass_full_density: false,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Animation {
    angle: f64,
    wind_time: f64,
    cloud_wind: f64,
    day_minutes: f64,
    time_of_day_running: bool,
}

impl Animation {
    pub fn new(time: f64) -> Self {
        Self {
            angle: (0.22 + time * ROTATION_SPEED).rem_euclid(TAU),
            wind_time: time.rem_euclid(WIND_PERIOD),
            cloud_wind: (time * 0.001).rem_euclid(2.0),
            day_minutes: MINUTES_PER_DAY / 2.0,
            time_of_day_running: false,
        }
    }

    pub fn advance(&mut self, seconds: f64) {
        self.angle = (self.angle + seconds * ROTATION_SPEED).rem_euclid(TAU);
        self.wind_time = (self.wind_time + seconds).rem_euclid(WIND_PERIOD);
        self.cloud_wind = (self.cloud_wind + seconds * 0.001).rem_euclid(2.0);
    }

    pub fn toggle_time_of_day(&mut self) {
        self.time_of_day_running = !self.time_of_day_running;
    }

    pub fn time_of_day_running(&mut self) -> &mut bool {
        &mut self.time_of_day_running
    }

    pub fn day_minutes(&self) -> f64 {
        self.day_minutes
    }

    pub fn set_day_minutes(&mut self, minutes: f64) {
        self.day_minutes = minutes.rem_euclid(MINUTES_PER_DAY);
    }

    pub fn update_time_of_day(&mut self, elapsed_seconds: f64) {
        if self.time_of_day_running {
            self.advance_day_minutes(2.0 * elapsed_seconds);
        }
    }

    pub fn step_time_of_day(&mut self) {
        self.advance_day_minutes(15.0);
    }

    fn advance_day_minutes(&mut self, minutes: f64) {
        self.set_day_minutes(self.day_minutes + minutes);
    }

    fn sun_direction(self) -> Vec3 {
        let (vertical, horizontal) = 75.0_f32.to_radians().sin_cos();
        let azimuth = Vec3::new(-0.67, 0.0, 0.50).normalize();
        let noon = azimuth * horizontal + Vec3::Y * vertical;
        let evening = Vec3::Y.cross(azimuth);
        let angle = ((self.day_minutes / MINUTES_PER_DAY - 0.5) * TAU) as f32;
        let (sin, cos) = angle.sin_cos();
        noon * cos + evening * sin
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

#[repr(C)]
pub struct Frame {
    pub view_projection: [f32; 16],
    pub model: [f32; 16],
    pub light_projection: [[f32; 16]; SHADOW_CASCADES],
    camera: [f32; 4],
    pub sun: [f32; 4],
    pub grass: [f32; 4],
    pub output: [f32; 4],
    terrain_params: [f32; 4],
    terrain_grid: [u32; 4],
    pub fps_hud: [u32; 4],
    depth_projection: [f32; 4],
    terrain_frustum: [[f32; 4]; 6],
    shadow_ranges: [f32; SHADOW_CASCADES],
    pub cloud_quality: CloudQuality,
}

#[derive(Clone, Copy, PartialEq)]
pub struct TerrainLodView {
    view_projection: [f32; 16],
    camera_and_scale: [f32; 4],
}

impl Frame {
    pub fn new(
        [width, height]: [u32; 2],
        camera: &Camera,
        animation: Animation,
        exposure_ev: f32,
        hdr: bool,
        peak_nits: f32,
        ground: f32,
    ) -> Self {
        let aspect = width as f32 / height as f32;
        // Reverse camera depth so D32_FLOAT keeps enough precision for world-space
        // reconstruction on distant terrain. Light projections stay conventional.
        let projection = Mat4::perspective_rh(
            56f32.to_radians(),
            aspect,
            camera.draw_distance,
            CAMERA_NEAR,
        );
        let view_projection = projection * camera.view();
        let sun = animation.sun_direction();
        // Keep enough shadow texels across the two-metre cube in the near view.
        let extent = (camera.eye.y - ground).abs().clamp(32.0, 3_850.0);
        let light_view = Mat4::look_at_rh(camera.eye + sun * (3.0 * extent), camera.eye, Vec3::Y);
        let light_projection =
            Mat4::orthographic_rh(-extent, extent, -extent, extent, 0.1, 6.0 * extent);
        let model = Mat4::from_translation(Vec3::Y * (ground + CUBE_CLEARANCE + CUBE_HALF_SIZE))
            * Mat4::from_rotation_y(animation.angle as f32);
        Self {
            view_projection: view_projection.to_cols_array(),
            model: model.to_cols_array(),
            light_projection: [(light_projection * light_view).to_cols_array(); SHADOW_CASCADES],
            camera: camera.eye.extend(1.0).to_array(),
            sun: sun.extend(5.0).to_array(),
            grass: [animation.wind_time as f32, 1.0, 1.0, 1.0],
            output: [exposure_ev.exp2(), hdr as u8 as f32, 160.0, peak_nits],
            terrain_params: [
                terrain::GRID_STEP,
                height as f32,
                0.5 * height as f32 * projection.y_axis.y,
                width as f32,
            ],
            terrain_grid: terrain::GRID,
            fps_hud: [0; 4],
            depth_projection: [
                projection.z_axis.z,
                projection.w_axis.z,
                animation.cloud_wind as f32,
                -1.0e6,
            ],
            terrain_frustum: frustum_planes(view_projection).map(|plane| plane.to_array()),
            shadow_ranges: [0.0; SHADOW_CASCADES],
            cloud_quality: CloudQuality::default(),
        }
    }

    pub fn pixels_at_one_meter(&self) -> f32 {
        self.terrain_params[2]
    }

    pub fn configure_alpine(&mut self, camera: &Camera, shadow_size: u32) {
        self.depth_projection[3] = terrain::alpine::LAKE_LEVEL;
        let far = camera.draw_distance;
        self.shadow_ranges = [
            (0.20 * far).min(120.0),
            (0.45 * far).min(650.0),
            (0.72 * far).min(3_200.0),
            far,
        ];
        let sun = Vec3::from_array([self.sun[0], self.sun[1], self.sun[2]]);
        let up = if sun.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
        let basis = Mat4::look_to_rh(Vec3::ZERO, -sun, up);
        let center = basis.transform_point3(camera.eye);
        for (matrix, radius) in self.light_projection.iter_mut().zip(self.shadow_ranges) {
            // Nested spheres also cover off-camera land visible in the lake mirror.
            // Snap in a fixed light basis so camera motion and rotation keep texels stable.
            let extent = radius * 1.02;
            let texel = 2.0 * extent / shadow_size as f32;
            let x = (center.x / texel).round() * texel;
            let y = (center.y / texel).round() * texel;
            // Leave room toward the sun for ridges casting into a nearby slice.
            let projection = Mat4::orthographic_rh(
                x - extent,
                x + extent,
                y - extent,
                y + extent,
                -center.z - extent - 3500.0,
                -center.z + extent,
            );
            *matrix = (projection * basis).to_cols_array();
        }
    }

    pub fn terrain_lod_view(&self) -> TerrainLodView {
        TerrainLodView {
            view_projection: self.view_projection,
            camera_and_scale: [
                self.camera[0],
                self.camera[1],
                self.camera[2],
                self.pixels_at_one_meter(),
            ],
        }
    }
}

// Tessellate only the narrow bevels. The central quad of each face stays flat.
pub fn cube_geometry() -> Vec<Vertex> {
    let mut vertices = Vec::new();
    let axes = [
        (Vec3::X, -Vec3::Z, Vec3::Y),
        (-Vec3::X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, -Vec3::Z),
        (-Vec3::Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (-Vec3::Z, -Vec3::X, Vec3::Y),
    ];
    let radius = 0.055;
    let inner = 1.0 - radius;
    let t = [-1.0, -0.98, -0.96, -inner, inner, 0.96, 0.98, 1.0];
    for (face, u, v) in axes {
        for x in t.windows(2) {
            for y in t.windows(2) {
                for (i, j) in [(0, 0), (1, 0), (1, 1), (0, 0), (1, 1), (0, 1)] {
                    let p = face + u * x[i] + v * y[j];
                    let core = p.clamp(Vec3::splat(-inner), Vec3::splat(inner));
                    let normal = (p - core).normalize();
                    vertices.push(Vertex {
                        position: (CUBE_HALF_SIZE * (core + radius * normal)).to_array(),
                        normal: normal.to_array(),
                    });
                }
            }
        }
    }
    vertices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_depth_reconstructs_distant_shadow_receivers() {
        let camera = Camera::alpine_overlook();
        let frame = Frame::new(
            [1280, 720],
            &camera,
            Animation::new(0.0),
            0.0,
            false,
            80.0,
            0.0,
        );
        let projection = Mat4::from_cols_array(&frame.view_projection) * camera.view().inverse();
        for depth in [CAMERA_NEAR, 1.0, 1_000.0, 8_000.0, camera.draw_distance] {
            let clip = projection * Vec3::new(0.0, 0.0, -depth).extend(1.0);
            let device_depth = (clip.z / clip.w).clamp(0.0, 1.0);
            let [z, w, ..] = frame.depth_projection;
            // Match deferred_ps, including the f32 depth-buffer round trip.
            let reconstructed = w / (device_depth + z);
            assert!(
                (reconstructed - depth).abs() < 0.005,
                "depth {depth}: {reconstructed}"
            );
        }
        let near = projection.project_point3(Vec3::NEG_Z * CAMERA_NEAR).z;
        let far = projection
            .project_point3(Vec3::NEG_Z * camera.draw_distance)
            .z;
        assert!((near - 1.0).abs() < 1e-3 && far.abs() < 1e-6);
    }

    #[test]
    fn alpine_shadow_cascades_cover_their_range_and_snap_to_texels() {
        let mut camera = Camera::alpine_overlook();
        for far in [100.0, 18_000.0] {
            camera.draw_distance = far;
            let mut frame = Frame::new(
                [1280, 720],
                &camera,
                Animation::new(0.0),
                0.0,
                false,
                80.0,
                0.0,
            );
            frame.configure_alpine(&camera, 2048);
            assert_eq!(frame.shadow_ranges[3], far);
            assert!(frame.shadow_ranges.windows(2).all(|pair| pair[0] < pair[1]));
            for (radius, matrix) in frame.shadow_ranges.into_iter().zip(frame.light_projection) {
                let light = Mat4::from_cols_array(&matrix);
                for axis in [Vec3::X, Vec3::Y, Vec3::Z, -Vec3::X, -Vec3::Y, -Vec3::Z] {
                    let q = light.transform_point3(camera.eye + axis * radius);
                    assert!(q.x.abs() <= 1.0 && q.y.abs() <= 1.0 && (0.0..=1.0).contains(&q.z));
                }
            }
            camera.eye = Vec3::ZERO;
            frame.configure_alpine(&camera, 2048);
            let aligned = frame.light_projection;
            camera.eye += Vec3::X * 0.0005;
            frame.configure_alpine(&camera, 2048);
            for (a, b) in aligned.into_iter().zip(frame.light_projection) {
                assert_eq!(&a[12..14], &b[12..14]);
            }
        }
    }

    #[test]
    fn time_of_day_advances_at_simulated_rate_and_wraps() {
        let mut animation = Animation::new(0.0);
        animation.update_time_of_day(5.0);
        assert_eq!(animation.day_minutes, 720.0);
        animation.step_time_of_day();
        assert_eq!(animation.day_minutes, 735.0);
        animation.toggle_time_of_day();
        animation.update_time_of_day(0.25);
        animation.update_time_of_day(2.75);
        assert_eq!(animation.day_minutes, 741.0);
        animation.toggle_time_of_day();
        animation.update_time_of_day(10.0);
        assert_eq!(animation.day_minutes, 741.0);

        animation.day_minutes = 1439.0;
        animation.toggle_time_of_day();
        animation.update_time_of_day(1.0);
        assert_eq!(animation.day_minutes, 1.0);
        animation.day_minutes = 1430.0;
        animation.step_time_of_day();
        assert_eq!(animation.day_minutes, 5.0);
    }

    #[test]
    fn spinning_cube_bottom_stays_two_metres_above_its_terrain_anchor() {
        let ground = terrain::origin_height();
        let cube = cube_geometry();
        for time in [0.0, 1.0, 3.0, 7.0] {
            let frame = Frame::new(
                [1280, 720],
                &Camera::default(),
                Animation::new(time),
                0.0,
                false,
                80.0,
                ground,
            );
            let model = Mat4::from_cols_array(&frame.model);
            let bottom = cube
                .iter()
                .map(|v| model.transform_point3(v.position.into()).y)
                .fold(f32::INFINITY, f32::min);
            assert!((bottom - ground - 2.0).abs() < 1e-5);
        }
    }
}
