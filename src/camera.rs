use glam::{Mat4, Vec2, Vec3};
use std::f32::consts::TAU;

const LOOK_SENSITIVITY: f32 = 0.0025;
const PITCH_LIMIT: f32 = 89.0_f32.to_radians();

pub struct Camera {
    pub eye: Vec3,
    pub speed_kmh: f32,
    pub draw_distance: f32,
    yaw: f32,
    pitch: f32,
}

impl Default for Camera {
    fn default() -> Self {
        let ground = crate::terrain::origin_height();
        let eye = Vec3::new(5.5, ground + 3.5, 7.5);
        let target = Vec3::new(0.0, ground + 2.95, 0.0);
        let forward = (target - eye).normalize();
        Self {
            eye,
            speed_kmh: 50.0,
            draw_distance: 18_000.0,
            yaw: forward.x.atan2(-forward.z),
            pitch: forward.y.asin(),
        }
    }
}

impl Camera {
    fn forward(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        Vec3::new(sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch)
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_to_rh(self.eye, self.forward(), Vec3::Y)
    }

    pub fn update(&mut self, movement: Vec2, mouse: Vec2, seconds: f32, boost: bool) {
        if mouse != Vec2::ZERO {
            self.yaw = (self.yaw + mouse.x * LOOK_SENSITIVITY).rem_euclid(TAU);
            self.pitch = (self.pitch - mouse.y * LOOK_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        }
        let movement = movement.clamp_length_max(1.0);
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let right = Vec3::new(cos_yaw, 0.0, sin_yaw);
        let speed = self.speed_kmh / 3.6 * if boost { 2.0 } else { 1.0 };
        self.eye += (right * movement.x + self.forward() * movement.y) * (speed * seconds);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_steers_flight_and_pitch_stays_below_vertical() {
        let mut camera = Camera {
            eye: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            ..Camera::default()
        };
        camera.update(
            Vec2::Y,
            Vec2::new(TAU / (4.0 * LOOK_SENSITIVITY), -100_000.0),
            1.0,
            false,
        );
        assert_eq!(camera.pitch, PITCH_LIMIT);
        assert!(camera.eye.x > 0.0 && camera.eye.y > 3.9);
        assert!(camera.eye.z.abs() < 1e-5 && camera.view().is_finite());
    }

    #[test]
    fn diagonal_flight_doubles_with_boost_and_returns_to_base_speed() {
        let mut camera = Camera::default();
        let start = camera.eye;
        // Repeated f32 position updates round at the scale of the terrain elevation.
        let tolerance = 12.0 * f32::EPSILON * start.abs().max_element();
        let input = Vec2::ONE;
        for _ in 0..10 {
            camera.update(input, Vec2::ZERO, 0.1, false);
        }
        let base_end = camera.eye;
        assert!(((base_end - start).length() - 50.0 / 3.6).abs() < tolerance);

        camera.update(-input, Vec2::ZERO, 1.0, true);
        assert!(((camera.eye - base_end).length() - 100.0 / 3.6).abs() < tolerance);

        camera.update(input, Vec2::ZERO, 1.0, false);
        assert!((camera.eye - start).length() < tolerance);
    }
}
