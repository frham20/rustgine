use crate::camera::frustum_planes;
use crate::gpu::{buffer, upload};
use crate::pipeline::{SurfacePass, SurfacePipelines};
use crate::terrain::{Terrain, SIDE_METERS};
use glam::{IVec2, Mat4, Vec2, Vec3, Vec4};
use std::{collections::HashMap, fmt, mem::size_of};
use windows::{
    core::{Error, Interface, Result},
    Win32::{
        Foundation::E_INVALIDARG,
        Graphics::{Direct3D12::*, Dxgi::Common::DXGI_FORMAT_R16_UINT},
    },
};

const CELL_SPACING: f32 = 0.56;
const PATCH_RADIUS: f32 = 0.47;
const FADE_DISTANCE: f32 = 3.0;
pub const MIN_DRAW_DISTANCE: f32 = 5.0;
pub const MAX_DRAW_DISTANCE: f32 = 1_000.0;
pub const MAX_DENSITY_PERCENT: u32 = 500;
const DEFAULT_COLOR: [f32; 3] = [0.28, 0.38, 0.18];
const DISTANCES: [f32; 24] = [
    MIN_DRAW_DISTANCE,
    9.0,
    13.0,
    17.0,
    21.0,
    25.0,
    29.0,
    33.0,
    37.0,
    41.0,
    45.0,
    60.0,
    80.0,
    100.0,
    125.0,
    150.0,
    200.0,
    250.0,
    320.0,
    400.0,
    500.0,
    630.0,
    800.0,
    MAX_DRAW_DISTANCE,
];
const MAX_BLADES: usize = 32;
const MAX_WIDTH_SCALE: f32 = 2.0;
const VERTICES_PER_BLADE: u16 = 8;
const INDICES_PER_BLADE: u32 = 18;
const MESH_PATCHES_PER_DISPATCH: usize = 65_535;

struct GrassLod {
    spacing: f32,
    default_end: f32,
    maximum_end: f32,
    transition_width: f32,
}

impl GrassLod {
    fn transition_start(&self, previous_end: f32, end: f32) -> f32 {
        (end - self.transition_width)
            .max(previous_end)
            .max(end * 0.5)
    }
}

const LOD_COUNT: usize = 4;
const MIN_LOD_SPAN: f32 = 1.0;
const LODS: [GrassLod; LOD_COUNT] = [
    GrassLod {
        spacing: CELL_SPACING,
        default_end: 96.0,
        // Bound the size of the finest grids even at their maximum slider values.
        maximum_end: 192.0,
        transition_width: 40.0,
    },
    GrassLod {
        spacing: CELL_SPACING * 4.0,
        default_end: 224.0,
        maximum_end: 448.0,
        transition_width: 24.0,
    },
    GrassLod {
        spacing: CELL_SPACING * 16.0,
        default_end: 640.0,
        maximum_end: MAX_DRAW_DISTANCE - MIN_LOD_SPAN,
        transition_width: 80.0,
    },
    GrassLod {
        spacing: CELL_SPACING * 32.0,
        default_end: MAX_DRAW_DISTANCE,
        maximum_end: MAX_DRAW_DISTANCE,
        transition_width: FADE_DISTANCE,
    },
];
#[derive(Clone, Copy, Default)]
pub enum GrassPath {
    #[default]
    Auto,
    Mesh,
    Instanced,
    Off,
}

impl GrassPath {
    pub fn resolve(self, mesh_supported: bool) -> Result<Self> {
        match self {
            Self::Auto if mesh_supported => Ok(Self::Mesh),
            Self::Auto => Ok(Self::Instanced),
            Self::Mesh if !mesh_supported => Err(Error::new(
                E_INVALIDARG,
                "Mesh grass requires Direct3D 12 mesh shader tier 1 and Shader Model 6.5",
            )),
            _ => Ok(self),
        }
    }
}

impl fmt::Display for GrassPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Auto => "auto",
            Self::Mesh => "mesh",
            Self::Instanced => "instanced",
            Self::Off => "off",
        })
    }
}

// Matches the 32-byte HLSL Patch and preserves alignment when binding an LOD bucket.
#[repr(C)]
#[derive(Clone, Copy)]
struct Patch {
    center: [f32; 2],
    radius: f32,
    height: f32,
    seed: u32,
    blade_count: f32,
    fade: f32,
    width_scale: f32,
}

