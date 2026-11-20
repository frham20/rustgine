use crate::{
    camera::frustum_planes,
    gpu::{transition, upload_to_gpu, upload_to_gpu_with_flags},
    pipeline::{AlpinePipelines, SurfacePass},
    scene::Vertex,
    terrain::{alpine, Projection, Terrain, SIDE_METERS},
};
use glam::{Mat4, Vec2, Vec3, Vec4};
use std::{f32::consts::TAU, ops::Range};
use windows::{
    core::Result,
    Win32::Graphics::{Direct3D12::*, Dxgi::Common::DXGI_FORMAT_R32_UINT},
};

#[derive(Default)]
struct Mesh {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
}

impl Mesh {
    fn vertex(&mut self, p: Vec3, n: Vec3) -> u32 {
        let id = self.vertices.len() as u32;
        self.vertices.push(Vertex {
            position: p.to_array(),
            normal: n.to_array(),
        });
        id
    }

    fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.indices.extend([a, b, c, a, c, d]);
    }
}

struct GpuMesh {
    vertices: ID3D12Resource,
    indices: ID3D12Resource,
    vertex_bytes: u32,
    count: u32,
}

impl GpuMesh {
    unsafe fn upload(
        mesh: Mesh,
        device: &ID3D12Device,
        list: &ID3D12GraphicsCommandList,
        staging: &mut Vec<ID3D12Resource>,
    ) -> Result<Self> {
        Ok(Self {
            vertices: upload_to_gpu(
                device,
                list,
                &mesh.vertices,
                D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
                staging,
            )?,
            indices: upload_to_gpu(
                device,
                list,
                &mesh.indices,
                D3D12_RESOURCE_STATE_INDEX_BUFFER,
                staging,
            )?,
            vertex_bytes: size_of_val(mesh.vertices.as_slice()) as u32,
            count: mesh.indices.len() as u32,
        })
    }

    unsafe fn bind(&self, list: &ID3D12GraphicsCommandList) {
        list.IASetVertexBuffers(
            0,
            Some(&[D3D12_VERTEX_BUFFER_VIEW {
                BufferLocation: self.vertices.GetGPUVirtualAddress(),
                SizeInBytes: self.vertex_bytes,
                StrideInBytes: size_of::<Vertex>() as u32,
            }]),
        );
        list.IASetIndexBuffer(Some(&D3D12_INDEX_BUFFER_VIEW {
            BufferLocation: self.indices.GetGPUVirtualAddress(),
            SizeInBytes: self.count * 4,
            Format: DXGI_FORMAT_R32_UINT,
        }));
    }

    unsafe fn draw(&self, list: &ID3D12GraphicsCommandList) {
        self.bind(list);
        list.DrawIndexedInstanced(self.count, 1, 0, 0, 0);
    }
}

fn ground_vertex(mesh: &mut Mesh, p: Vec2, spacing: f32, height: impl Fn(Vec2) -> f32) -> u32 {
    let dx = Vec2::X * spacing;
    let dz = Vec2::Y * spacing;
    let n = Vec3::new(
        height(p - dx) - height(p + dx),
        2.0 * spacing,
        height(p - dz) - height(p + dz),
    );
    mesh.vertex(Vec3::new(p.x, height(p), p.y), n.normalize())
}

// The water mesh stores signed bed depth in the first auxiliary vertex channel.
fn lake_mesh(terrain: &Terrain) -> Mesh {
    const CELLS: u32 = 192;
    let step = SIDE_METERS / CELLS as f32;
    let mut lake = Mesh::default();
    for z in 0..=CELLS {
        for x in 0..=CELLS {
            let p = Vec2::new(x as f32, z as f32) * step - SIDE_METERS * 0.5;
            let depth = alpine::LAKE_LEVEL - terrain.approximate_height(p);
            let i = lake.vertex(Vec3::new(p.x, alpine::LAKE_LEVEL, p.y), Vec3::X * depth);
            if x > 0 && z > 0 {
                let a = i - CELLS - 2;
                let b = i - CELLS - 1;
                // Allow dry margin for the active terrain LOD to define the shoreline.
                if [a, b, i - 1, i]
                    .iter()
                    .any(|&j| lake.vertices[j as usize].normal[0] > -25.0)
                {
                    lake.quad(a, i - 1, i, b);
                }
            }
        }
    }
    lake
}

