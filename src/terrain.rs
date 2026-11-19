mod heightfield;

use crate::{
    gpu::{buffer, buffer_with_flags, readback_u32, transition, uav_barrier, upload_to_gpu},
    pipeline::{root_descriptor, serialize_signature, shader},
};
use heightfield::{Heightfield, GRID_CELLS, NODES_PER_ROOT, ROOT_COUNT, ROOT_GRID};
use std::mem::ManuallyDrop;
use windows::{
    core::Result,
    Win32::Graphics::{
        Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Direct3D12::*,
        Dxgi::Common::DXGI_FORMAT_R32_UINT,
    },
};

pub use heightfield::{origin_height, GRID_STEP, MAX_DEPTH, SIDE_METERS};

const UPDATE_THREADS: u32 = 128;
const REDUCE_THREADS: u32 = 64;
const BITMAP_WORDS: u32 = REDUCE_THREADS * REDUCE_THREADS;
const POOL_CAPACITY: u32 = 32 * BITMAP_WORDS;
const NODE_COUNT: u32 = ROOT_COUNT * NODES_PER_ROOT;
const VERTEX_COUNT: u32 = (GRID_CELLS + 1) * (GRID_CELLS + 1);
const SEAM_STRIDE_PARAM: u32 = 13;
const INDEX_BYTES: u32 = POOL_CAPACITY * 3 * size_of::<u32>() as u32;
const INDEX: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_INDEX_BUFFER;
const DRAW_OFFSET: u64 = size_of::<D3D12_DISPATCH_ARGUMENTS>() as u64;
const STABLE_OFFSET: u64 = DRAW_OFFSET + size_of::<D3D12_DRAW_INDEXED_ARGUMENTS>() as u64;
const UAV: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
const SRV: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE;
const INDIRECT: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT;
const COPY_SOURCE: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_COPY_SOURCE;

pub const GRID: [u32; 4] = [GRID_CELLS, ROOT_COUNT, NODES_PER_ROOT, POOL_CAPACITY];

// One per frame slot. The renderer waits for the slot's existing fence before reuse.
pub struct LodFeedback {
    readback: ID3D12Resource,
    generation: Option<u64>,
}

impl LodFeedback {
    pub unsafe fn new(device: &ID3D12Device) -> Result<Self> {
        Ok(Self {
            readback: buffer(
                device,
                4,
                D3D12_HEAP_TYPE_READBACK,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )?,
            generation: None,
        })
    }
}

struct LodPipeline {
    root: ID3D12RootSignature,
    init: ID3D12PipelineState,
    evaluate: ID3D12PipelineState,
    split_offsets: ID3D12PipelineState,
    apply: ID3D12PipelineState,
    reduce_blocks: ID3D12PipelineState,
    reduce_root: ID3D12PipelineState,
    clear_seams: ID3D12PipelineState,
    mark_seams: ID3D12PipelineState,
    resolve_vertices: ID3D12PipelineState,
    dispatch: ID3D12CommandSignature,
    draw: ID3D12CommandSignature,
}