fn grid_radius(distance: f32, spacing: f32) -> i32 {
    // Rounding the camera cell and jittering the patch can each shift it by half a cell.
    (distance / spacing).ceil() as i32 + 1
}

#[derive(Default)]
struct PatchGrid {
    tiles: HashMap<IVec2, GrassTile>,
    bounds: Option<(IVec2, IVec2)>,
}

const TILE_SIDE: usize = 16;

struct GrassTile {
    patches: Vec<(Patch, Vec3)>,
    row_offsets: [usize; TILE_SIDE + 1],
    min: Vec3,
    max: Vec3,
}

impl GrassTile {
    fn new(tile: IVec2, level: usize, lod: &GrassLod, terrain: &Terrain) -> Self {
        let mut patches = Vec::with_capacity(TILE_SIDE * TILE_SIDE);
        let mut row_offsets = [0; TILE_SIDE + 1];
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        let corner = tile * TILE_SIDE as i32;
        for (z, end) in row_offsets[1..].iter_mut().enumerate() {
            for x in 0..TILE_SIDE {
                if let Some(patch) = make_patch(corner + IVec2::new(x as i32, z as i32), level, lod)
                {
                    let xz = Vec2::from_array(patch.center);
                    let center = Vec3::new(xz.x, terrain.approximate_height(xz), xz.y);
                    min = min.min(center);
                    max = max.max(center);
                    patches.push((patch, center));
                }
            }
            *end = patches.len();
        }
        Self {
            patches,
            row_offsets,
            min,
            max,
        }
    }

    fn visible(&self, eye: Vec2, limit_squared: f32, planes: &[Vec4; 6], reach: f32) -> bool {
        if self.patches.is_empty() {
            return false;
        }
        let min_xz = Vec2::new(self.min.x, self.min.z);
        let max_xz = Vec2::new(self.max.x, self.max.z);
        eye.clamp(min_xz, max_xz).distance_squared(eye) < limit_squared
            && planes.iter().all(|plane| {
                let corner = Vec3::select(plane.truncate().cmpge(Vec3::ZERO), self.max, self.min);
                plane.dot(corner.extend(1.0)) >= -reach - 0.001
            })
    }

    fn row(&self, z: usize) -> &[(Patch, Vec3)] {
        &self.patches[self.row_offsets[z]..self.row_offsets[z + 1]]
    }
}

impl PatchGrid {
    fn update(
        &mut self,
        cell: IVec2,
        radius: i32,
        level: usize,
        lod: &GrassLod,
        terrain: &Terrain,
    ) -> (IVec2, IVec2) {
        let tile_side = IVec2::splat(TILE_SIDE as i32);
        let first = (cell - radius).div_euclid(tile_side);
        let last = (cell + radius).div_euclid(tile_side);
        if self.bounds != Some((first, last)) {
            // World tiles and their sampled heights remain valid as the camera crosses cells.
            self.tiles
                .retain(|&p, _| p.cmpge(first).all() && p.cmple(last).all());
            for z in first.y..=last.y {
                for x in first.x..=last.x {
                    let tile = IVec2::new(x, z);
                    self.tiles
                        .entry(tile)
                        .or_insert_with(|| GrassTile::new(tile, level, lod, terrain));
                }
            }
            self.bounds = Some((first, last));
        }
        (first, last)
    }
}

fn hash_patch(mut value: u32) -> u32 {
    value = (value ^ (value >> 16)).wrapping_mul(0x7feb352d);
    value = (value ^ (value >> 15)).wrapping_mul(0x846ca68b);
    value ^ (value >> 16)
}

