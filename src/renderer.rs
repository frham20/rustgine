mod cloud_noise;
mod imgui;
mod scene;
mod terrain_textures;

use self::{imgui::Overlay, scene::RenderScene};
use crate::{
    camera::Camera,
    capture::{GpuMark, Readback, SceneTimer},
    display::{choose_device, supports_mesh_shaders, supports_tearing, Output},
    gpu::*,
    grass::{Grass, GrassPath, GrassSettings},
    pipeline::{Pipelines, SurfacePass, TERRAIN_TEXTURES_ROOT},
    scene::{Animation, Frame},
    terrain::LodFeedback,
    AppResult,
};
use ::imgui::{Context, DrawData};
use std::{mem::size_of, path::Path, slice};
use windows::{
    core::{Error, Interface, Result},
    Win32::{
        Foundation::{E_FAIL, HWND, RECT},
        Graphics::{
            Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
            Direct3D12::*,
            Dxgi::{Common::*, *},
        },
    },
};

const BUFFER_COUNT: usize = 2;
const COLOR_RTV: usize = BUFFER_COUNT;
const GBUFFER_RTV: usize = COLOR_RTV + 1;
const CLOUD_RTV: usize = GBUFFER_RTV + GBUFFER_FORMATS.len();
const DEPTH_DSV: usize = 0;
const SHADOW_DSV: usize = 1;
const COLOR_SRV: usize = 0;
const SHADOW_SRV: usize = 1;
const GBUFFER_SRV: usize = SHADOW_SRV + 1;
const DEPTH_SRV: usize = GBUFFER_SRV + GBUFFER_FORMATS.len();
const TERRAIN_TEXTURES_SRV: usize = DEPTH_SRV + 1;
const IMGUI_FONT_SRV: usize = TERRAIN_TEXTURES_SRV + 1;
const CLOUD_NOISE_SRV: usize = IMGUI_FONT_SRV + 1;
const CLOUD_SRV: usize = CLOUD_NOISE_SRV + 1;
pub const SHADOW_MAP_SIZES: [(u32, &str); 4] = [
    (512, "512 x 512"),
    (1024, "1024 x 1024"),
    (2048, "2048 x 2048"),
    (4096, "4096 x 4096"),
];
const DEFAULT_SHADOW_SIZE: u32 = SHADOW_MAP_SIZES[2].0;
const WIREFRAME_BACKGROUND: [f32; 4] = [0.006, 0.01, 0.016, 1.0];

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum RenderMode {
    #[default]
    Normal,
    WireframeOverlay,
    WireframeOnly,
}

impl RenderMode {
    pub fn next(self) -> Self {
        match self {
            Self::Normal => Self::WireframeOverlay,
            Self::WireframeOverlay => Self::WireframeOnly,
            Self::WireframeOnly => Self::Normal,
        }
    }
}
const SAMPLE_STATE: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;

struct FrameSlot {
    allocator: ID3D12CommandAllocator,
    constants: ID3D12Resource,
    lod_feedback: LodFeedback,
    fence_value: u64,
    overlay_mesh: Option<ID3D12Resource>,
}

pub struct Hud<'a> {
    pub fps: [u32; 4],
    pub settings: Option<&'a DrawData>,
}

struct Targets {
    clouds: ID3D12Resource,
    back: Vec<ID3D12Resource>,
    color: ID3D12Resource,
    gbuffer: Vec<ID3D12Resource>,
    depth: ID3D12Resource,
}