impl LodPipeline {
    unsafe fn new(device: &ID3D12Device) -> Result<Self> {
        let mut params = vec![root_descriptor(D3D12_ROOT_PARAMETER_TYPE_CBV, 0)];
        params.extend(
            [3, 4, 6].map(|register| root_descriptor(D3D12_ROOT_PARAMETER_TYPE_SRV, register)),
        );
        params.extend(
            (0..9).map(|register| root_descriptor(D3D12_ROOT_PARAMETER_TYPE_UAV, register)),
        );
        debug_assert_eq!(params.len(), SEAM_STRIDE_PARAM as usize);
        params.push(D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 1,
                    RegisterSpace: 0,
                    Num32BitValues: 1,
                },
            },
        });
        let root = serialize_signature(
            device,
            &D3D12_ROOT_SIGNATURE_DESC {
                NumParameters: params.len() as u32,
                pParameters: params.as_ptr(),
                ..Default::default()
            },
        )?;
        let compute = |cs| {
            let mut desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
                pRootSignature: ManuallyDrop::new(Some(root.clone())),
                CS: cs,
                ..Default::default()
            };
            let result = device.CreateComputePipelineState(&desc);
            ManuallyDrop::drop(&mut desc.pRootSignature);
            result
        };
        let signature = |kind, stride| -> Result<ID3D12CommandSignature> {
            let argument = D3D12_INDIRECT_ARGUMENT_DESC {
                Type: kind,
                ..Default::default()
            };
            let mut signature = None;
            device.CreateCommandSignature(
                &D3D12_COMMAND_SIGNATURE_DESC {
                    ByteStride: stride,
                    NumArgumentDescs: 1,
                    pArgumentDescs: &argument,
                    NodeMask: 0,
                },
                None,
                &mut signature,
            )?;
            Ok(signature.unwrap())
        };
        let init = compute(shader!("terrain_init"))?;
        let evaluate = compute(shader!("terrain_evaluate"))?;
        let split_offsets = compute(shader!("terrain_split_offsets"))?;
        let apply = compute(shader!("terrain_apply"))?;
        let reduce_blocks = compute(shader!("terrain_reduce_blocks"))?;
        let reduce_root = compute(shader!("terrain_reduce_root"))?;
        let clear_seams = compute(shader!("terrain_seam_clear"))?;
        let mark_seams = compute(shader!("terrain_seam_mark"))?;
        let resolve_vertices = compute(shader!("terrain_seam_resolve"))?;
        Ok(Self {
            root,
            init,
            evaluate,
            split_offsets,
            apply,
            reduce_blocks,
            reduce_root,
            clear_seams,
            mark_seams,
            resolve_vertices,
            dispatch: signature(
                D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH,
                size_of::<D3D12_DISPATCH_ARGUMENTS>() as u32,
            )?,
            draw: signature(
                D3D12_INDIRECT_ARGUMENT_TYPE_DRAW_INDEXED,
                size_of::<D3D12_DRAW_INDEXED_ARGUMENTS>() as u32,
            )?,
        })
    }
}

pub struct Terrain {
    pipeline: LodPipeline,
    nodes: ID3D12Resource,
    base_vertices: ID3D12Resource,
    edge_constraints: ID3D12Resource,
    resolved_vertices: ID3D12Resource,
    indices: ID3D12Resource,
    pool: ID3D12Resource,
    leaf_slots: ID3D12Resource,
    commands: ID3D12Resource,
    occupancy: ID3D12Resource,
    tree: ID3D12Resource,
    arguments: ID3D12Resource,
    samples: Vec<f32>,
    initialized: bool,
    lod_view: Option<[f32; 4]>,
    generation: u64,
    settled: bool,
}

impl Terrain {
    pub unsafe fn new(
        device: &ID3D12Device,
        list: &ID3D12GraphicsCommandList,
        staging: &mut Vec<ID3D12Resource>,
    ) -> Result<Self> {
        let field = Heightfield::generate();
        let nodes = upload_to_gpu(device, list, &field.nodes, SRV, staging)?;
        let base_vertices = upload_to_gpu(device, list, &field.vertices(), SRV, staging)?;
        let storage = |bytes| {
            buffer_with_flags(
                device,
                bytes,
                D3D12_HEAP_TYPE_DEFAULT,
                D3D12_RESOURCE_STATE_COMMON,
                D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            )
        };
        Ok(Self {
            pipeline: LodPipeline::new(device)?,
            nodes,
            base_vertices,
            edge_constraints: storage(u64::from(VERTEX_COUNT) * 4)?,
            resolved_vertices: storage(u64::from(VERTEX_COUNT) * 16)?,
            indices: storage(u64::from(INDEX_BYTES))?,
            // Node IDs by slot, followed by the compact list of occupied slots.
            pool: storage(u64::from(POOL_CAPACITY) * 2 * 4)?,
            leaf_slots: storage(u64::from(NODE_COUNT) * 4)?,
            commands: storage(u64::from(POOL_CAPACITY) * 16)?,
            occupancy: storage(
                u64::from(BITMAP_WORDS + POOL_CAPACITY.div_ceil(UPDATE_THREADS) + 1) * 4,
            )?,
            tree: storage(u64::from(3 * BITMAP_WORDS) * 4)?,
            arguments: storage(STABLE_OFFSET + 4)?,
            samples: field.samples,
            initialized: false,
            lod_view: None,
            generation: 0,
            settled: false,
        })
    }