fn make_patch(cell: IVec2, level: usize, lod: &GrassLod) -> Option<Patch> {
    let patch_radius = PATCH_RADIUS.max(lod.spacing * 0.4);
    let lod_salt = (level as u32).wrapping_mul(0x9e3779b9);
    let seed = (cell.x as u32).wrapping_mul(0x8da6b343)
        ^ (cell.y as u32).wrapping_mul(0xd8163841)
        ^ lod_salt;
    // Jitter across the entire cell, with independent patterns at each grid spacing.
    let noise = hash_patch(seed ^ 0x25b1ce3d);
    let jitter =
        (Vec2::new((noise & 0xffff) as f32, (noise >> 16) as f32) + 0.5) * (1.0 / 65_536.0) - 0.5;
    let center = (cell.as_vec2() + jitter) * lod.spacing;
    if center.abs().max_element() + patch_radius > SIDE_METERS * 0.5 {
        return None;
    }
    let height_noise = (hash_patch(seed ^ 0x7301a2e7) >> 16) as f32 / 65_535.0;
    Some(Patch {
        center: center.to_array(),
        radius: patch_radius,
        height: 0.30 + 0.12 * height_noise,
        seed,
        blade_count: MAX_BLADES as f32,
        fade: 1.0,
        width_scale: 1.0,
    })
}