fn outer_ranges() -> Mesh {
    const SIDE: u32 = 128;
    let mut mesh = Mesh::default();
    let mut radius = SIDE_METERS * 0.5 - 30.0;
    for ring in 0..74 {
        for side in 0..4 {
            for i in 0..SIDE {
                let t = 2.0 * i as f32 / SIDE as f32 - 1.0;
                let edge = match side {
                    0 => Vec2::new(t, -1.0),
                    1 => Vec2::new(1.0, t),
                    2 => Vec2::new(-t, 1.0),
                    _ => Vec2::new(-1.0, -t),
                };
                let id = ground_vertex(&mut mesh, edge * radius, 18.0, alpine::outer_height);
                if ring == 0 {
                    mesh.vertices[id as usize].position[1] -= 55.0;
                }
                if ring > 0 {
                    let next = ring * 4 * SIDE + (side * SIDE + i + 1) % (4 * SIDE);
                    mesh.quad(id - 4 * SIDE, next - 4 * SIDE, next, id);
                }
            }
        }
        radius += 30.0 + ring as f32 * 4.1;
    }
    mesh
}

// A shared mesh with overlapping, ragged whorls. The normal length tags bark.
fn pine_mesh() -> Mesh {
    let mut mesh = Mesh::default();
    for i in 0..7 {
        let angle = TAU * i as f32 / 7.0;
        let (s, c) = angle.sin_cos();
        let n = Vec3::new(c, 0.0, s);
        mesh.vertex(n * 0.021, n * 2.0);
        mesh.vertex(n * 0.009 + Vec3::Y * 0.73, n * 2.0);
        let next = (i + 1) % 7 * 2;
        mesh.quad(i * 2, i * 2 + 1, next + 1, next);
    }
    for tier in 0..8 {
        let t = tier as f32 / 8.0;
        let y = 0.12 + 0.78 * t;
        let radius = 0.19 * (1.0 - 0.86 * t);
        let crown = mesh.vertex(Vec3::Y * (y + (0.33 - 0.16 * t)).min(1.0), Vec3::Y);
        let start = crown + 1;
        for i in 0..26 {
            let a = TAU * i as f32 / 26.0 + tier as f32 * 1.71;
            let (s, c) = a.sin_cos();
            let tip = i % 2 == 0;
            let uneven = (a * 7.0 + t * 13.0).sin();
            let r = radius * if tip { 0.88 + 0.14 * uneven } else { 0.55 };
            let h = y + if tip {
                0.018 * uneven
            } else {
                0.065 * (1.0 - t)
            };
            mesh.vertex(
                Vec3::new(c * r, h, s * r),
                Vec3::new(c, if tip { 0.38 } else { 0.72 }, s).normalize(),
            );
            mesh.indices
                .extend([crown, start + i, start + (i + 1) % 26]);
        }
        // Distinct sprays break up the cone silhouette on nearby trees. Each
        // bough has a raised spine and two drooping fans of needles.
        for branch in 0..9 {
            let angle = branch as f32 * TAU / 9.0 + 2.18 * tier as f32;
            let (s, c) = angle.sin_cos();
            let out = Vec3::new(c, 0.0, s);
            let side = Vec3::new(-s, 0.0, c);
            let reach = radius * (1.0 + 0.22 * (3.7 * angle).sin());
            let root = Vec3::Y * (y + 0.10 * (1.0 - t));
            let keel = root + out * reach * 0.57;
            let end = root + out * reach - Vec3::Y * reach * 0.51;
            let wing = keel - Vec3::Y * reach * 0.35;
            let a = mesh.vertex(root, (out + Vec3::Y).normalize());
            let b = mesh.vertex(wing - side * reach * 0.27, (out - side * 0.8).normalize());
            let mid = mesh.vertex(keel, (out + Vec3::Y).normalize());
            let tip = mesh.vertex(end, out);
            let d = mesh.vertex(wing + side * reach * 0.27, (out + side * 0.8).normalize());
            mesh.indices
                .extend([a, b, mid, mid, b, tip, a, mid, d, mid, tip, d]);
        }
    }
    mesh
}

// A camera-facing silhouette. Its stepped outline retains individual boughs
// until the whole tree is only a few pixels wide.
fn pine_billboard() -> Mesh {
    let mut mesh = Mesh::default();
    for (row, (y, r)) in [
        (0.0, 0.014),
        (0.14, 0.014),
        (0.14, 0.183),
        (0.33, 0.10),
        (0.32, 0.16),
        (0.50, 0.077),
        (0.49, 0.13),
        (0.68, 0.05),
        (0.67, 0.092),
        (0.82, 0.028),
        (0.81, 0.057),
        (1.0, 0.0),
    ]
    .into_iter()
    .enumerate()
    {
        for side in [-1.0, 0.0, 1.0] {
            let n = Vec3::new(side * 0.75, 0.48, 0.7).normalize();
            mesh.vertex(
                Vec3::new(side * r, y, 0.0),
                if row < 2 { n * 2.0 } else { n },
            );
        }
        if row > 0 {
            let i = row as u32 * 3;
            mesh.quad(i - 3, i, i + 1, i - 2);
            mesh.quad(i - 2, i + 1, i + 2, i - 1);
        }
    }
    mesh
}

