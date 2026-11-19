use std::time::{Duration, Instant};

pub const MIN_FPS_LIMIT: u32 = 30;
pub const MAX_FPS_LIMIT: u32 = 2_000;
pub const DEFAULT_FPS_LIMIT: u32 = 300;

pub fn pace_frame(start: Instant, max_fps: u32) {
    const SPIN_MARGIN: Duration = Duration::from_micros(150);
    let frame_time = Duration::from_nanos(1_000_000_000_u64.div_ceil(u64::from(max_fps)));

    // Sleep for most of the remaining budget, then finish the short wait at clock precision.
    let remaining = frame_time.saturating_sub(start.elapsed());
    if remaining > SPIN_MARGIN {
        std::thread::sleep(remaining - SPIN_MARGIN);
    }
    while start.elapsed() < frame_time {
        std::hint::spin_loop();
    }
}

pub struct FpsCounter {
    start: Instant,
    frames: u32,
    fps: Option<u32>,
}

impl FpsCounter {
    pub fn new(start: Instant) -> Self {
        Self {
            start,
            frames: 0,
            fps: None,
        }
    }

    pub fn frame_presented(&mut self, now: Instant) {
        self.frames += 1;
        let elapsed = now.duration_since(self.start);
        if elapsed >= Duration::from_millis(500) {
            self.fps = Some((f64::from(self.frames) / elapsed.as_secs_f64()).round() as u32);
            self.frames = 0;
            self.start = now;
        }
    }

    pub fn shader_constants(&self, dpi: u32) -> [u32; 4] {
        // Glyph indices match the font in scene.hlsl.
        const SPACE: u32 = 10;
        const DASH: u32 = 11;
        const F: u32 = 12;
        const P: u32 = 13;
        const S: u32 = 14;
        let mut glyphs = [F, P, S, SPACE, SPACE, SPACE, SPACE, SPACE];
        if let Some(fps) = self.fps {
            let mut value = fps.min(9999);
            for glyph in glyphs[4..].iter_mut().rev() {
                *glyph = value % 10;
                value /= 10;
                if value == 0 {
                    break;
                }
            }
        } else {
            glyphs[6..].fill(DASH);
        }
        let packed = glyphs
            .into_iter()
            .enumerate()
            .fold(0, |word, (i, glyph)| word | (glyph << (4 * i)));
        let scale = ((dpi + 24) / 48).max(1);
        [scale, packed, 0, 0]
    }
}
