use super::heightfield::{fractal, noise};
use glam::Vec2;

pub const LAKE_LEVEL: f32 = 165.0;
pub const OVERLOOK: Vec2 = Vec2::new(1020.0, 1520.0);

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// Cross-sections of a single closed glacial trough. The staggered spurs on the
// two banks narrow the water without cutting the lake into separate pools.
pub fn height(p: Vec2) -> f32 {
    let k = p * 0.001;
    let z = k.y;
    let center = -0.13 + 0.33 * (2.35 * z - 0.50).sin() + 0.13 * (4.1 * z + 0.5).sin();
    let end = ((z + 0.06) / 2.28).abs();
    let width = (0.62 + 0.12 * (2.9 * z + 1.1).cos()) * (1.0 - end.powi(4)).max(0.0).sqrt();
    let edge = (k.x - center).abs() - width
        + (end - 1.0).max(0.0) * 2.3
        + (noise(k * 7.0, 31) - 0.5) * 0.045;
    if edge < 0.0 {
        return LAKE_LEVEL - 72.0 * (1.0 - (edge * 6.0).exp());
    }
    let east = smooth(-0.15, 0.15, k.x - center);
    let bank = (0.43 + 0.12 * (z * 3.0 + east).cos()) * 1000.0 * edge;
    let crest = 1.15 + 0.26 * (2.5 * z + east * 2.0).sin();
    let ridge = (-((edge - crest) / 0.46).powi(2)).exp();
    let summits = 180.0
        + 340.0 * fractal(k * 2.0, 741)
        + 140.0 * (1.0 - (2.0 * noise(k * 5.0, 113) - 1.0).abs());
    let relief = smooth(0.02, 0.65, edge);
    let folds = (fractal(k * 7.5, 215) - 0.5) * (62.0 + ridge * 125.0);
    let bank_height = 450.0 * (1.0 - (-bank / 450.0).exp()) + relief * (ridge * summits + folds);
    // Keep a low saddle at the head of the lake, opening a view of the outer ranges.
    let saddle = smooth(0.75, 2.35, -z) * (1.0 - smooth(0.25, 1.65, k.x.abs()));
    let land = bank_height * (1.0 - 0.74 * saddle);
    // A glacial bench on the east bank falls away toward the distant water.
    // Blend back into the natural shoreline before the bench reaches the lake.
    let offset = k - OVERLOOK * 0.001;
    let bench = (1.0 - smooth(0.4, 1.18, (offset / Vec2::new(0.83, 1.25)).length()))
        * smooth(0.03, 0.25, edge);
    let meadow = (352.0
        + 460.0 * offset.x
        + 195.0 * offset.y
        + 18.0 * (3.2 * offset.y + 0.8).sin() * (4.0 * offset.x).sin()
        + 7.0 * (fractal(k * 11.0, 940) - 0.5))
        .max(5.0);
    LAKE_LEVEL + land + bench * (meadow - land)
}

// World-space elevation beyond the playable heightfield. Each belt has its own
// crest and saddles, so the near and far silhouettes separate as the camera moves.
pub fn outer_height(p: Vec2) -> f32 {
    let k = p * 0.001;
    let radius = k.abs().max_element();
    let blend = smooth(2.50, 3.18, radius);
    let warp = (fractal(k * 0.33, 87) - 0.5) * 2.0;
    let rugged = fractal(k * 0.92, 519);
    let crest = |distance: f32, width: f32| (-((radius + warp - distance) / width).powi(2)).exp();
    let far = 175.0
        + 160.0 * rugged
        + crest(4.7, 0.85) * (260.0 + 900.0 * rugged)
        + crest(8.0, 1.1) * (650.0 + 1800.0 * rugged)
        + crest(12.8, 1.35) * (1060.0 + 2350.0 * rugged);
    if blend == 1.0 {
        return far;
    }
    height(p) * (1.0 - blend) + far * blend
}

