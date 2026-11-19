use super::{GrassSettings, LODS, LOD_COUNT, MAX_BLADES};
use crate::{
    gpu::{buffer, buffer_with_flags, transition, uav_barrier, upload},
    pipeline::{root_descriptor, serialize_signature, shader},
    terrain::Terrain,
};
use std::mem::ManuallyDrop;
use windows::{core::Result, Win32::Graphics::Direct3D12::*};

const GROUP_WIDTH: u32 = 32;
const PATCH_BYTES: u64 = 32;
const CANDIDATE_BYTES: u64 = PATCH_BYTES + 8;
pub(super) const ARGUMENT_OFFSET: u64 = 8;
// Header, indexed commands (one root constant each), bucket totals and bucket offsets.
const DRAW_DATA_BYTES: u64 = ARGUMENT_OFFSET + MAX_BLADES as u64 * (24 + 8);
const UAV: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
const COMMON: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_COMMON;
const SRV: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE;
const DRAW: D3D12_RESOURCE_STATES =
    D3D12_RESOURCE_STATES(SRV.0 | D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT.0);
const COPY_SOURCE: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_COPY_SOURCE;

/// Matches GrassGenerate in the compute shader. The CPU only sizes the dispatch and uploads settings.
#[repr(C)]
pub(super) struct GrassWork {
    lod_ends: [f32; LOD_COUNT],
    lod_spacings: [f32; LOD_COUNT],
    transition_widths: [f32; LOD_COUNT],
    grid_sides: [u32; LOD_COUNT],
    group_ends: [u32; LOD_COUNT],
    draw_distance: f32,
    density: f32,
    height_scale: f32,
    mesh_path: u32,
    full_density: u32,
    groups: u32,
    patch_capacity: u32,
    _padding: u32,
}

impl GrassWork {
    pub(super) fn new(settings: &GrassSettings, full_density: bool, mesh: bool) -> Self {
        let draw_distance = settings.draw_distance.min(settings.lod_ends[LOD_COUNT - 1]);
        let mut groups = 0;
        let mut patch_capacity = 0;
        let mut grid_sides = [0; LOD_COUNT];
        let mut group_ends = [0; LOD_COUNT];
        // Round up density here to bound floating-point blade counts in the shader.
        let density_ceiling = settings.density_percent.div_ceil(100);
        if density_ceiling != 0 {
            for (level, lod) in LODS.iter().enumerate() {
                // Include both the half-cell camera rounding and the half-cell patch jitter.
                let side = 2
                    * ((draw_distance.min(settings.lod_ends[level]) / lod.spacing).ceil() as u32
                        + 1)
                    + 1;
                let cells = side * side;
                groups += cells.div_ceil(GROUP_WIDTH);
                group_ends[level] = groups;
                grid_sides[level] = side;

                let multiplier = if level == 0 || full_density { 1 } else { 2 };
                let dense = density_ceiling * multiplier;
                if full_density {
                    patch_capacity += cells * dense;
                } else {
                    // Beyond 48 m there are at most two base blades per cell.
                    let near_side = (2 * (48.0 / lod.spacing).ceil() as u32 + 3).min(side);
                    let near_cells = near_side * near_side;
                    let far = (2 * dense).div_ceil(MAX_BLADES);
                    patch_capacity += near_cells * dense + (cells - near_cells) * far;
                }
            }
        }
        Self {
            lod_ends: settings.lod_ends,
            lod_spacings: LODS.map(|lod| lod.spacing),
            transition_widths: LODS.map(|lod| lod.transition_width),
            grid_sides,
            group_ends,
            draw_distance,
            density: settings.density_percent as f32 / 100.0,
            height_scale: settings.height_percent as f32 / 100.0,
            mesh_path: u32::from(mesh),
            full_density: u32::from(full_density),
            groups,
            patch_capacity,
            _padding: 0,
        }
    }

    fn channels(&self) -> u32 {
        if self.mesh_path != 0 {
            2
        } else {
            MAX_BLADES
        }
    }
}

pub(super) struct GrassOutput {
    pub patches: ID3D12Resource,
    pub draw_data: ID3D12Resource,
    candidates: ID3D12Resource,
    offsets: ID3D12Resource,
}

impl GrassOutput {
    unsafe fn new(device: &ID3D12Device, work: &GrassWork) -> Result<Self> {
        let storage = |bytes: u64| {
            buffer_with_flags(
                device,
                bytes.next_multiple_of(65_536),
                D3D12_HEAP_TYPE_DEFAULT,
                COMMON,
                D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            )
        };
        Ok(Self {
            patches: storage(u64::from(work.patch_capacity) * PATCH_BYTES)?,
            draw_data: storage(DRAW_DATA_BYTES)?,
            candidates: storage(u64::from(work.groups * GROUP_WIDTH) * CANDIDATE_BYTES)?,
            offsets: storage(u64::from(work.groups * work.channels()) * 4)?,
        })
    }

    unsafe fn fits(&self, work: &GrassWork) -> bool {
        self.patches.GetDesc().Width >= u64::from(work.patch_capacity) * PATCH_BYTES
            && self.candidates.GetDesc().Width
                >= u64::from(work.groups * GROUP_WIDTH) * CANDIDATE_BYTES
            && self.offsets.GetDesc().Width >= u64::from(work.groups * work.channels()) * 4
    }
}

struct GrassSlot {
    constants: ID3D12Resource,
    buffers: Option<GrassOutput>,
    active: bool,
}

pub(super) struct GrassCompute {
    root: ID3D12RootSignature,
    classify: ID3D12PipelineState,
    scan: ID3D12PipelineState,
    commands: ID3D12PipelineState,
    scatter: ID3D12PipelineState,
    slots: Vec<GrassSlot>,
}