    // Used for conservative grass culling. The shader places roots on the active LOD.
    pub fn approximate_height(&self, xz: glam::Vec2) -> f32 {
        let p = (xz / GRID_STEP + glam::Vec2::splat(GRID_CELLS as f32 * 0.5))
            .clamp(glam::Vec2::ZERO, glam::Vec2::splat(GRID_CELLS as f32));
        let col = (p.x as u32).min(GRID_CELLS - 1);
        let row = (p.y as u32).min(GRID_CELLS - 1);
        let i = (row * (GRID_CELLS + 1) + col) as usize;
        let stride = (GRID_CELLS + 1) as usize;
        let u = p.x - col as f32;
        let v = p.y - row as f32;
        let h = &self.samples;
        let a = h[i] + u * (h[i + 1] - h[i]);
        let b = h[i + stride] + u * (h[i + stride + 1] - h[i + stride]);
        a + v * (b - a)
    }

    // A single ordered direct queue owns the persistent pool and its reduction tree.
    pub unsafe fn prepare(
        &mut self,
        list: &ID3D12GraphicsCommandList,
        frame: u64,
        view: [f32; 4],
        feedback: &mut LodFeedback,
    ) -> Result<()> {
        let same_view = self.lod_view == Some(view);
        if !same_view {
            self.lod_view = Some(view);
            self.generation += 1;
            self.settled = false;
        }
        if feedback.generation.take() == Some(self.generation) && !self.settled {
            self.settled = readback_u32(&feedback.readback)? != 0;
        }
        if !self.settled {
            self.update_mesh(list, frame);
            if same_view {
                transition(list, &self.arguments, INDIRECT, COPY_SOURCE);
                list.CopyBufferRegion(&feedback.readback, 0, &self.arguments, STABLE_OFFSET, 4);
                transition(list, &self.arguments, COPY_SOURCE, INDIRECT);
                feedback.generation = Some(self.generation);
            }
        }
        Ok(())
    }

    unsafe fn update_mesh(&mut self, list: &ID3D12GraphicsCommandList, frame: u64) {
        if self.initialized {
            transition(list, &self.indices, INDEX, UAV);
            transition(list, &self.leaf_slots, SRV, UAV);
            transition(list, &self.resolved_vertices, SRV, UAV);
        }
        self.update_lod(list, frame);
        self.resolve_vertices(list);
        transition(list, &self.indices, UAV, INDEX);
        transition(list, &self.leaf_slots, UAV, SRV);
        transition(list, &self.resolved_vertices, UAV, SRV);
    }

    unsafe fn update_lod(&mut self, list: &ID3D12GraphicsCommandList, frame: u64) {
        list.SetComputeRootSignature(&self.pipeline.root);
        list.SetComputeRootConstantBufferView(0, frame);
        for (index, resource) in [&self.nodes, &self.base_vertices, &self.tree]
            .into_iter()
            .enumerate()
        {
            list.SetComputeRootShaderResourceView(
                index as u32 + 1,
                resource.GetGPUVirtualAddress(),
            );
        }
        for (index, resource) in [
            &self.pool,
            &self.leaf_slots,
            &self.commands,
            &self.occupancy,
            &self.tree,
            &self.arguments,
            &self.edge_constraints,
            &self.resolved_vertices,
            &self.indices,
        ]
        .into_iter()
        .enumerate()
        {
            list.SetComputeRootUnorderedAccessView(
                index as u32 + 4,
                resource.GetGPUVirtualAddress(),
            );
        }
        if !self.initialized {
            list.SetPipelineState(&self.pipeline.init);
            list.Dispatch(NODE_COUNT.div_ceil(UPDATE_THREADS), 1, 1);
            uav_barrier(list);
            self.reduce(list);
            self.initialized = true;
        }
        list.SetPipelineState(&self.pipeline.evaluate);
        list.ExecuteIndirect(&self.pipeline.dispatch, 1, &self.arguments, 0, None, 0);
        uav_barrier(list);
        list.SetPipelineState(&self.pipeline.split_offsets);
        list.Dispatch(1, 1, 1);
        uav_barrier(list);
        list.SetPipelineState(&self.pipeline.apply);
        list.ExecuteIndirect(&self.pipeline.dispatch, 1, &self.arguments, 0, None, 0);
        uav_barrier(list);
        transition(list, &self.tree, SRV, UAV);
        transition(list, &self.arguments, INDIRECT, UAV);
        self.reduce(list);
    }