impl Targets {
    unsafe fn new(
        device: &ID3D12Device,
        swap: &IDXGISwapChain3,
        rtv: &Descriptors,
        dsv: &Descriptors,
        srv: &Descriptors,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let mut back = Vec::with_capacity(BUFFER_COUNT);
        for i in 0..BUFFER_COUNT {
            let buffer: ID3D12Resource = swap.GetBuffer(i as u32)?;
            device.CreateRenderTargetView(&buffer, None, rtv.cpu(i));
            back.push(buffer);
        }
        let clear = D3D12_CLEAR_VALUE {
            Format: SCENE_FORMAT,
            Anonymous: D3D12_CLEAR_VALUE_0 {
                Color: WIREFRAME_BACKGROUND,
            },
        };
        let color = resource(
            device,
            D3D12_HEAP_TYPE_DEFAULT,
            &texture(
                width,
                height,
                SCENE_FORMAT,
                D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
            ),
            SAMPLE_STATE,
            Some(&clear),
        )?;
        device.CreateRenderTargetView(&color, None, rtv.cpu(COLOR_RTV));
        device.CreateShaderResourceView(&color, None, srv.cpu(COLOR_SRV));
        let clouds = resource(
            device,
            D3D12_HEAP_TYPE_DEFAULT,
            &texture(
                width.div_ceil(4),
                height.div_ceil(4),
                SCENE_FORMAT,
                D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
            ),
            SAMPLE_STATE,
            None,
        )?;
        device.CreateRenderTargetView(&clouds, None, rtv.cpu(CLOUD_RTV));
        device.CreateShaderResourceView(&clouds, None, srv.cpu(CLOUD_SRV));
        let mut gbuffer = Vec::with_capacity(GBUFFER_FORMATS.len());
        for (i, format) in GBUFFER_FORMATS.into_iter().enumerate() {
            let attachment = resource(
                device,
                D3D12_HEAP_TYPE_DEFAULT,
                &texture(
                    width,
                    height,
                    format,
                    D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                ),
                SAMPLE_STATE,
                Some(&D3D12_CLEAR_VALUE {
                    Format: format,
                    Anonymous: D3D12_CLEAR_VALUE_0 { Color: [0.0; 4] },
                }),
            )?;
            device.CreateRenderTargetView(&attachment, None, rtv.cpu(GBUFFER_RTV + i));
            device.CreateShaderResourceView(&attachment, None, srv.cpu(GBUFFER_SRV + i));
            gbuffer.push(attachment);
        }
        let depth = resource(
            device,
            D3D12_HEAP_TYPE_DEFAULT,
            &texture(
                width,
                height,
                DXGI_FORMAT_R32_TYPELESS,
                D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
            ),
            D3D12_RESOURCE_STATE_DEPTH_WRITE,
            Some(&depth_clear()),
        )?;
        create_depth_views(device, &depth, dsv.cpu(DEPTH_DSV), srv.cpu(DEPTH_SRV));
        Ok(Self {
            clouds,
            back,
            color,
            gbuffer,
            depth,
        })
    }
}

unsafe fn create_shadow(
    device: &ID3D12Device,
    dsv: &Descriptors,
    srv: &Descriptors,
    size: u32,
) -> Result<ID3D12Resource> {
    let shadow = resource(
        device,
        D3D12_HEAP_TYPE_DEFAULT,
        &texture(
            size,
            size,
            DXGI_FORMAT_R32_TYPELESS,
            D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
        ),
        SAMPLE_STATE,
        Some(&depth_clear()),
    )?;
    create_depth_views(device, &shadow, dsv.cpu(SHADOW_DSV), srv.cpu(SHADOW_SRV));
    Ok(shadow)
}

unsafe fn create_depth_views(
    device: &ID3D12Device,
    resource: &ID3D12Resource,
    dsv: D3D12_CPU_DESCRIPTOR_HANDLE,
    srv: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    let depth = D3D12_DEPTH_STENCIL_VIEW_DESC {
        Format: DEPTH_FORMAT,
        ViewDimension: D3D12_DSV_DIMENSION_TEXTURE2D,
        ..Default::default()
    };
    device.CreateDepthStencilView(resource, Some(&depth), dsv);
    let view = D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: DXGI_FORMAT_R32_FLOAT,
        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_SRV {
                MipLevels: 1,
                ..Default::default()
            },
        },
    };
    device.CreateShaderResourceView(resource, Some(&view), srv);
}

pub struct Renderer {
    factory: IDXGIFactory4,
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    swap: IDXGISwapChain3,
    swap_flags: DXGI_SWAP_CHAIN_FLAG,
    list: ID3D12GraphicsCommandList,
    timeline: Timeline,
    slots: Vec<FrameSlot>,
    rtv: Descriptors,
    dsv: Descriptors,
    srv: Descriptors,
    targets: Option<Targets>,
    shadow: ID3D12Resource,
    shadow_size: u32,
    overlay: Overlay,
    _terrain_textures: ID3D12Resource,
    _cloud_noise: ID3D12Resource,
    pipes: Pipelines,
    scene: RenderScene,
    width: u32,
    height: u32,
    output: Output,
    requested_output: Output,
    force_sdr: bool,
    debug_queue: Option<ID3D12InfoQueue>,
}