struct Stand {
    range: Range<u32>,
    center: Vec3,
    extent: Vec3,
}

fn hash(mut h: u32) -> u32 {
    h = (h ^ (h >> 16)).wrapping_mul(0x7feb352d);
    h = (h ^ (h >> 15)).wrapping_mul(0x846ca68b);
    h ^ (h >> 16)
}

fn scatter(terrain: &Terrain) -> (Vec<[f32; 4]>, Vec<Stand>) {
    const TILES: u32 = 40;
    const CELLS: u32 = 16;
    let tile_size = SIDE_METERS / TILES as f32;
    let spacing = tile_size / CELLS as f32;
    let mut trees = Vec::new();
    let mut stands = Vec::new();
    for tz in 0..TILES {
        for tx in 0..TILES {
            let first = trees.len() as u32;
            let origin = Vec2::new(tx as f32, tz as f32) * tile_size - SIDE_METERS * 0.5;
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for z in 0..CELLS {
                for x in 0..CELLS {
                    let seed = hash((tz * CELLS + z) * (TILES * CELLS) + tx * CELLS + x);
                    let rand = |salt| (hash(seed ^ salt) >> 8) as f32 / 16_777_216.0;
                    let p = origin
                        + (Vec2::new(x as f32, z as f32) + Vec2::new(rand(3), rand(5))) * spacing;
                    let y = terrain.approximate_height(p);
                    let dx = terrain.approximate_height(p + Vec2::X * 4.0) - y;
                    let dz = terrain.approximate_height(p + Vec2::Y * 4.0) - y;
                    let density = alpine::forest_density(p, y, dx.hypot(dz) * 0.25);
                    if rand(7) >= density {
                        continue;
                    }
                    let h = (11.0 + 15.0 * rand(11)) * (0.58 + 0.42 * density);
                    trees.push([p.x, y - 0.8, p.y, h]);
                    lo = lo.min(y - 1.0);
                    hi = hi.max(y + h);
                }
            }
            if first < trees.len() as u32 {
                let mid = origin + tile_size * 0.5;
                stands.push(Stand {
                    range: first..trees.len() as u32,
                    center: Vec3::new(mid.x, (lo + hi) * 0.5, mid.y),
                    extent: Vec3::new(
                        tile_size * 0.5 + 9.0,
                        (hi - lo) * 0.5 + 28.0, // Allow for grounding on coarser terrain triangles.
                        tile_size * 0.5 + 9.0,
                    ),
                });
            }
        }
    }
    (trees, stands)
}

#[derive(Default)]
struct TreeDraws {
    near: Vec<Range<u32>>,
    far: Vec<Range<u32>>,
}

impl TreeDraws {
    fn select(&mut self, stands: &[Stand], planes: &[Vec4; 6], eye: Vec3) {
        self.near.clear();
        self.far.clear();
        for stand in stands {
            if planes.iter().any(|p| {
                p.dot(stand.center.extend(1.0)) + p.truncate().abs().dot(stand.extent) < 0.0
            }) {
                continue;
            }
            let closest = ((stand.center - eye).abs() - stand.extent).max(Vec3::ZERO);
            let ranges = if closest.length_squared() < 240.0_f32.powi(2) {
                &mut self.near
            } else {
                &mut self.far
            };
            if let Some(previous) = ranges.last_mut().filter(|r| r.end == stand.range.start) {
                previous.end = stand.range.end;
            } else {
                ranges.push(stand.range.clone());
            }
        }
    }
}

pub(super) struct AlpineScene {
    pipes: AlpinePipelines,
    lake: GpuMesh,
    ranges: GpuMesh,
    pine: GpuMesh,
    billboard: GpuMesh,
    instances: ID3D12Resource,
    tree_count: u32,
    stands: Vec<Stand>,
    visible: TreeDraws,
    reflected: TreeDraws,
    casters: [TreeDraws; crate::scene::SHADOW_CASCADES],
}