    unsafe fn resolve_vertices(&self, list: &ID3D12GraphicsCommandList) {
        list.SetPipelineState(&self.pipeline.clear_seams);
        list.Dispatch(VERTEX_COUNT.div_ceil(UPDATE_THREADS), 1, 1);
        uav_barrier(list);

        list.SetPipelineState(&self.pipeline.mark_seams);
        list.ExecuteIndirect(&self.pipeline.dispatch, 1, &self.arguments, 0, None, 0);
        uav_barrier(list);

        list.SetPipelineState(&self.pipeline.resolve_vertices);
        let mut stride = GRID_CELLS / ROOT_GRID;
        while stride != 0 {
            let groups = (GRID_CELLS / stride + 1).div_ceil(8);
            list.SetComputeRoot32BitConstant(SEAM_STRIDE_PARAM, stride, 0);
            list.Dispatch(groups, groups, 1);
            stride /= 2;
            if stride != 0 {
                uav_barrier(list);
            }
        }
    }

    unsafe fn reduce(&self, list: &ID3D12GraphicsCommandList) {
        list.SetPipelineState(&self.pipeline.reduce_blocks);
        list.Dispatch(BITMAP_WORDS / REDUCE_THREADS, 1, 1);
        uav_barrier(list);
        list.SetPipelineState(&self.pipeline.reduce_root);
        list.Dispatch(1, 1, 1);
        uav_barrier(list);
        transition(list, &self.tree, UAV, SRV);
        transition(list, &self.arguments, UAV, INDIRECT);
    }

    pub unsafe fn draw(
        &self,
        list: &ID3D12GraphicsCommandList,
        pipeline: &ID3D12PipelineState,
        shadow: bool,
    ) {
        list.SetPipelineState(pipeline);
        list.SetGraphicsRoot32BitConstant(1, u32::from(shadow), 0);
        for (index, resource) in [&self.leaf_slots, &self.resolved_vertices]
            .into_iter()
            .enumerate()
        {
            list.SetGraphicsRootShaderResourceView(
                index as u32 + 5,
                resource.GetGPUVirtualAddress(),
            );
        }
        list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        list.IASetIndexBuffer(Some(&D3D12_INDEX_BUFFER_VIEW {
            BufferLocation: self.indices.GetGPUVirtualAddress(),
            SizeInBytes: INDEX_BYTES,
            Format: DXGI_FORMAT_R32_UINT,
        }));
        list.ExecuteIndirect(
            &self.pipeline.draw,
            1,
            &self.arguments,
            DRAW_OFFSET,
            None,
            0,
        );
    }

