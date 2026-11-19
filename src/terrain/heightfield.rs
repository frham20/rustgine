use glam::{IVec2, Vec2, Vec3};

pub const SIDE_METERS: f32 = 5_000.0;
pub const ROOT_GRID: u32 = 8;
pub const MAX_DEPTH: u32 = 12;
pub const ROOT_COUNT: u32 = 2 * ROOT_GRID * ROOT_GRID;
pub const NODES_PER_ROOT: u32 = 2 << MAX_DEPTH;
pub const GRID_CELLS: u32 = ROOT_GRID << (MAX_DEPTH / 2);
pub const GRID_STEP: f32 = SIDE_METERS / GRID_CELLS as f32;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Node {
    pub(super) corners: [u32; 3],
    error: f32,
}

pub struct Heightfield {
    pub samples: Vec<f32>,
    pub nodes: Vec<Node>,
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn smooth_range(start: f32, end: f32, value: f32) -> f32 {
    smooth(((value - start) / (end - start)).clamp(0.0, 1.0))
}

fn lattice(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ seed;
    h = (h ^ (h >> 16)).wrapping_mul(0x7feb_352d);
    h = (h ^ (h >> 15)).wrapping_mul(0x846c_a68b);
    ((h ^ (h >> 16)) >> 8) as f32 / 16_777_215.0
}

fn noise(p: Vec2, seed: u32) -> f32 {
    let cell = p.floor();
    let IVec2 { x, y } = cell.as_ivec2();
    let t = p - cell;
    let (u, v) = (smooth(t.x), smooth(t.y));
    let [a, b, c, d] =
        [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)].map(|(i, j)| lattice(i, j, seed));
    let lo = a + (b - a) * u;
    let hi = c + (d - c) * u;
    lo + (hi - lo) * v
}

fn fractal(mut p: Vec2, seed: u32) -> f32 {
    let mut elevation = 0.0;
    let mut weight = 0.54;
    for octave in 0..5 {
        elevation += weight * noise(p, seed + 137 * octave);
        p = Vec2::new(1.7 * p.x - 1.1 * p.y, 1.1 * p.x + 1.7 * p.y);
        weight *= 0.48;
    }
    elevation
}

pub fn origin_height() -> f32 {
    elevation(Vec2::ZERO)
}

fn mountain(km: Vec2, center: Vec2, widths: Vec2, rise: f32, seed: u32) -> f32 {
    let offset = (km - center) / widths;
    let radius = offset.length();
    if radius >= 1.18 {
        return 0.0;
    }
    let shape = 0.89 + 0.33 * noise(offset * 2.7, seed);
    let foot = (1.0 - radius * shape).max(0.0);
    let ridge = 1.0 - (2.0 * noise(offset * 5.5, seed + 1) - 1.0).abs();
    // Sample around the summit so gullies follow the slopes and branch into spurs.
    let drainage = offset * (3.8 / radius.max(0.1) + 0.95);
    let channel = (2.0 * noise(drainage, seed + 2) - 1.0).abs();
    let gully = (1.0 - smooth_range(0.025, 0.30, channel)) * smooth_range(0.14, 0.42, radius);
    rise * foot * foot.sqrt() * (0.92 + 0.13 * ridge - 0.34 * gully)
}

struct Valley {
    head: Vec2,
    mouth: Vec2,
    width: f32,
    depth: f32,
}

impl Valley {
    fn carve(&self, km: Vec2, seed: u32) -> f32 {
        let reach = self.mouth - self.head;
        let length = reach.length();
        let axis = reach / length;
        let local = km - self.head;
        let along = axis.dot(local) / length;
        let across = axis.perp_dot(local);
        if !(0.0..1.0).contains(&along) || across.abs() > self.width * 3.5 {
            return 0.0;
        }
        let bend = 1.7 * (noise(Vec2::new(5.2 * along, 0.7), seed) - 0.5)
            + 0.7 * (noise(Vec2::new(11.3 * along, 2.1), seed + 1) - 0.5);
        let offset = across / self.width - bend;
        let run = smooth_range(0.0, 0.13, along) * (1.0 - smooth_range(0.86, 1.0, along));
        // The broad valley floor contains a narrower, more deeply incised channel.
        let basin = (-offset * offset).exp();
        let creek = (-(offset / 0.22).powi(2)).exp();
        self.depth * run * (0.72 * basin + 0.28 * creek)
    }
}