impl Renderer {
    pub unsafe fn new(
        hwnd: HWND,
        [width, height]: [u32; 2],
        warp: bool,
        force_sdr: bool,
        grass_path: GrassPath,
        grass_full_density: bool,
        imgui: &mut Context,
    ) -> Result<Self> {
        let mut debug: Option<ID3D12Debug> = None;
        if cfg!(debug_assertions) {
            if D3D12GetDebugInterface(&mut debug).is_ok() {
                debug.as_ref().unwrap().EnableDebugLayer();
                println!("D3D12 debug layer enabled");
            } else {
                eprintln!("D3D12 debug layer unavailable (Windows Graphics Tools)");
            }
        }
        let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
        let device = choose_device(&factory, warp)?;
        let grass_path = grass_path.resolve(supports_mesh_shaders(&device))?;
        let queue: ID3D12CommandQueue =
            device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC::default())?;
        let swap_flags = if supports_tearing(&factory) {
            DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING
        } else {
            DXGI_SWAP_CHAIN_FLAG(0)
        };
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: SDR_FORMAT,
            SampleDesc: SAMPLE,
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: BUFFER_COUNT as u32,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            Scaling: DXGI_SCALING_STRETCH,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: swap_flags.0 as u32,
            ..Default::default()
        };
        let swap = factory
            .CreateSwapChainForHwnd(&queue, hwnd, &desc, None, None)?
            .cast()?;
        factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;
        let rtv = Descriptors::new(
            &device,
            D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
            CLOUD_RTV as u32 + 1,
        )?;
        let dsv = Descriptors::new(&device, D3D12_DESCRIPTOR_HEAP_TYPE_DSV, 2)?;
        let srv = Descriptors::new(
            &device,
            D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
            CLOUD_SRV as u32 + 1,
        )?;
        let mut slots = Vec::new();
        let bytes = size_of::<Frame>()
            .next_multiple_of(D3D12_CONSTANT_BUFFER_DATA_PLACEMENT_ALIGNMENT as usize);
        for _ in 0..BUFFER_COUNT {
            slots.push(FrameSlot {
                lod_feedback: LodFeedback::new(&device)?,
                allocator: device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?,
                constants: buffer(
                    &device,
                    bytes as u64,
                    D3D12_HEAP_TYPE_UPLOAD,
                    D3D12_RESOURCE_STATE_GENERIC_READ,
                )?,
                fence_value: 0,
                overlay_mesh: None,
            });
        }
        let list: ID3D12GraphicsCommandList = device.CreateCommandList(
            0,
            D3D12_COMMAND_LIST_TYPE_DIRECT,
            &slots[0].allocator,
            None,
        )?;
        let (pipes, scene_pipelines) = Pipelines::new(&device, grass_path)?;
        let mut staging = Vec::new();
        let noise = cloud_noise::load(&device, &list, srv.cpu(CLOUD_NOISE_SRV), &mut staging)?;
        let terrain_textures =
            terrain_textures::load(&device, &list, srv.cpu(TERRAIN_TEXTURES_SRV), &mut staging)?;
        let scene = RenderScene::new(
            &device,
            &list,
            &pipes.root,
            scene_pipelines,
            slots.len(),
            grass_full_density,
            &mut staging,
        )?;
        println!("Grass path: {grass_path}");
        let shadow = create_shadow(&device, &dsv, &srv, DEFAULT_SHADOW_SIZE)?;
        imgui::configure_backend(imgui);
        let overlay = Overlay::new(
            &device,
            &list,
            imgui.fonts(),
            srv.cpu(IMGUI_FONT_SRV),
            srv.gpu(IMGUI_FONT_SRV),
            &mut staging,
        )?;
        list.Close()?;
        let mut timeline = Timeline::new(&device)?;
        queue.ExecuteCommandLists(&[Some(list.cast()?)]);
        let upload_done = timeline.signal(&queue)?;
        timeline.wait(upload_done)?;
        drop(staging);
        let debug_queue = if debug.is_some() {
            device.cast().ok()
        } else {
            None
        };
        let mut renderer = Self {
            device,
            factory,
            queue,
            swap,
            swap_flags,
            list,
            timeline,
            slots,
            rtv,
            dsv,
            srv,
            targets: None,
            shadow,
            shadow_size: DEFAULT_SHADOW_SIZE,
            overlay,
            _terrain_textures: terrain_textures,
            _cloud_noise: noise,
            pipes,
            scene,
            width: 0,
            height: 0,
            force_sdr,
            requested_output: Output::SDR,
            output: Output::SDR,
            debug_queue,
        };
        let requested = renderer.display_for(hwnd)?;
        renderer.configure(width, height, requested)?;
        Ok(renderer)
    }

    unsafe fn display_for(&mut self, hwnd: HWND) -> Result<Output> {
        if self.force_sdr {
            return Ok(Output::SDR);
        }
        if !self.factory.IsCurrent().as_bool() {
            self.factory = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
        }
        Ok(Output::detect(&self.factory, hwnd))
    }

    pub unsafe fn refresh(
        &mut self,
        hwnd: HWND,
        width: u32,
        height: u32,
        check_display: bool,
    ) -> Result<()> {
        let requested = if check_display {
            self.display_for(hwnd)?
        } else {
            self.requested_output
        };
        if (width, height, requested) != (self.width, self.height, self.requested_output) {
            self.configure(width, height, requested)?;
        }
        Ok(())
    }

    unsafe fn configure(&mut self, width: u32, height: u32, requested: Output) -> Result<()> {
        self.idle()?;
        self.targets.take(); // Release every GetBuffer reference before ResizeBuffers.
        let configure_swap = |mode: Output| -> Result<()> {
            self.swap.ResizeBuffers(
                BUFFER_COUNT as u32,
                width,
                height,
                mode.format(),
                self.swap_flags,
            )?;
            mode.apply(&self.swap)
        };
        let mut output = requested;
        if let Err(e) = configure_swap(requested) {
            if !requested.hdr {
                return Err(e);
            }
            eprintln!("scRGB presentation unavailable ({e}). Selecting SDR.");
            output = Output::SDR;
            configure_swap(output)?;
        }
        self.targets = Some(Targets::new(
            &self.device,
            &self.swap,
            &self.rtv,
            &self.dsv,
            &self.srv,
            width,
            height,
        )?);
        if self.width == 0 || self.output != output {
            println!(
                "Presentation: {} ({} nits)",
                if output.hdr { "HDR scRGB" } else { "SDR sRGB" },
                output.peak_nits
            );
        }
        (self.width, self.height, self.output, self.requested_output) =
            (width, height, output, requested);
        Ok(())
    }

    unsafe fn viewport(&self, width: u32, height: u32) {
        self.list.RSSetViewports(&[D3D12_VIEWPORT {
            Width: width as f32,
            Height: height as f32,
            MaxDepth: 1.0,
            ..Default::default()
        }]);
        self.list.RSSetScissorRects(&[RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        }]);
    }

    unsafe fn shadow_pass(&self) {
        let list = &self.list;
        transition(
            list,
            &self.shadow,
            SAMPLE_STATE,
            D3D12_RESOURCE_STATE_DEPTH_WRITE,
        );
        self.viewport(self.shadow_size, self.shadow_size);
        let depth = self.dsv.cpu(SHADOW_DSV);
        list.OMSetRenderTargets(0, None, false, Some(&depth));
        list.ClearDepthStencilView(depth, D3D12_CLEAR_FLAG_DEPTH, 1.0, 0, &[]);
        self.scene.draw_shadow(list);
        transition(
            list,
            &self.shadow,
            D3D12_RESOURCE_STATE_DEPTH_WRITE,
            SAMPLE_STATE,
        );
    }

    unsafe fn cloud_pass(&self, targets: &Targets) {
        let list = &self.list;
        transition(
            list,
            &targets.clouds,
            SAMPLE_STATE,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
        );
        list.OMSetRenderTargets(1, Some(&self.rtv.cpu(CLOUD_RTV)), false, None);
        self.viewport(self.width.div_ceil(4), self.height.div_ceil(4));
        list.SetGraphicsRootDescriptorTable(TERRAIN_TEXTURES_ROOT, self.srv.gpu(CLOUD_NOISE_SRV));
        list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        list.SetPipelineState(&self.pipes.clouds);
        list.DrawInstanced(3, 1, 0, 0);
        transition(
            list,
            &targets.clouds,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            SAMPLE_STATE,
        );
        self.viewport(self.width, self.height);
    }

    unsafe fn lighting_pass(&self, targets: &Targets) {
        let list = &self.list;
        list.OMSetRenderTargets(1, Some(&self.rtv.cpu(COLOR_RTV)), false, None);
        for attachment in &targets.gbuffer {
            transition(
                list,
                attachment,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                SAMPLE_STATE,
            );
        }
        transition(
            list,
            &targets.depth,
            D3D12_RESOURCE_STATE_DEPTH_WRITE,
            SAMPLE_STATE,
        );
        list.SetGraphicsRootDescriptorTable(3, self.srv.gpu(SHADOW_SRV));
        list.SetGraphicsRootDescriptorTable(7, self.srv.gpu(GBUFFER_SRV));
        list.SetGraphicsRootDescriptorTable(2, self.srv.gpu(CLOUD_SRV));
        list.SetPipelineState(&self.pipes.lighting);
        list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        list.DrawInstanced(3, 1, 0, 0);
        transition(
            list,
            &targets.depth,
            SAMPLE_STATE,
            D3D12_RESOURCE_STATE_DEPTH_WRITE,
        );
    }

    unsafe fn scene_pass(
        &self,
        targets: &Targets,
        frame_index: usize,
        mode: RenderMode,
        timer: Option<&SceneTimer>,
    ) {
        let list = &self.list;
        transition(
            list,
            &targets.color,
            SAMPLE_STATE,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
        );
        self.viewport(self.width, self.height);
        let color = self.rtv.cpu(COLOR_RTV);
        let depth = self.dsv.cpu(DEPTH_DSV);
        list.ClearDepthStencilView(depth, D3D12_CLEAR_FLAG_DEPTH, 1.0, 0, &[]);
        let base_pass = if mode == RenderMode::WireframeOnly {
            list.ClearRenderTargetView(color, &WIREFRAME_BACKGROUND, None);
            // Write solid depth so hidden edges stay out of the wireframe.
            list.OMSetRenderTargets(0, None, false, Some(&depth));
            SurfacePass::Depth
        } else {
            for attachment in &targets.gbuffer {
                transition(
                    list,
                    attachment,
                    SAMPLE_STATE,
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                );
            }
            // The clear value marks uncovered pixels as sky. Lighting reads albedo only on geometry.
            list.ClearRenderTargetView(self.rtv.cpu(GBUFFER_RTV + 1), &[0.0; 4], None);
            list.OMSetRenderTargets(
                GBUFFER_FORMATS.len() as u32,
                Some(&self.rtv.cpu(GBUFFER_RTV)),
                true,
                Some(&depth),
            );
            list.SetGraphicsRootDescriptorTable(
                TERRAIN_TEXTURES_ROOT,
                self.srv.gpu(TERRAIN_TEXTURES_SRV),
            );
            SurfacePass::Geometry
        };
        self.scene.draw_surfaces(list, base_pass);
        if let Some(timer) = timer {
            timer.mark(list, GpuMark::SurfacesReady);
        }
        if let Some(grass) = &self.scene.grass {
            grass.draw(list, frame_index, base_pass);
        }
        if let Some(timer) = timer {
            timer.mark(list, GpuMark::GeometryReady);
        }
        if mode != RenderMode::WireframeOnly {
            self.cloud_pass(targets);
        }
        if let Some(timer) = timer {
            timer.mark(list, GpuMark::CloudsReady);
        }
        if mode != RenderMode::WireframeOnly {
            self.lighting_pass(targets);
        }
        if mode != RenderMode::Normal {
            list.OMSetRenderTargets(1, Some(&color), false, Some(&depth));
            self.scene.draw_surfaces(list, SurfacePass::Wireframe);
            if let Some(grass) = &self.scene.grass {
                grass.draw(list, frame_index, SurfacePass::Wireframe);
            }
        }
        transition(
            list,
            &targets.color,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            SAMPLE_STATE,
        );
        if let Some(timer) = timer {
            timer.mark(list, GpuMark::SceneReady);
        }
    }

    pub fn adjust_grass_distance(&mut self, increase: bool) {
        if let Some(grass) = &mut self.scene.grass {
            grass.adjust_distance(increase);
        }
    }

    pub fn grass_settings(&mut self) -> Option<&mut GrassSettings> {
        self.scene.grass.as_mut().map(|grass| &mut grass.settings)
    }

    pub fn shadow_size(&self) -> u32 {
        self.shadow_size
    }

    pub unsafe fn set_shadow_size(&mut self, size: u32) -> Result<()> {
        if size != self.shadow_size {
            // Both descriptors may be referenced by submitted command lists.
            self.idle()?;
            self.shadow = create_shadow(&self.device, &self.dsv, &self.srv, size)?;
            self.shadow_size = size;
            println!("Shadow map: {size} x {size}.");
        }
        Ok(())
    }

    pub unsafe fn draw(
        &mut self,
        camera: &Camera,
        animation: Animation,
        exposure: f32,
        mode: RenderMode,
        capture: Option<&Path>,
        hud: Hud<'_>,
    ) -> AppResult<()> {
        let i = self.swap.GetCurrentBackBufferIndex() as usize;
        let slot = &mut self.slots[i];
        self.timeline.wait(slot.fence_value)?;
        let mut frame = Frame::new(
            [self.width, self.height],
            camera,
            animation,
            exposure,
            self.output.hdr,
            self.output.peak_nits,
            self.scene.terrain.approximate_height(glam::Vec2::ZERO),
        );
        frame.fps_hud = hud.fps;
        if let Some(grass) = &self.scene.grass {
            frame.grass[1..].copy_from_slice(&grass.settings.color_scale());
        }
        upload(&slot.constants, slice::from_ref(&frame))?;
        let timer = capture.map(|_| SceneTimer::new(&self.device)).transpose()?;
        slot.allocator.Reset()?;
        let list = &self.list;
        list.Reset(&slot.allocator, None)?;
        if let Some(timer) = &timer {
            timer.mark(list, GpuMark::Start);
        }
        self.scene.terrain.prepare(
            list,
            slot.constants.GetGPUVirtualAddress(),
            frame.terrain_lod_view(),
            &mut slot.lod_feedback,
        )?;
        if let Some(timer) = &timer {
            timer.mark(list, GpuMark::TerrainReady);
        }
        if let Some(grass) = &mut self.scene.grass {
            grass.prepare(
                &self.device,
                list,
                i,
                slot.constants.GetGPUVirtualAddress(),
                &self.scene.terrain,
            )?;
        }
        if let Some(timer) = &timer {
            timer.mark(list, GpuMark::GrassReady);
        }
        list.SetGraphicsRootSignature(&self.pipes.root);
        list.SetDescriptorHeaps(&[Some(self.srv.heap.clone())]);
        list.SetGraphicsRootConstantBufferView(0, slot.constants.GetGPUVirtualAddress());
        list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        let targets = self.targets.as_ref().unwrap();
        if mode != RenderMode::WireframeOnly && frame.sun[1] > 0.0 {
            self.shadow_pass();
        }
        if let Some(timer) = &timer {
            timer.mark(list, GpuMark::ShadowsReady);
        }
        self.scene_pass(targets, i, mode, timer.as_ref());
        let back = &targets.back[i];
        transition(
            list,
            back,
            D3D12_RESOURCE_STATE_PRESENT,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
        );
        list.OMSetRenderTargets(1, Some(&self.rtv.cpu(i)), false, None);
        list.SetPipelineState(if self.output.hdr {
            &self.pipes.hdr
        } else {
            &self.pipes.sdr
        });
        list.SetGraphicsRootDescriptorTable(2, self.srv.gpu(COLOR_SRV));
        list.DrawInstanced(3, 1, 0, 0);
        if let Some(timer) = &timer {
            timer.finish(list);
        }
        if let Some(settings) = hud.settings {
            self.overlay.draw(
                &self.device,
                list,
                &mut self.slots[i].overlay_mesh,
                settings,
                self.output.hdr,
            )?;
        }
        let readbacks = if capture.is_some() {
            transition(
                list,
                back,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            );
            transition(
                list,
                &targets.color,
                SAMPLE_STATE,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            );
            let scene = Readback::record(&self.device, list, &targets.color)?;
            let display = Readback::record(&self.device, list, back)?;
            let terrain_count = self.scene.terrain.capture_count(&self.device, list)?;
            let grass_counts = self
                .scene
                .grass
                .as_ref()
                .map(|grass| grass.capture_counts(&self.device, list, i))
                .transpose()?
                .flatten();
            transition(
                list,
                &targets.color,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                SAMPLE_STATE,
            );
            transition(
                list,
                back,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_PRESENT,
            );
            Some((scene, display, terrain_count, grass_counts))
        } else {
            transition(
                list,
                back,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_PRESENT,
            );
            None
        };
        list.Close()?;
        self.queue.ExecuteCommandLists(&[Some(list.cast()?)]);
        let present_flags = if self.swap_flags.contains(DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING) {
            DXGI_PRESENT_ALLOW_TEARING
        } else {
            DXGI_PRESENT(0)
        };
        self.swap.Present(0, present_flags).ok()?;
        self.slots[i].fence_value = self.timeline.signal(&self.queue)?;
        if let (Some(path), Some((scene, display, terrain_count, grass_counts))) =
            (capture, readbacks)
        {
            self.timeline.wait(self.slots[i].fence_value)?;
            let (peak, count) = scene.scene_peak()?;
            display.write_ppm(path)?;
            println!(
                "Saved {}. Scene-linear RGB peak {peak:.3}, {count} pixels above 1.0.",
                path.display()
            );
            if let Some(timer) = timer {
                let [prepare, grass_generate, shadows, surfaces, grass_ms, clouds, resolve, output] =
                    timer.pass_milliseconds(self.queue.GetTimestampFrequency()?)?;
                let scene_ms =
                    prepare + grass_generate + shadows + surfaces + grass_ms + clouds + resolve;
                let triangles = readback_u32(&terrain_count)?;
                println!("Terrain: {triangles} active triangles.");
                let (patches, blades_triangles) = grass_counts
                    .as_ref()
                    .map(|readback| Grass::read_counts(readback))
                    .transpose()?
                    .unwrap_or_default();
                if let Some(grass) = &self.scene.grass {
                    println!("Grass distance: {} m.", grass.draw_distance());
                }
                print!("Grass: {patches} visible patches, {blades_triangles} candidate triangles. GPU: {scene_ms:.3} ms scene");
                let resolve_label = match mode {
                    RenderMode::Normal => "lighting",
                    RenderMode::WireframeOverlay => "lighting + wireframe",
                    RenderMode::WireframeOnly => "wireframe",
                };
                println!(", {grass_ms:.3} ms grass geometry.");
                println!("GPU passes (ms): terrain {prepare:.3}, grass generation {grass_generate:.3}, shadows {shadows:.3}, surfaces {surfaces:.3}, grass {grass_ms:.3}, clouds {clouds:.3}, {resolve_label} {resolve:.3}, output {output:.3}, total {:.3}.", scene_ms + output);
            }
        }
        self.check_debug()?;
        Ok(())
    }

    unsafe fn check_debug(&self) -> Result<()> {
        let Some(info) = &self.debug_queue else {
            return Ok(());
        };
        let mut error = false;
        for i in 0..info.GetNumStoredMessagesAllowedByRetrievalFilter() {
            let mut bytes = 0;
            info.GetMessage(i, None, &mut bytes)?;
            let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
            let message = storage.as_mut_ptr().cast::<D3D12_MESSAGE>();
            info.GetMessage(i, Some(message), &mut bytes)?;
            let m = &*message;
            if m.Severity.0 <= D3D12_MESSAGE_SEVERITY_WARNING.0 {
                let desc = slice::from_raw_parts(m.pDescription, m.DescriptionByteLength);
                eprintln!(
                    "D3D12 {:?}: {}",
                    m.Severity,
                    String::from_utf8_lossy(desc).trim_end_matches('\0')
                );
                error |= m.Severity.0 <= D3D12_MESSAGE_SEVERITY_ERROR.0;
            }
        }
        info.ClearStoredMessages();
        if error {
            return Err(Error::new(
                E_FAIL,
                "D3D12 validation error (see diagnostic output)",
            ));
        }
        Ok(())
    }

    unsafe fn idle(&mut self) -> Result<()> {
        let value = self.timeline.signal(&self.queue)?;
        self.timeline.wait(value)
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            let _ = self.idle();
        }
    }
}
