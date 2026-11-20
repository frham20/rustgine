use super::alpine::AlpineScene;
use crate::{
    grass::Grass,
    pipeline::{ScenePipelines, SurfacePass, TerrainGraphics},
    scene::SceneOptions,
    terrain::{self, Terrain},
};
use windows::{core::Result, Win32::Graphics::Direct3D12::*};

pub(super) struct RenderScene {
    pub(super) terrain: Terrain,
    pub(super) grass: Option<Grass>,
    pub(super) alpine: AlpineScene,
    terrain_graphics: TerrainGraphics,
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
        let terrain = Terrain::new(device, list, staging)?;
        let alpine = AlpineScene::new(device, list, staging, &terrain, pipelines.alpine)?;
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
        Ok(Self {
            alpine,
            terrain,
            grass,
            terrain_graphics: pipelines.terrain,
        })
    }

    pub(super) unsafe fn draw_shadow(&self, list: &ID3D12GraphicsCommandList, cascade: usize) {
        self.terrain.draw(
            list,
            &self.terrain_graphics.shadow,
            terrain::Projection::Shadow,
        );
        self.alpine.shadows(list, cascade);
    }

    pub(super) unsafe fn draw_surfaces(&self, list: &ID3D12GraphicsCommandList, pass: SurfacePass) {
        self.terrain.draw(
            list,
            self.terrain_graphics.surface.get(pass),
            terrain::Projection::Camera,
        );
        self.alpine.surfaces(list, pass);
    }
}
