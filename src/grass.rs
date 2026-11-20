mod compute;

use self::compute::{GrassCompute, GrassWork, ARGUMENT_OFFSET};
use crate::{
    gpu::{buffer, upload},
    pipeline::{SurfacePass, SurfacePipelines, GRASS_PATCHES_ROOT, GRASS_STATS_ROOT},
    terrain::Terrain,
};
use std::fmt;
use windows::{
    core::{Error, Result},
    Win32::{
        Foundation::E_INVALIDARG,
        Graphics::{Direct3D12::*, Dxgi::Common::DXGI_FORMAT_R16_UINT},
    },
};

const CELL_SPACING: f32 = 0.56;
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
struct GrassLod {
    spacing: f32,
    default_end: f32,
    maximum_end: f32,
    transition_width: f32,
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
        transition_width: 3.0,
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

const MAX_BLADES: u32 = 32;
const VERTICES_PER_BLADE: u16 = 8;
const INDICES_PER_BLADE: u32 = 18;

pub enum GrassPipeline {
    Mesh(SurfacePipelines),
    Instanced(SurfacePipelines),
}

struct IndexedGrass {
    _indices: ID3D12Resource,
    view: D3D12_INDEX_BUFFER_VIEW,
}

pub struct Grass {
    compute: GrassCompute,
    commands: ID3D12CommandSignature,
    pipeline: SurfacePipelines,
    indexed: Option<IndexedGrass>,
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
        root: &ID3D12RootSignature,
        pipeline: GrassPipeline,
        frame_count: usize,
        full_density: bool,
    ) -> Result<Self> {
        let (pipeline, indexed) = match pipeline {
            GrassPipeline::Mesh(pipeline) => (pipeline, None),
            GrassPipeline::Instanced(pipeline) => {
                let mut indices = Vec::with_capacity((MAX_BLADES * INDICES_PER_BLADE) as usize);
                for blade in 0..MAX_BLADES as u16 {
                    for row in 0..3 {
                        let i = blade * VERTICES_PER_BLADE + row * 2;
                        indices.extend([i, i + 1, i + 2, i + 2, i + 1, i + 3]);
                    }
                }
                let bytes = size_of_val(indices.as_slice()) as u32;
                let resource = buffer(
                    device,
                    bytes.into(),
                    D3D12_HEAP_TYPE_UPLOAD,
                    D3D12_RESOURCE_STATE_GENERIC_READ,
                )?;
                upload(&resource, &indices)?;
                let view = D3D12_INDEX_BUFFER_VIEW {
                    BufferLocation: resource.GetGPUVirtualAddress(),
                    SizeInBytes: bytes,
                    Format: DXGI_FORMAT_R16_UINT,
                };
                (
                    pipeline,
                    Some(IndexedGrass {
                        _indices: resource,
                        view,
                    }),
                )
            }
        };

        let commands = crate::pipeline::instance_commands(device, root, indexed.is_none())?;

        Ok(Self {
            compute: GrassCompute::new(device, frame_count)?,
            commands,
            pipeline,
            indexed,
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
        list: &ID3D12GraphicsCommandList,
        frame_index: usize,
        constants: u64,
        terrain: &Terrain,
    ) -> Result<()> {
        let work = GrassWork::new(&self.settings, self.full_density, self.indexed.is_none());
        self.compute
            .record(device, list, frame_index, constants, terrain, &work)
    }

    pub unsafe fn draw(
        &self,
        list: &ID3D12GraphicsCommandList,
        frame_index: usize,
        pass: SurfacePass,
    ) {
        let Some(output) = self.compute.output(frame_index) else {
            return;
        };
        list.SetPipelineState(self.pipeline.get(pass));
        list.SetGraphicsRootShaderResourceView(
            GRASS_PATCHES_ROOT,
            output.patches.GetGPUVirtualAddress(),
        );
        list.SetGraphicsRootShaderResourceView(
            GRASS_STATS_ROOT,
            output.draw_data.GetGPUVirtualAddress(),
        );
        if let Some(indexed) = &self.indexed {
            list.IASetIndexBuffer(Some(&indexed.view));
            list.ExecuteIndirect(
                &self.commands,
                MAX_BLADES,
                &output.draw_data,
                ARGUMENT_OFFSET,
                None,
                0,
            );
            list.IASetIndexBuffer(None);
        } else {
            list.ExecuteIndirect(
                &self.commands,
                1,
                &output.draw_data,
                ARGUMENT_OFFSET,
                None,
                0,
            );
        }
    }

    pub unsafe fn capture_counts(
        &self,
        device: &ID3D12Device,
        list: &ID3D12GraphicsCommandList,
        frame_index: usize,
    ) -> Result<Option<ID3D12Resource>> {
        self.compute.capture_counts(device, list, frame_index)
    }

    pub unsafe fn read_counts(readback: &ID3D12Resource) -> Result<(u32, u32)> {
        let mut mapped = std::ptr::null_mut();
        readback.Map(
            0,
            Some(&D3D12_RANGE { Begin: 0, End: 8 }),
            Some(&mut mapped),
        )?;
        let [patches, blades] = mapped.cast::<[u32; 2]>().read();
        readback.Unmap(0, Some(&D3D12_RANGE::default()));
        Ok((patches, blades * (INDICES_PER_BLADE / 3)))
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
}