impl AlpineScene {
    pub(super) fn sky_reflection(&self) -> &ID3D12PipelineState {
        &self.pipes.sky_reflection
    }
    pub(super) fn cloud_shadow(&self) -> &ID3D12PipelineState {
        &self.pipes.cloud_shadow
    }
    pub(super) unsafe fn new(
        device: &ID3D12Device,
        list: &ID3D12GraphicsCommandList,
        staging: &mut Vec<ID3D12Resource>,
        terrain: &Terrain,
        pipes: AlpinePipelines,
    ) -> Result<Self> {
        let (trees, stands) = scatter(terrain);
        println!(
            "Alpine basin: {} conifers in {} stands.",
            trees.len(),
            stands.len()
        );
        let instances = upload_to_gpu_with_flags(
            device,
            list,
            &trees,
            D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            staging,
        )?;
        let mut upload = |m| GpuMesh::upload(m, device, list, staging);
        Ok(Self {
            pipes,
            lake: upload(lake_mesh(terrain))?,
            ranges: upload(outer_ranges())?,
            pine: upload(pine_mesh())?,
            billboard: upload(pine_billboard())?,
            instances,
            tree_count: trees.len() as u32,
            stands,
            visible: TreeDraws::default(),
            reflected: TreeDraws::default(),
            casters: Default::default(),
        })
    }

    pub(super) fn prepare(
        &mut self,
        eye: Vec3,
        view: Mat4,
        lights: &[[f32; 16]; crate::scene::SHADOW_CASCADES],
    ) {
        let mirror = Mat4::from_translation(Vec3::Y * (2.0 * alpine::LAKE_LEVEL))
            * Mat4::from_scale(Vec3::new(1.0, -1.0, 1.0));
        self.visible
            .select(&self.stands, &frustum_planes(view), eye);
        self.reflected
            .select(&self.stands, &frustum_planes(view * mirror), eye);
        for (casters, light) in self.casters.iter_mut().zip(lights) {
            casters.select(
                &self.stands,
                &frustum_planes(Mat4::from_cols_array(light)),
                eye,
            );
        }
    }

    pub(super) unsafe fn anchor_trees(
        &self,
        list: &ID3D12GraphicsCommandList,
        frame: u64,
        terrain: &Terrain,
    ) {
        let srv = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE;
        let uav = D3D12_RESOURCE_STATE_UNORDERED_ACCESS;
        transition(list, &self.instances, srv, uav);
        list.SetComputeRootSignature(&self.pipes.anchor_root);
        list.SetComputeRootConstantBufferView(0, frame);
        terrain.bind_active_surface(list, 1, 2);
        list.SetComputeRootUnorderedAccessView(3, self.instances.GetGPUVirtualAddress());
        list.SetComputeRoot32BitConstant(4, self.tree_count, 0);
        list.SetPipelineState(&self.pipes.anchor_trees);
        list.Dispatch(self.tree_count.div_ceil(64), 1, 1);
        transition(list, &self.instances, uav, srv);
    }

    unsafe fn trees(
        &self,
        list: &ID3D12GraphicsCommandList,
        draws: &TreeDraws,
        projection: Projection,
    ) {
        for (billboard, mesh, ranges) in [
            (false, &self.pine, &draws.near),
            (true, &self.billboard, &draws.far),
        ] {
            mesh.bind(list);
            list.SetGraphicsRoot32BitConstant(
                1,
                projection as u32 | (u32::from(billboard) << 2),
                0,
            );
            for range in ranges {
                list.SetGraphicsRootShaderResourceView(
                    4,
                    self.instances.GetGPUVirtualAddress() + u64::from(range.start) * 16,
                );
                list.DrawIndexedInstanced(mesh.count, range.end - range.start, 0, 0, 0);
            }
        }
    }

    pub(super) unsafe fn surfaces(&self, list: &ID3D12GraphicsCommandList, pass: SurfacePass) {
        list.SetGraphicsRoot32BitConstant(1, 0, 0);
        list.SetPipelineState(self.pipes.land.get(pass));
        self.ranges.draw(list);
        list.SetPipelineState(self.pipes.lake.get(pass));
        self.lake.draw(list);
        list.SetPipelineState(self.pipes.pine.get(pass));
        self.trees(list, &self.visible, Projection::Camera);
    }

    pub(super) unsafe fn shadows(&self, list: &ID3D12GraphicsCommandList, cascade: usize) {
        list.SetGraphicsRoot32BitConstant(1, Projection::Shadow as u32, 0);
        list.SetPipelineState(&self.pipes.range_shadow);
        self.ranges.draw(list);
        list.SetPipelineState(&self.pipes.pine_shadow);
        self.trees(list, &self.casters[cascade], Projection::Shadow);
    }

    pub(super) unsafe fn reflection(&self, list: &ID3D12GraphicsCommandList, terrain: &Terrain) {
        terrain.draw(list, &self.pipes.terrain_reflection, Projection::Reflection);
        list.SetPipelineState(&self.pipes.land_reflection);
        self.ranges.draw(list);
        list.SetPipelineState(&self.pipes.pine_reflection);
        self.trees(list, &self.reflected, Projection::Reflection);
    }
}