pub fn forest_density(p: Vec2, altitude: f32, slope: f32) -> f32 {
    let stand = 0.65 * noise(p * 0.0036, 811) + 0.35 * noise(p * 0.012, 207);
    let timberline = 575.0 + 145.0 * stand;
    let offset = p - OVERLOOK;
    let clearing = smooth(
        100.0,
        355.0,
        (offset / Vec2::new(1.55, 1.0)).length() - 0.33 * offset.x + 95.0 * (stand - 0.5),
    );
    smooth(LAKE_LEVEL + 2.0, LAKE_LEVEL + 13.0, altitude)
        * (1.0 - smooth(timberline - 95.0, timberline, altitude))
        * (1.0 - smooth(0.68, 1.05, slope))
        * smooth(0.17, 0.55, stand)
        * clearing
}

pub const LANDFORM_SIDE: usize = 256;

// Texel centers cover the basin and the outer ranges independently. Channels
// record shelter, uphill scree sources and the density used to scatter trees.
pub fn landform_maps(terrain: &super::Terrain) -> [Vec<u8>; 2] {
    const SIDE: i32 = LANDFORM_SIDE as i32;
    [(super::SIDE_METERS, 3, 8), (32_000.0, 1, 3)].map(|(span, near, far)| {
        let mut map = Vec::with_capacity(4 * LANDFORM_SIDE * LANDFORM_SIDE);
        let step = span / SIDE as f32;
        let world = |x, z| (Vec2::new(x as f32, z as f32) + 0.5) * step - span * 0.5;
        let mut heights = Vec::with_capacity((SIDE * SIDE) as usize);
        for z in 0..SIDE {
            for x in 0..SIDE {
                let p = world(x, z);
                heights.push(if p.abs().max_element() <= super::SIDE_METERS * 0.5 {
                    terrain.approximate_height(p)
                } else {
                    outer_height(p)
                });
            }
        }
        let height =
            |x: i32, z: i32| heights[(z.clamp(0, SIDE - 1) * SIDE + x.clamp(0, SIDE - 1)) as usize];
        for z in 0..SIDE {
            for x in 0..SIDE {
                let center = height(x, z);
                let mut hollow = 0.0;
                let mut cliff = 0.0_f32;
                for (dx, dz) in [
                    (1, 0),
                    (-1, 0),
                    (0, 1),
                    (0, -1),
                    (1, 1),
                    (1, -1),
                    (-1, 1),
                    (-1, -1),
                ] {
                    let scale = step * (dx as f32).hypot(dz as f32);
                    let mid = height(x + near * dx, z + near * dz);
                    let high = height(x + far * dx, z + far * dz);
                    hollow += 0.55 * (mid - center) / (scale * near as f32)
                        + 0.45 * (high - center) / (scale * far as f32);
                    if mid > center {
                        cliff = cliff.max((high - mid) / (scale * (far - near) as f32));
                    }
                }
                let shelter = smooth(-0.04, 0.20, hollow / 8.0);
                let source = smooth(0.47, 1.02, cliff);
                let canopy = if span == super::SIDE_METERS {
                    let dx = height(x + 1, z) - height(x - 1, z);
                    let dz = height(x, z + 1) - height(x, z - 1);
                    forest_density(world(x, z), center, dx.hypot(dz) / (2.0 * step))
                } else {
                    0.0
                };
                map.extend([shelter, source, canopy, 0.0].map(|v| (255.0 * v).round() as u8));
            }
        }
        map
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lake_is_enclosed_and_the_overlook_stays_clear() {
        let edge = super::super::SIDE_METERS / 2.0;
        for i in -125..=125 {
            let t = i as f32 * 20.0;
            for side in [-edge, edge] {
                assert!(height(Vec2::new(t, side)) > LAKE_LEVEL);
                assert!(height(Vec2::new(side, t)) > LAKE_LEVEL);
            }
        }
        for i in -100..=100 {
            let z = i as f32 * 20.0;
            assert!((-50..50).any(|x| height(Vec2::new(x as f32 * 20.0, z)) < LAKE_LEVEL));
        }
        assert!(height(OVERLOOK) > LAKE_LEVEL + 100.0);
        assert_eq!(forest_density(OVERLOOK, height(OVERLOOK), 0.25), 0.0);
        assert_eq!(forest_density(Vec2::ZERO, LAKE_LEVEL, 0.0), 0.0);
    }
}