impl GrassCompute {
    pub(super) unsafe fn new(device: &ID3D12Device, frame_count: usize) -> Result<Self> {
        let mut params = vec![
            root_descriptor(D3D12_ROOT_PARAMETER_TYPE_CBV, 0),
            root_descriptor(D3D12_ROOT_PARAMETER_TYPE_CBV, 1),
            root_descriptor(D3D12_ROOT_PARAMETER_TYPE_SRV, 4),
        ];
        params.extend(
            (0..4).map(|register| root_descriptor(D3D12_ROOT_PARAMETER_TYPE_UAV, register)),
        );
        let root = serialize_signature(
            device,
            &D3D12_ROOT_SIGNATURE_DESC {
                NumParameters: params.len() as u32,
                pParameters: params.as_ptr(),
                ..Default::default()
            },
        )?;
        let pipeline = |shader| {
            let mut desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
                pRootSignature: ManuallyDrop::new(Some(root.clone())),
                CS: shader,
                ..Default::default()
            };
            let result = device.CreateComputePipelineState(&desc);
            ManuallyDrop::drop(&mut desc.pRootSignature);
            result
        };
        let slots = (0..frame_count)
            .map(|_| {
                Ok(GrassSlot {
                    constants: buffer(
                        device,
                        u64::from(D3D12_CONSTANT_BUFFER_DATA_PLACEMENT_ALIGNMENT),
                        D3D12_HEAP_TYPE_UPLOAD,
                        D3D12_RESOURCE_STATE_GENERIC_READ,
                    )?,
                    buffers: None,
                    active: false,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            classify: pipeline(shader!("grass_classify"))?,
            scan: pipeline(shader!("grass_scan"))?,
            commands: pipeline(shader!("grass_commands"))?,
            scatter: pipeline(shader!("grass_scatter"))?,
            root,
            slots,
        })
    }

    pub(super) unsafe fn record(
        &mut self,
        device: &ID3D12Device,
        list: &ID3D12GraphicsCommandList,
        frame_index: usize,
        frame_constants: u64,
        terrain: &Terrain,
        work: &GrassWork,
    ) -> Result<()> {
        let slot = &mut self.slots[frame_index];
        slot.active = work.groups != 0;
        if !slot.active {
            return Ok(());
        }
        // This frame slot's fence has completed before allocating or writing its buffers.
        let allocate = slot
            .buffers
            .as_ref()
            .is_none_or(|buffers| !buffers.fits(work));
        if allocate {
            slot.buffers = Some(GrassOutput::new(device, work)?);
        }
        let buffers = slot.buffers.as_ref().unwrap();
        upload(&slot.constants, std::slice::from_ref(work))?;
        transition(
            list,
            &buffers.patches,
            if allocate { COMMON } else { SRV },
            UAV,
        );
        transition(
            list,
            &buffers.draw_data,
            if allocate { COMMON } else { DRAW },
            UAV,
        );
        list.SetComputeRootSignature(&self.root);
        list.SetComputeRootConstantBufferView(0, frame_constants);
        list.SetComputeRootConstantBufferView(1, slot.constants.GetGPUVirtualAddress());
        terrain.bind_base_samples(list, 2);
        for (index, buffer) in [
            &buffers.candidates,
            &buffers.offsets,
            &buffers.patches,
            &buffers.draw_data,
        ]
        .into_iter()
        .enumerate()
        {
            list.SetComputeRootUnorderedAccessView(index as u32 + 3, buffer.GetGPUVirtualAddress());
        }
        list.SetPipelineState(&self.classify);
        list.Dispatch(work.groups, 1, 1);
        uav_barrier(list);
        list.SetPipelineState(&self.scan);
        list.Dispatch(work.channels(), 1, 1);
        uav_barrier(list);
        list.SetPipelineState(&self.commands);
        list.Dispatch(1, 1, 1);
        uav_barrier(list);
        list.SetPipelineState(&self.scatter);
        list.Dispatch(work.groups, 1, 1);
        transition(list, &buffers.patches, UAV, SRV);
        transition(list, &buffers.draw_data, UAV, DRAW);
        Ok(())
    }

    pub(super) fn output(&self, frame_index: usize) -> Option<&GrassOutput> {
        let slot = &self.slots[frame_index];
        slot.buffers.as_ref().filter(|_| slot.active)
    }

    pub(super) unsafe fn capture_counts(
        &self,
        device: &ID3D12Device,
        list: &ID3D12GraphicsCommandList,
        frame_index: usize,
    ) -> Result<Option<ID3D12Resource>> {
        let Some(buffers) = self.output(frame_index) else {
            return Ok(None);
        };
        let readback = buffer(
            device,
            ARGUMENT_OFFSET,
            D3D12_HEAP_TYPE_READBACK,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        transition(list, &buffers.draw_data, DRAW, COPY_SOURCE);
        list.CopyBufferRegion(&readback, 0, &buffers.draw_data, 0, ARGUMENT_OFFSET);
        transition(list, &buffers.draw_data, COPY_SOURCE, DRAW);
        Ok(Some(readback))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximum_grass_grid_and_indirect_dispatch_fit_d3d12_limits() {
        let mut settings = GrassSettings {
            density_percent: super::super::MAX_DENSITY_PERCENT,
            ..GrassSettings::default()
        };
        for level in (0..LOD_COUNT).rev() {
            settings.set_lod_end(level, super::super::MAX_DRAW_DISTANCE);
        }
        let work = GrassWork::new(&settings, true, true);
        assert!(work.groups <= 65_535);
        assert!(work.patch_capacity.div_ceil(1024) * 1024 <= (1 << 22));
        assert!(GrassWork::new(&settings, false, true).patch_capacity < work.patch_capacity);
    }
}