fn elevation(p: Vec2) -> f32 {
    // Each hill has a center and elliptical widths in kilometers, with a rise in meters.
    const HILLS: [([f32; 2], [f32; 2], f32); 7] = [
        ([-0.47, -0.22], [0.26, 0.32], 112.0),
        ([-0.12, -0.65], [0.32, 0.31], 92.0),
        ([-1.65, -1.62], [0.74, 0.57], 137.0),
        ([-0.55, -2.20], [0.67, 0.81], 122.0),
        ([1.47, -1.28], [0.83, 0.61], 104.0),
        ([1.25, 1.19], [0.66, 0.91], 115.0),
        ([-1.25, 1.56], [0.66, 0.70], 126.0),
    ];
    const PEAKS: [([f32; 2], [f32; 2], f32); 10] = [
        ([-1.96, -1.12], [0.63, 0.88], 740.0),
        ([-1.56, -1.45], [0.51, 0.66], 476.0),
        ([-1.22, -1.79], [0.47, 0.62], 438.0),
        ([-0.65, -1.86], [0.73, 0.68], 635.0),
        ([-0.34, -1.49], [0.52, 0.54], 310.0),
        ([1.31, -1.54], [0.74, 0.69], 585.0),
        ([1.89, -1.18], [0.58, 0.92], 463.0),
        ([1.39, 1.51], [0.66, 0.70], 513.0),
        ([0.97, 1.06], [0.55, 0.64], 368.0),
        ([-1.48, 1.39], [0.80, 0.63], 423.0),
    ];
    const VALLEYS: [Valley; 5] = [
        Valley {
            head: Vec2::new(-2.58, -2.42),
            mouth: Vec2::new(0.38, 0.40),
            width: 0.24,
            depth: 74.0,
        },
        Valley {
            head: Vec2::new(-1.24, -1.23),
            mouth: Vec2::new(-2.68, 1.42),
            width: 0.21,
            depth: 65.0,
        },
        Valley {
            head: Vec2::new(0.69, -2.67),
            mouth: Vec2::new(0.57, 2.68),
            width: 0.25,
            depth: 80.0,
        },
        Valley {
            head: Vec2::new(2.68, -0.66),
            mouth: Vec2::new(0.14, 0.76),
            width: 0.28,
            depth: 73.0,
        },
        Valley {
            head: Vec2::new(2.65, 2.17),
            mouth: Vec2::new(-0.65, 0.93),
            width: 0.18,
            depth: 62.0,
        },
    ];
    let km = p / 1_000.0;
    let hills: f32 = HILLS
        .iter()
        .map(|&(center, widths, rise)| {
            let offset = (km - Vec2::from_array(center)) / Vec2::from_array(widths);
            rise * (-offset.length_squared()).exp()
        })
        .sum();
    let meadow = 74.0 + hills + 12.0 * noise(km * 1.4, 448) + 2.0 * fractal(km * 12.0, 906);
    // The camera starts above a gentler meadow, with relief increasing around it.
    let relief = smooth_range(0.065, 0.27, km.length());
    if relief == 0.0 {
        return meadow;
    }
    let peaks: f32 = PEAKS
        .iter()
        .enumerate()
        .map(|(i, &(center, widths, rise))| {
            mountain(
                km,
                Vec2::from_array(center),
                Vec2::from_array(widths),
                rise,
                395 + i as u32 * 137,
            )
        })
        .sum();
    let valleys = VALLEYS
        .iter()
        .enumerate()
        .map(|(i, valley)| valley.carve(km, 633 + i as u32 * 101))
        .fold(0.0_f32, f32::max);
    let ruggedness = ((peaks + 0.6 * hills) / 260.0).clamp(0.0, 1.0);
    let hills_between = 44.0 * fractal(km * 3.2, 736);
    let folds = (10.0 + 31.0 * ruggedness) * (fractal(km * 13.0, 183) - 0.5);
    let ledges = (3.0 + 12.0 * ruggedness) * (noise(km * 44.0, 52) - 0.5);
    meadow + relief * (peaks - valleys + hills_between + folds + ledges)
}

