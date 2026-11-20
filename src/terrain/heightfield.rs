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

fn lattice(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ seed;
    h = (h ^ (h >> 16)).wrapping_mul(0x7feb_352d);
    h = (h ^ (h >> 15)).wrapping_mul(0x846c_a68b);
    ((h ^ (h >> 16)) >> 8) as f32 / 16_777_215.0
}

pub(super) fn noise(p: Vec2, seed: u32) -> f32 {
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

pub(super) fn fractal(mut p: Vec2, seed: u32) -> f32 {
    let mut elevation = 0.0;
    let mut weight = 0.54;
    for octave in 0..5 {
        elevation += weight * noise(p, seed + 137 * octave);
        p = Vec2::new(1.7 * p.x - 1.1 * p.y, 1.1 * p.x + 1.7 * p.y);
        weight *= 0.48;
    }
    elevation
}

impl Heightfield {
    pub fn generate() -> Self {
        let mut field = Self {
            samples: (0..=GRID_CELLS)
                .flat_map(|z| {
                    (0..=GRID_CELLS).map(move |x| {
                        let p = Vec2::new(x as f32, z as f32) * GRID_STEP;
                        super::alpine::height(p - Vec2::splat(SIDE_METERS * 0.5))
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