    pub unsafe fn capture_count(
        &self,
        device: &ID3D12Device,
        list: &ID3D12GraphicsCommandList,
    ) -> Result<ID3D12Resource> {
        let readback = buffer(
            device,
            4,
            D3D12_HEAP_TYPE_READBACK,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        transition(list, &self.tree, SRV, COPY_SOURCE);
        list.CopyBufferRegion(&readback, 0, &self.tree, 4, 4);
        transition(list, &self.tree, COPY_SOURCE, SRV);
        Ok(readback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        camera::Camera,
        display::choose_device,
        gpu::{upload, Timeline},
        scene::{Animation, Frame},
    };
    use std::{ptr, slice};
    use windows::{
        core::Interface,
        Win32::Graphics::Dxgi::{CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS},
    };

    unsafe fn capture_frontier(
        device: &ID3D12Device,
        list: &ID3D12GraphicsCommandList,
        terrain: &Terrain,
    ) -> Result<ID3D12Resource> {
        let pool_bytes = u64::from(POOL_CAPACITY) * 4;
        let bitmap_bytes = u64::from(BITMAP_WORDS) * 4;
        let vertex_bytes = u64::from(VERTEX_COUNT) * 16;
        let readback = buffer(
            device,
            pool_bytes + bitmap_bytes + vertex_bytes + u64::from(INDEX_BYTES),
            D3D12_HEAP_TYPE_READBACK,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        transition(list, &terrain.pool, UAV, COPY_SOURCE);
        transition(list, &terrain.indices, INDEX, COPY_SOURCE);
        for resource in [&terrain.tree, &terrain.resolved_vertices] {
            transition(list, resource, SRV, COPY_SOURCE);
        }
        list.CopyBufferRegion(&readback, 0, &terrain.pool, 0, pool_bytes);
        list.CopyBufferRegion(
            &readback,
            pool_bytes,
            &terrain.tree,
            2 * bitmap_bytes,
            bitmap_bytes,
        );
        list.CopyBufferRegion(
            &readback,
            pool_bytes + bitmap_bytes,
            &terrain.resolved_vertices,
            0,
            vertex_bytes,
        );
        transition(list, &terrain.pool, COPY_SOURCE, UAV);
        list.CopyBufferRegion(
            &readback,
            pool_bytes + bitmap_bytes + vertex_bytes,
            &terrain.indices,
            0,
            u64::from(INDEX_BYTES),
        );
        transition(list, &terrain.indices, COPY_SOURCE, INDEX);
        for resource in [&terrain.tree, &terrain.resolved_vertices] {
            transition(list, resource, COPY_SOURCE, SRV);
        }
        Ok(readback)
    }

    unsafe fn verify_frontier(resource: &ID3D12Resource, field: &Heightfield) -> Result<usize> {
        let bitmap_end = (POOL_CAPACITY + BITMAP_WORDS) as usize;
        let mut mapped = ptr::null_mut();
        resource.Map(
            0,
            Some(&D3D12_RANGE {
                Begin: 0,
                End: bitmap_end * 4 + VERTEX_COUNT as usize * 16 + INDEX_BYTES as usize,
            }),
            Some(&mut mapped),
        )?;
        let (pool, bitmap) = slice::from_raw_parts(mapped.cast::<u32>(), bitmap_end)
            .split_at(POOL_CAPACITY as usize);
        let vertices = slice::from_raw_parts(
            mapped.cast::<u32>().add(bitmap_end).cast::<[f32; 4]>(),
            VERTEX_COUNT as usize,
        );
        let mut intervals = Vec::new();
        let mut triangles = Vec::new();
        let indices = slice::from_raw_parts(
            mapped
                .cast::<u8>()
                .add(bitmap_end * 4 + VERTEX_COUNT as usize * 16)
                .cast::<[u32; 3]>(),
            POOL_CAPACITY as usize,
        );
        for (slot, &node) in pool.iter().enumerate() {
            if bitmap[slot / 32] & (1 << (slot % 32)) == 0 {
                continue;
            }
            let root = node / NODES_PER_ROOT;
            let local = node % NODES_PER_ROOT;
            assert!(root < ROOT_COUNT && local > 0);
            assert_eq!(indices[triangles.len()], field.nodes[node as usize].corners);
            let depth = local.ilog2();
            let len = 1 << (MAX_DEPTH - depth);
            let start = (root << MAX_DEPTH) + (local - (1 << depth)) * len;
            intervals.push((start, len));
            triangles.push(
                field.nodes[node as usize]
                    .corners
                    .map(|p| glam::IVec2::new((p & 65535) as i32, (p >> 16) as i32)),
            );
        }
        let grid_index = |p: glam::IVec2| p.y as usize * (GRID_CELLS + 1) as usize + p.x as usize;
        let mut used = vec![false; VERTEX_COUNT as usize];
        for &p in triangles.iter().flatten() {
            used[grid_index(p)] = true;
        }
        let mut hanging = 0;
        let mut moved = 0;
        for [a, b, c] in triangles {
            for (from, to) in [(a, b), (b, c), (c, a)] {
                let span = (to - from).abs().max_element();
                let step = (to - from) / span;
                let start = vertices[grid_index(from)];
                let end = vertices[grid_index(to)];
                for offset in 1..span {
                    let index = grid_index(from + offset * step);
                    if !used[index] {
                        continue;
                    }
                    hanging += 1;
                    let t = offset as f32 / span as f32;
                    let actual = vertices[index];
                    moved += usize::from((actual[0] - field.samples[index]).abs() > 0.01);
                    for channel in 0..4 {
                        let expected = start[channel] + (end[channel] - start[channel]) * t;
                        assert!(
                            (actual[channel] - expected).abs() < 0.0005,
                            "T-junction at grid vertex {index} disagrees with edge {from:?} to {to:?} on channel {channel}"
                        );
                    }
                }
            }
        }
        resource.Unmap(0, Some(&D3D12_RANGE::default()));
        if intervals.len() == ROOT_COUNT as usize {
            assert_eq!(hanging, 0);
        } else {
            assert!(hanging > 0 && moved > 0, "expected different LODs to meet");
        }
        intervals.sort_unstable();
        let mut next = 0;
        for &(start, len) in &intervals {
            assert_eq!(start, next, "the active triangles overlap or leave a hole");
            next += len;
        }
        assert_eq!(next, ROOT_COUNT << MAX_DEPTH);
        Ok(intervals.len())
    }

    #[test]
    #[ignore = "requires D3D12 WARP"]
    fn gpu_pool_preserves_coverage_and_continuous_seams_through_lod_changes() -> Result<()> {
        unsafe {
            let factory = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
            let device = choose_device(&factory, true)?;
            let queue: ID3D12CommandQueue = device.CreateCommandQueue(&Default::default())?;
            let allocator: ID3D12CommandAllocator =
                device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
            let list: ID3D12GraphicsCommandList =
                device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)?;
            let mut staging = Vec::new();
            let mut terrain = Terrain::new(&device, &list, &mut staging)?;
            let mut near = Camera::default();
            near.eye = glam::vec3(-250.0, 525.0, 2030.0);
            let mut far = Camera::default();
            far.eye.y = 2_000_000.0;
            let field = Heightfield::generate();
            let mut timeline = Timeline::new(&device)?;
            let mut feedback = [LodFeedback::new(&device)?, LodFeedback::new(&device)?];
            let mut slot = 0;
            let mut counts = Vec::new();
            for (camera, viewport) in [
                (&near, [1280, 720]),
                // Use a synthetic projection scale to drive even gentle hills into pool saturation.
                (&near, [7680, 1_000_000]),
                (&far, [1280, 720]),
                (&near, [1280, 720]),
            ] {
                let constants = buffer(
                    &device,
                    512,
                    D3D12_HEAP_TYPE_UPLOAD,
                    D3D12_RESOURCE_STATE_GENERIC_READ,
                )?;
                let frame = Frame::new(
                    viewport,
                    camera,
                    Animation::new(0.0),
                    0.0,
                    false,
                    80.0,
                    origin_height(),
                );
                let view = frame.terrain_lod_view();
                upload(&constants, &[frame])?;
                for _ in 0..2 * MAX_DEPTH {
                    terrain.prepare(
                        &list,
                        constants.GetGPUVirtualAddress(),
                        view,
                        &mut feedback[slot],
                    )?;
                    slot ^= 1;
                    let capture = terrain
                        .settled
                        .then(|| capture_frontier(&device, &list, &terrain))
                        .transpose()?;
                    list.Close()?;
                    queue.ExecuteCommandLists(&[Some(list.cast()?)]);
                    let done = timeline.signal(&queue)?;
                    timeline.wait(done)?;
                    allocator.Reset()?;
                    list.Reset(&allocator, None)?;
                    if let Some(capture) = capture {
                        counts.push(verify_frontier(&capture, &field)?);
                        break;
                    }
                }
                assert!(terrain.settled, "unchanged LOD inputs should converge");
            }
            list.Close()?;
            assert!(
                (ROOT_COUNT as usize + 1..POOL_CAPACITY as usize).contains(&counts[0]),
                "{counts:?}"
            );
            assert_eq!(
                counts,
                [
                    counts[0],
                    POOL_CAPACITY as usize,
                    ROOT_COUNT as usize,
                    counts[0]
                ]
            );
            Ok(())
        }
    }
}
