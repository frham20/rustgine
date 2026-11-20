use super::alpine::AlpineScene;
use crate::{
    gpu::upload_to_gpu,
    grass::Grass,
    pipeline::{ScenePipelines, SurfacePass, SurfacePipelines, TerrainGraphics},
    scene::{cube_geometry, SceneOptions, Vertex},
    terrain::{self, Terrain},
};
use windows::{
    core::Result,
    Win32::Graphics::{Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Direct3D12::*},
};

pub(super) struct RenderScene {
    pub(super) terrain: Terrain,
    pub(super) grass: Option<Grass>,
    pub(super) alpine: Option<AlpineScene>,
    terrain_graphics: TerrainGraphics,
    cube_surface: SurfacePipelines,
    cube_shadow: ID3D12PipelineState,
    _cube_geometry: ID3D12Resource,
    cube_vertices: D3D12_VERTEX_BUFFER_VIEW,
    cube_count: u32,
}

impl RenderScene {
    pub(super) unsafe fn new(
        device: &ID3D12Device,
        list: &ID3D12GraphicsCommandList,
        graphics_root: &ID3D12RootSignature,
        pipelines: ScenePipelines,
        frame_count: usize,
        options: SceneOptions,
        staging: &mut Vec<ID3D12Resource>,
    ) -> Result<Self> {
        let terrain = Terrain::new(device, list, options.alpine, staging)?;
        let alpine = pipelines
            .alpine
            .map(|p| AlpineScene::new(device, list, staging, &terrain, p))
            .transpose()?;
        let side = terrain::SIDE_METERS;
        let samples = terrain::GRID[0] + 1;
        println!("Terrain: {side} m x {side} m, {samples} x {samples} height samples.");
        let grass = pipelines
            .grass
            .map(|pso| {
                Grass::new(
                    device,
                    graphics_root,
                    pso,
                    frame_count,
                    options.grass_full_density,
                )
            })
            .transpose()?;
        let mesh = cube_geometry();
        let geometry = upload_to_gpu(
            device,
            list,
            &mesh,
            D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER,
            staging,
        )?;
        let vertices = D3D12_VERTEX_BUFFER_VIEW {
            BufferLocation: geometry.GetGPUVirtualAddress(),
            SizeInBytes: size_of_val(mesh.as_slice()) as u32,
            StrideInBytes: size_of::<Vertex>() as u32,
        };
        Ok(Self {
            alpine,
            terrain,
            grass,
            terrain_graphics: pipelines.terrain,
            cube_surface: pipelines.cube,
            cube_shadow: pipelines.cube_shadow,
            _cube_geometry: geometry,
            cube_vertices: vertices,
            cube_count: mesh.len() as u32,
        })
    }

    unsafe fn draw_cube(&self, list: &ID3D12GraphicsCommandList, pipeline: &ID3D12PipelineState) {
        list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        list.IASetVertexBuffers(0, Some(&[self.cube_vertices]));
        list.SetPipelineState(pipeline);
        list.DrawInstanced(self.cube_count, 1, 0, 0);
    }

    pub(super) unsafe fn draw_shadow(&self, list: &ID3D12GraphicsCommandList, cascade: usize) {
        self.terrain.draw(
            list,
            &self.terrain_graphics.shadow,
            terrain::Projection::Shadow,
        );
        if let Some(scene) = &self.alpine {
            scene.shadows(list, cascade);
        } else {
            self.draw_cube(list, &self.cube_shadow);
        }
    }

    pub(super) unsafe fn draw_surfaces(&self, list: &ID3D12GraphicsCommandList, pass: SurfacePass) {
        self.terrain.draw(
            list,
            self.terrain_graphics.surface.get(pass),
            terrain::Projection::Camera,
        );
        if let Some(scene) = &self.alpine {
            scene.surfaces(list, pass);
        } else {
            self.draw_cube(list, self.cube_surface.get(pass));
        }
    }
}