impl Heightfield {
    pub fn generate() -> Self {
        let mut field = Self {
            samples: (0..=GRID_CELLS)
                .flat_map(|z| {
                    (0..=GRID_CELLS).map(move |x| {
                        let p = Vec2::new(x as f32, z as f32) * GRID_STEP;
                        elevation(p - Vec2::splat(SIDE_METERS * 0.5))
                    })
                })
                .collect(),
            nodes: vec![Node::default(); (ROOT_COUNT * NODES_PER_ROOT) as usize],
        };
        let tile = (GRID_CELLS / ROOT_GRID) as i32;
        for z in 0..ROOT_GRID {
            for x in 0..ROOT_GRID {
                let nw = IVec2::new(x as i32, z as i32) * tile;
                let sw = nw + IVec2::new(0, tile);
                let se = nw + IVec2::splat(tile);
                let ne = nw + IVec2::new(tile, 0);
                let root = 2 * (z * ROOT_GRID + x);
                field.build(root * NODES_PER_ROOT, 1, [nw, sw, se]);
                field.build((root + 1) * NODES_PER_ROOT, 1, [se, ne, nw]);
            }
        }
        field
    }

    fn sample(&self, p: IVec2) -> f32 {
        self.samples[(p.y as u32 * (GRID_CELLS + 1) + p.x as u32) as usize]
    }

    pub fn vertices(&self) -> Vec<[f32; 4]> {
        let last = IVec2::splat(GRID_CELLS as i32);
        (0..=last.y)
            .flat_map(|z| {
                (0..=last.x).map(move |x| {
                    let p = IVec2::new(x, z);
                    let lo = (p - IVec2::ONE).max(IVec2::ZERO);
                    let hi = (p + IVec2::ONE).min(last);
                    let dx = self.sample(IVec2::new(hi.x, z)) - self.sample(IVec2::new(lo.x, z));
                    let dz = self.sample(IVec2::new(x, hi.y)) - self.sample(IVec2::new(x, lo.y));
                    let normal = Vec3::new(
                        -dx / (hi.x - lo.x) as f32,
                        GRID_STEP,
                        -dz / (hi.y - lo.y) as f32,
                    )
                    .normalize();
                    [self.sample(p), normal.x, normal.y, normal.z]
                })
            })
            .collect()
    }

    fn build(&mut self, root: u32, index: u32, [a, b, c]: [IVec2; 3]) -> f32 {
        let mut error = 0.0;
        if index < 1 << MAX_DEPTH {
            let m = (a + c) / 2;
            let left = self.build(root, index * 2, [b, m, a]);
            let right = self.build(root, index * 2 + 1, [c, m, b]);
            // A split only moves the new midpoint away from the parent's plane.
            error =
                left.max(right) + (self.sample(m) - 0.5 * (self.sample(a) + self.sample(c))).abs();
        }
        self.nodes[(root + index) as usize] = Node {
            corners: [a, b, c].map(|p| p.x as u32 | ((p.y as u32) << 16)),
            error,
        };
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn bisectors_tile_the_heightfield_on_aligned_power_of_two_edges() {
        let field = Heightfield::generate();
        assert_eq!(GRID_CELLS as f32 * GRID_STEP, 5_000.0);
        let mut edges = HashMap::<_, u32>::new();
        let mut double_area = 0;
        for root in 0..ROOT_COUNT {
            for index in 1..NODES_PER_ROOT {
                let node = field.nodes[(root * NODES_PER_ROOT + index) as usize];
                let [a, b, c] = node
                    .corners
                    .map(|p| IVec2::new((p & 65535) as i32, (p >> 16) as i32));
                for (from, to) in [(a, b), (b, c), (c, a)] {
                    let dir = (to - from).abs();
                    let span = dir.max_element();
                    assert!((span as u32).is_power_of_two());
                    assert!(dir.min_element() == 0 || dir.min_element() == span);
                    assert_eq!(from % span, IVec2::ZERO);
                    assert_eq!(to % span, IVec2::ZERO);
                }
                if index < 1 << MAX_DEPTH {
                    continue;
                }
                assert_eq!(node.error, 0.0);
                double_area += (c - a).perp_dot(b - a);
                for i in 0..3 {
                    let (a, b) = (node.corners[i], node.corners[(i + 1) % 3]);
                    let key = (a.min(b), a.max(b));
                    let xy = [a, b].map(|p| [p & 65535, p >> 16]);
                    let boundary = (0..2).any(|axis| {
                        xy[0][axis] == xy[1][axis]
                            && (xy[0][axis] == 0 || xy[0][axis] == GRID_CELLS)
                    });
                    if !boundary {
                        *edges.entry(key).or_insert(0) += 1;
                    }
                }
            }
        }
        assert_eq!(double_area as u32, 2 * GRID_CELLS * GRID_CELLS);
        assert!(edges.values().all(|&count| count == 2));
    }
}