fn smooth_step(start: f32, end: f32, value: f32) -> f32 {
    let t = ((value - start) / (end - start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn blades_at_distance(distance: f32) -> f32 {
    // Keep more of the slender blades in the midground where their shapes are still visible.
    MAX_BLADES as f32
        - (MAX_BLADES as f32 - 6.0) * smooth_step(4.0, 19.0, distance)
        - 4.0 * smooth_step(20.0, 48.0, distance)
}

fn blades_in_lod_blend(blade_count: f32, coverage: f32, seed: u32) -> f32 {
    if coverage >= 1.0 {
        return blade_count;
    }
    // A fixed random offset varies where each patch sheds full blades as the camera moves.
    let hash = seed ^ 0xa511e9b3;
    let hash = (hash ^ (hash >> 16)).wrapping_mul(0x7feb352d);
    let offset = (hash >> 8) as f32 * (1.0 / 16_777_216.0);
    let whole = (blade_count.floor() * coverage + offset).floor();
    let partial = if offset < coverage {
        blade_count.fract()
    } else {
        0.0
    };
    whole + partial
}

pub enum GrassPipeline {
    Mesh(SurfacePipelines),
    Instanced(SurfacePipelines),
}

enum Backend {
    Mesh(ID3D12GraphicsCommandList6),
    Instanced {
        _index_buffer: ID3D12Resource,
        view: D3D12_INDEX_BUFFER_VIEW,
    },
}

pub struct Grass {
    backend: Backend,
    pipeline: SurfacePipelines,
    patches: Vec<Option<ID3D12Resource>>,
    grids: [PatchGrid; LOD_COUNT],
    visible: Vec<Patch>,
    buckets: [u32; MAX_BLADES + 1],
    full_density: bool,
    pub settings: GrassSettings,
}

pub struct GrassSettings {
    pub density_percent: u32,
    pub draw_distance: f32,
    pub height_percent: u32,
    pub color: [f32; 3],
    lod_ends: [f32; LOD_COUNT],
}

impl Default for GrassSettings {
    fn default() -> Self {
        Self {
            density_percent: 100,
            draw_distance: MAX_DRAW_DISTANCE,
            height_percent: 100,
            color: DEFAULT_COLOR,
            lod_ends: LODS.map(|lod| lod.default_end),
        }
    }
}

impl GrassSettings {
    pub fn lod_ends(&self) -> [f32; LOD_COUNT] {
        self.lod_ends
    }

    pub fn lod_end_bounds(&self, level: usize) -> [f32; 2] {
        let min = if level == 0 {
            MIN_DRAW_DISTANCE
        } else {
            self.lod_ends[level - 1] + MIN_LOD_SPAN
        };
        let mut max = LODS[level].maximum_end;
        if let Some(next) = self.lod_ends.get(level + 1) {
            max = max.min(next - MIN_LOD_SPAN);
        }
        [min, max]
    }

    pub fn set_lod_end(&mut self, level: usize, end: f32) {
        let [min, max] = self.lod_end_bounds(level);
        self.lod_ends[level] = end.clamp(min, max);
    }

    pub fn color_scale(&self) -> [f32; 3] {
        let linear = |s: f32| {
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        std::array::from_fn(|i| linear(self.color[i]) / linear(DEFAULT_COLOR[i]))
    }
}

impl Grass {
    pub unsafe fn new(
        device: &ID3D12Device,
        list: &ID3D12GraphicsCommandList,
        pipeline: GrassPipeline,
        frame_count: usize,
        full_density: bool,
    ) -> Result<Self> {
        let (backend, pipeline) = match pipeline {
            GrassPipeline::Mesh(pso) => (Backend::Mesh(list.cast()?), pso),
            GrassPipeline::Instanced(pso) => {
                let mut indices = Vec::with_capacity(MAX_BLADES * INDICES_PER_BLADE as usize);
                for blade in 0..MAX_BLADES as u16 {
                    for row in 0..3 {
                        let i = blade * VERTICES_PER_BLADE + row * 2;
                        indices.extend([i, i + 1, i + 2, i + 2, i + 1, i + 3]);
                    }
                }
                let bytes = (indices.len() * size_of::<u16>()) as u32;
                let index_buffer = buffer(
                    device,
                    bytes.into(),
                    D3D12_HEAP_TYPE_UPLOAD,
                    D3D12_RESOURCE_STATE_GENERIC_READ,
                )?;
                upload(&index_buffer, &indices)?;
                let view = D3D12_INDEX_BUFFER_VIEW {
                    BufferLocation: index_buffer.GetGPUVirtualAddress(),
                    SizeInBytes: bytes,
                    Format: DXGI_FORMAT_R16_UINT,
                };
                (
                    Backend::Instanced {
                        _index_buffer: index_buffer,
                        view,
                    },
                    pso,
                )
            }
        };
        Ok(Self {
            backend,
            pipeline,
            patches: vec![None; frame_count],
            visible: Vec::new(),
            grids: Default::default(),
            buckets: [0; MAX_BLADES + 1],
            full_density,
            settings: GrassSettings::default(),
        })
    }

    pub fn adjust_distance(&mut self, increase: bool) {
        let distance = self.draw_distance();
        let index = if increase {
            DISTANCES
                .partition_point(|&step| step <= distance)
                .min(DISTANCES.len() - 1)
        } else {
            DISTANCES
                .partition_point(|&step| step < distance)
                .saturating_sub(1)
        };
        self.settings.draw_distance = DISTANCES[index];
    }

    pub fn draw_distance(&self) -> f32 {
        self.settings.draw_distance
    }

    pub unsafe fn prepare(
        &mut self,
        device: &ID3D12Device,
        frame_index: usize,
        view_projection: Mat4,
        eye: Vec3,
        pixels_at_one_meter: f32,
        terrain: &Terrain,
    ) -> Result<()> {
        let planes = frustum_planes(view_projection);
        self.visible.clear();
        self.buckets.fill(0);
        let xz = Vec2::new(eye.x, eye.z);
        let distance = self
            .draw_distance()
            .min(self.settings.lod_ends[LOD_COUNT - 1]);
        let density = self.settings.density_percent as f32 / 100.0;
        if density == 0.0 || xz.abs().max_element() > SIDE_METERS * 0.5 + distance {
            return Ok(());
        }
        let height_scale = self.settings.height_percent as f32 / 100.0;
        let mut fade_in = [-1.0, 0.0];
        let mut visible_tiles = Vec::new();
        for (level, (grid, lod)) in self.grids.iter_mut().zip(&LODS).enumerate() {
            let end = self.settings.lod_ends[level];
            let limit = distance.min(end);
            if limit <= fade_in[0] {
                break;
            }
            let cell = (xz / lod.spacing).round().as_ivec2();
            let radius = grid_radius(limit, lod.spacing);
            let (first, last) = grid.update(cell, radius, level, lod, terrain);
            let limit_squared = limit * limit;
            // Include scattering, bend and the widest ribbon in the frustum bound.
            let reach = PATCH_RADIUS.max(lod.spacing * 0.4) + 1.05 * height_scale.max(1.0);
            let fade_out_start = lod.transition_start(fade_in[1], end);
            let final_level = level + 1 == LOD_COUNT;
            let has_draw_fade = final_level || distance - FADE_DISTANCE < end;
            let density_multiplier = if level == 0 || self.full_density {
                1.0
            } else {
                2.0
            };
            for z in first.y..=last.y {
                visible_tiles.clear();
                for x in first.x..=last.x {
                    let tile = &grid.tiles[&IVec2::new(x, z)];
                    if tile.visible(xz, limit_squared, &planes, reach) {
                        visible_tiles.push(tile);
                    }
                }
                // Keep the world-cell draw order for blades with coincident depth.
                let patches = (0..TILE_SIDE)
                    .flat_map(|row| visible_tiles.iter().flat_map(move |tile| tile.row(row)));
                for &(patch, center) in patches {
                    let patch_xz = Vec2::from_array(patch.center);
                    if patch_xz.distance_squared(xz) >= limit_squared {
                        continue;
                    }
                    if planes
                        .iter()
                        .any(|plane| plane.dot(center.extend(1.0)) < -reach)
                    {
                        continue;
                    }
                    let patch_distance = center.distance(eye);
                    let incoming = smooth_step(fade_in[0], fade_in[1], patch_distance);
                    let outgoing = if final_level {
                        1.0
                    } else {
                        1.0 - smooth_step(fade_out_start, end, patch_distance)
                    };
                    let coverage = incoming * outgoing;
                    if coverage == 0.0 {
                        continue;
                    }
                    let height = patch.height * height_scale;
                    let projected_height = height * pixels_at_one_meter / patch_distance.max(1.0);
                    // Reduce the silhouette gradually as it becomes too small to resolve.
                    let mut fade = smooth_step(0.16, 2.0, projected_height);
                    if has_draw_fade {
                        fade *=
                            1.0 - smooth_step(distance - FADE_DISTANCE, distance, patch_distance);
                    }
                    if fade < 0.005 {
                        continue;
                    }
                    let base_blades = if self.full_density {
                        MAX_BLADES as f32
                    } else {
                        blades_at_distance(patch_distance)
                    };
                    // Crossfade the populations so surviving blades keep their silhouette.
                    let mut remaining = blades_in_lod_blend(
                        base_blades * density_multiplier * density,
                        coverage,
                        patch.seed,
                    );
                    let mut seed = patch.seed;
                    let width_scale = (MAX_BLADES as f32 / base_blades).min(MAX_WIDTH_SCALE);
                    // Mesh workgroups and the fallback index buffer each hold up to 32 blades.
                    // Add batches with distinct seeds so higher density also affects nearby grass.
                    while remaining > 0.0 {
                        let blade_count = remaining.min(MAX_BLADES as f32);
                        self.buckets[blade_count.ceil() as usize] += 1;
                        self.visible.push(Patch {
                            height,
                            seed,
                            blade_count,
                            fade,
                            width_scale,
                            ..patch
                        });
                        remaining -= blade_count;
                        seed = seed.wrapping_add(0x9e3779b9);
                    }
                }
            }
            fade_in = [fade_out_start, end];
        }
        if !self.visible.is_empty() {
            if matches!(self.backend, Backend::Instanced { .. }) {
                self.visible
                    .sort_unstable_by_key(|patch| patch.blade_count.ceil() as u32);
            }
            // The renderer has waited for this frame slot before allowing it to grow.
            let bytes = size_of_val(self.visible.as_slice()) as u64;
            let allocation = &mut self.patches[frame_index];
            if allocation
                .as_ref()
                .is_none_or(|buffer| buffer.GetDesc().Width < bytes)
            {
                *allocation = Some(buffer(
                    device,
                    bytes.next_power_of_two(),
                    D3D12_HEAP_TYPE_UPLOAD,
                    D3D12_RESOURCE_STATE_GENERIC_READ,
                )?);
            }
            upload(allocation.as_ref().unwrap(), &self.visible)?;
        }
        Ok(())
    }

    pub unsafe fn draw(
        &self,
        list: &ID3D12GraphicsCommandList,
        frame_index: usize,
        pass: SurfacePass,
    ) {
        if self.visible.is_empty() {
            return;
        }
        let address = self.patches[frame_index]
            .as_ref()
            .unwrap()
            .GetGPUVirtualAddress();
        list.SetPipelineState(self.pipeline.get(pass));
        match &self.backend {
            Backend::Mesh(mesh) => {
                // Each patch takes one workgroup, and the larger grids can exceed one dispatch.
                let mut start = address;
                for batch in self.visible.chunks(MESH_PATCHES_PER_DISPATCH) {
                    list.SetGraphicsRootShaderResourceView(4, start);
                    mesh.DispatchMesh(batch.len() as u32, 1, 1);
                    start += size_of_val(batch) as u64;
                }
            }
            Backend::Instanced { view, .. } => {
                list.IASetIndexBuffer(Some(view));
                let mut start = address;
                for (blades, &instances) in self.buckets.iter().enumerate().skip(1) {
                    if instances != 0 {
                        list.SetGraphicsRootShaderResourceView(4, start);
                        list.DrawIndexedInstanced(
                            blades as u32 * INDICES_PER_BLADE,
                            instances,
                            0,
                            0,
                            0,
                        );
                        start += u64::from(instances) * size_of::<Patch>() as u64;
                    }
                }
                list.IASetIndexBuffer(None);
            }
        }
    }

    pub fn counts(&self) -> (usize, u32) {
        let blades: u32 = self
            .buckets
            .iter()
            .enumerate()
            .map(|(blades, &patches)| blades as u32 * patches)
            .sum();
        (self.visible.len(), blades * (INDICES_PER_BLADE / 3))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grass_lod_sliders_keep_their_ends_ordered_and_bounded() {
        let mut settings = GrassSettings::default();
        assert_eq!(settings.lod_ends(), [96.0, 224.0, 640.0, 1000.0]);

        settings.set_lod_end(0, MAX_DRAW_DISTANCE);
        settings.set_lod_end(1, 0.0);
        settings.set_lod_end(3, 0.0);
        settings.set_lod_end(2, MAX_DRAW_DISTANCE);
        assert_eq!(settings.lod_ends(), [192.0, 193.0, 640.0, 641.0]);

        settings.set_lod_end(3, MAX_DRAW_DISTANCE * 2.0);
        settings.set_lod_end(2, MAX_DRAW_DISTANCE * 2.0);
        settings.set_lod_end(1, MAX_DRAW_DISTANCE * 2.0);
        assert_eq!(settings.lod_ends(), [192.0, 448.0, 999.0, 1000.0]);
    }

    #[test]
    fn handoff_changes_blade_coverage_gradually() {
        assert_eq!(LODS[0].transition_start(0.0, 96.0), 56.0);
        assert_eq!(LODS[0].transition_start(0.0, 5.0), 2.5);

        // Check both the default distant blade count and a fractional density setting.
        for count in [2.0, 6.5] {
            let mut totals = [0.0; 5];
            for seed in 0_u32..2048 {
                let mut previous = 0.0;
                for (i, total) in totals.iter_mut().enumerate() {
                    let selected = blades_in_lod_blend(count, i as f32 / 4.0, seed);
                    assert!(selected >= previous && selected <= count);
                    *total += selected;
                    previous = selected;
                }
            }
            for (i, total) in totals.into_iter().enumerate() {
                let expected = count * i as f32 / 4.0;
                assert!((total / 2048.0 - expected).abs() < 0.05);
            }
        }
    }

    #[test]
    fn patch_jitter_uses_the_full_cell_and_stays_fixed_when_the_grid_moves() {
        const RADIUS: i32 = 8;
        let side = (2 * RADIUS + 1) as usize;
        for (level, lod) in LODS.iter().enumerate() {
            let grid = |center: IVec2| {
                (-RADIUS..=RADIUS)
                    .flat_map(|z| {
                        (-RADIUS..=RADIUS)
                            .filter_map(move |x| make_patch(center + IVec2::new(x, z), level, lod))
                    })
                    .collect::<Vec<_>>()
            };
            let patches = grid(IVec2::ZERO);
            let shifted = grid(IVec2::X);
            let mut min = Vec2::ZERO;
            let mut max = Vec2::ZERO;
            for (index, patch) in patches.iter().enumerate() {
                let (x, z) = (index % side, index / side);
                let cell = Vec2::new(x as f32, z as f32) - RADIUS as f32;
                let jitter = Vec2::from_array(patch.center) / lod.spacing - cell;
                assert!(jitter.abs().max_element() < 0.5);
                min = min.min(jitter);
                max = max.max(jitter);
                if x > 0 {
                    assert_eq!(patch.center, shifted[index - 1].center);
                    assert_eq!(patch.seed, shifted[index - 1].seed);
                }
            }
            assert!(min.max_element() <= -0.45);
            assert!(max.min_element() >= 0.45);
        }
    }
}
