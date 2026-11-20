use crate::{
    gpu::{DEPTH_FORMAT, GBUFFER_FORMATS, SAMPLE, SCENE_FORMAT, SDR_FORMAT},
    grass::{GrassPath, GrassPipeline},
};
use std::{
    ffi::c_void,
    mem::{size_of_val, ManuallyDrop},
    ptr, slice,
};
use windows::{
    core::{s, Interface, Result},
    Win32::Graphics::{Direct3D12::*, Dxgi::Common::*},
};

macro_rules! shader {
    ($name:literal) => {{
        let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/", $name, ".dxil"));
        D3D12_SHADER_BYTECODE {
            pShaderBytecode: bytes.as_ptr().cast(),
            BytecodeLength: bytes.len(),
        }
    }};
}
pub(crate) use shader;
pub const TERRAIN_TEXTURES_ROOT: u32 = 8;
pub const GRASS_PATCHES_ROOT: u32 = 4;
pub const GRASS_BASE_ROOT: u32 = 9;
pub const GRASS_STATS_ROOT: u32 = 10;
pub const REFLECTION_ROOT: u32 = 11;
pub const CASCADE_ROOT: u32 = 12;
pub const CLOUD_SHADOW_ROOT: u32 = 13;
pub const LANDFORMS_ROOT: u32 = 14;
pub const DETAIL_SLOPES_ROOT: u32 = 15;

pub fn root_descriptor(kind: D3D12_ROOT_PARAMETER_TYPE, register: u32) -> D3D12_ROOT_PARAMETER {
    D3D12_ROOT_PARAMETER {
        ParameterType: kind,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Descriptor: D3D12_ROOT_DESCRIPTOR {
                ShaderRegister: register,
                RegisterSpace: 0,
            },
        },
    }
}

pub unsafe fn serialize_signature(
    device: &ID3D12Device,
    desc: &D3D12_ROOT_SIGNATURE_DESC,
) -> Result<ID3D12RootSignature> {
    let mut blob = None;
    D3D12SerializeRootSignature(desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, None)?;
    let blob = blob.unwrap();
    device.CreateRootSignature(
        0,
        slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize()),
    )
}

unsafe fn signature(device: &ID3D12Device) -> Result<ID3D12RootSignature> {
    let ranges = [
        (0, 1),
        (1, 1),
        (9, 3),
        (12, 1),
        (14, 1),
        (15, 1),
        (16, 1),
        (17, 1),
    ]
    .map(|(register, count)| D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: count,
        BaseShaderRegister: register,
        ..Default::default()
    });
    let table = |index: usize| D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                NumDescriptorRanges: 1,
                pDescriptorRanges: &ranges[index],
            },
        },
    };
    let mut params = vec![
        root_descriptor(D3D12_ROOT_PARAMETER_TYPE_CBV, 0),
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 1,
                    RegisterSpace: 0,
                    Num32BitValues: 1,
                },
            },
        },
        table(0),
        table(1),
        root_descriptor(D3D12_ROOT_PARAMETER_TYPE_SRV, 2),
    ];
    params.extend([7, 8].map(|register| root_descriptor(D3D12_ROOT_PARAMETER_TYPE_SRV, register)));
    params.push(table(2));
    debug_assert_eq!(params.len() as u32, TERRAIN_TEXTURES_ROOT);
    params.push(table(3));
    debug_assert_eq!(params.len() as u32, GRASS_BASE_ROOT);
    params.push(D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_VERTEX,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Constants: D3D12_ROOT_CONSTANTS {
                ShaderRegister: 2,
                RegisterSpace: 0,
                Num32BitValues: 1,
            },
        },
    });
    debug_assert_eq!(params.len() as u32, GRASS_STATS_ROOT);
    params.push(root_descriptor(D3D12_ROOT_PARAMETER_TYPE_SRV, 13));
    params.push(table(4));
    params.push(D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_VERTEX,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Constants: D3D12_ROOT_CONSTANTS {
                ShaderRegister: 3,
                RegisterSpace: 0,
                Num32BitValues: 1,
            },
        },
    });
    params.push(table(5));
    params.push(D3D12_ROOT_PARAMETER {
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        ..table(6)
    });
    params.push(table(7));
    let shadow_sampler = D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_COMPARISON_MIN_MAG_LINEAR_MIP_POINT,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_BORDER,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_BORDER,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_BORDER,
        BorderColor: D3D12_STATIC_BORDER_COLOR_OPAQUE_WHITE,
        ComparisonFunc: D3D12_COMPARISON_FUNC_LESS_EQUAL,
        MaxAnisotropy: 1,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        ..Default::default()
    };
    let terrain_sampler = D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_ANISOTROPIC,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
        MaxAnisotropy: D3D12_MAX_MAXANISOTROPY,
        ComparisonFunc: D3D12_COMPARISON_FUNC_ALWAYS,
        MaxLOD: f32::MAX,
        ShaderRegister: 1,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        ..Default::default()
    };
    let cloud_sampler = D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
        ShaderRegister: 2,
        MaxAnisotropy: 1,
        ..terrain_sampler
    };
    let landform_sampler = D3D12_STATIC_SAMPLER_DESC {
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        ShaderRegister: 3,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        ..terrain_sampler
    };
    let samplers = [
        shadow_sampler,
        terrain_sampler,
        cloud_sampler,
        landform_sampler,
    ];
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: params.len() as u32,
        pParameters: params.as_ptr(),
        NumStaticSamplers: samplers.len() as u32,
        pStaticSamplers: samplers.as_ptr(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
    };
    serialize_signature(device, &desc)
}

// D3D12 reads a native stream of tagged, pointer-aligned subobjects on x64.
#[repr(C, align(8))]
struct Subobject<T>(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE, T);

#[repr(C)]
struct MeshPipelineStream {
    root: Subobject<*mut c_void>,
    mesh: Subobject<D3D12_SHADER_BYTECODE>,
    pixel: Subobject<D3D12_SHADER_BYTECODE>,
    blend: Subobject<D3D12_BLEND_DESC>,
    rasterizer: Subobject<D3D12_RASTERIZER_DESC>,
    depth: Subobject<D3D12_DEPTH_STENCIL_DESC>,
    color_format: Subobject<D3D12_RT_FORMAT_ARRAY>,
    depth_format: Subobject<DXGI_FORMAT>,
    sample: Subobject<DXGI_SAMPLE_DESC>,
    sample_mask: Subobject<u32>,
}

unsafe fn mesh_pipeline(
    device: &ID3D12Device,
    root: &ID3D12RootSignature,
    states: &D3D12_GRAPHICS_PIPELINE_STATE_DESC,
    pixel: D3D12_SHADER_BYTECODE,
) -> Result<ID3D12PipelineState> {
    let stream = MeshPipelineStream {
        root: Subobject(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_ROOT_SIGNATURE,
            root.as_raw(),
        ),
        mesh: Subobject(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_MS, shader!("grass_ms")),
        pixel: Subobject(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_PS, pixel),
        blend: Subobject(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_BLEND, states.BlendState),
        rasterizer: Subobject(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_RASTERIZER,
            states.RasterizerState,
        ),
        depth: Subobject(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_DEPTH_STENCIL,
            states.DepthStencilState,
        ),
        color_format: Subobject(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_RENDER_TARGET_FORMATS,
            D3D12_RT_FORMAT_ARRAY {
                RTFormats: states.RTVFormats,
                NumRenderTargets: states.NumRenderTargets,
            },
        ),
        depth_format: Subobject(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_DEPTH_STENCIL_FORMAT,
            states.DSVFormat,
        ),
        sample: Subobject(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_SAMPLE_DESC,
            states.SampleDesc,
        ),
        sample_mask: Subobject(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_SAMPLE_MASK,
            states.SampleMask,
        ),
    };
    device
        .cast::<ID3D12Device2>()?
        .CreatePipelineState(&D3D12_PIPELINE_STATE_STREAM_DESC {
            SizeInBytes: size_of_val(&stream),
            pPipelineStateSubobjectStream: ptr::from_ref(&stream).cast_mut().cast(),
        })
}

pub struct Pipelines {
    pub root: ID3D12RootSignature,

    pub sdr: ID3D12PipelineState,
    pub hdr: ID3D12PipelineState,
    pub lighting: ID3D12PipelineState,
    pub clouds: ID3D12PipelineState,
}

pub struct ScenePipelines {
    pub cube: SurfacePipelines,
    pub cube_shadow: ID3D12PipelineState,
    pub grass: Option<GrassPipeline>,
    pub terrain: TerrainGraphics,
    pub alpine: Option<AlpinePipelines>,
}

pub struct AlpinePipelines {
    pub anchor_root: ID3D12RootSignature,
    pub anchor_trees: ID3D12PipelineState,
    pub land: SurfacePipelines,
    pub lake: SurfacePipelines,
    pub pine: SurfacePipelines,
    pub pine_shadow: ID3D12PipelineState,
    pub range_shadow: ID3D12PipelineState,
    pub cloud_shadow: ID3D12PipelineState,
    pub land_reflection: ID3D12PipelineState,
    pub terrain_reflection: ID3D12PipelineState,
    pub pine_reflection: ID3D12PipelineState,
    pub sky_reflection: ID3D12PipelineState,
}

pub struct TerrainGraphics {
    pub surface: SurfacePipelines,
    pub shadow: ID3D12PipelineState,
}

#[derive(Clone, Copy)]
pub enum SurfacePass {
    Geometry,
    Depth,
    Wireframe,
}

pub struct SurfacePipelines {
    geometry: ID3D12PipelineState,
    depth: ID3D12PipelineState,
    wireframe: ID3D12PipelineState,
}

impl SurfacePipelines {
    fn new(
        surface: D3D12_GRAPHICS_PIPELINE_STATE_DESC,
        pixel: D3D12_SHADER_BYTECODE,
        create: impl Fn(
            D3D12_GRAPHICS_PIPELINE_STATE_DESC,
            D3D12_SHADER_BYTECODE,
        ) -> Result<ID3D12PipelineState>,
    ) -> Result<Self> {
        let mut depth = surface.clone();
        depth.NumRenderTargets = 0;
        depth.RTVFormats[0] = DXGI_FORMAT_UNKNOWN;

        let mut wireframe = surface.clone();
        wireframe.RasterizerState.FillMode = D3D12_FILL_MODE_WIREFRAME;
        // Bias the lines slightly toward the camera so they pass against their own surfaces.
        wireframe.RasterizerState.DepthBias = 2;
        wireframe.RasterizerState.SlopeScaledDepthBias = 1.0;
        wireframe.RasterizerState.DepthBiasClamp = 0.000025;
        wireframe.DepthStencilState.DepthWriteMask = D3D12_DEPTH_WRITE_MASK_ZERO;
        let blend = &mut wireframe.BlendState.RenderTarget[0];
        blend.BlendEnable = true.into();
        blend.SrcBlend = D3D12_BLEND_SRC_ALPHA;
        blend.DestBlend = D3D12_BLEND_INV_SRC_ALPHA;

        let mut geometry = surface;
        geometry.NumRenderTargets = GBUFFER_FORMATS.len() as u32;
        geometry.RTVFormats[..GBUFFER_FORMATS.len()].copy_from_slice(&GBUFFER_FORMATS);
        Ok(Self {
            geometry: create(geometry, pixel)?,
            depth: create(depth, Default::default())?,
            wireframe: create(wireframe, shader!("wireframe_ps"))?,
        })
    }

    pub fn get(&self, pass: SurfacePass) -> &ID3D12PipelineState {
        match pass {
            SurfacePass::Geometry => &self.geometry,
            SurfacePass::Depth => &self.depth,
            SurfacePass::Wireframe => &self.wireframe,
        }
    }
}

impl Pipelines {
    pub unsafe fn new(
        device: &ID3D12Device,
        grass_path: GrassPath,
        alpine: bool,
    ) -> Result<(Self, ScenePipelines)> {
        let root = signature(device)?;
        let layout = [(s!("POSITION"), 0), (s!("NORMAL"), 12)].map(|(semantic, offset)| {
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: semantic,
                Format: DXGI_FORMAT_R32G32B32_FLOAT,
                AlignedByteOffset: offset,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                ..Default::default()
            }
        });
        let blend = D3D12_RENDER_TARGET_BLEND_DESC {
            SrcBlend: D3D12_BLEND_ONE,
            DestBlend: D3D12_BLEND_ZERO,
            BlendOp: D3D12_BLEND_OP_ADD,
            SrcBlendAlpha: D3D12_BLEND_ONE,
            DestBlendAlpha: D3D12_BLEND_ZERO,
            BlendOpAlpha: D3D12_BLEND_OP_ADD,
            LogicOp: D3D12_LOGIC_OP_NOOP,
            RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
            ..Default::default()
        };
        let stencil = D3D12_DEPTH_STENCILOP_DESC {
            StencilFailOp: D3D12_STENCIL_OP_KEEP,
            StencilDepthFailOp: D3D12_STENCIL_OP_KEEP,
            StencilPassOp: D3D12_STENCIL_OP_KEEP,
            StencilFunc: D3D12_COMPARISON_FUNC_ALWAYS,
        };
        let states = |depth: bool, shadow: bool, format| {
            let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                BlendState: D3D12_BLEND_DESC {
                    RenderTarget: [blend; 8],
                    ..Default::default()
                },
                RasterizerState: D3D12_RASTERIZER_DESC {
                    FillMode: D3D12_FILL_MODE_SOLID,
                    CullMode: D3D12_CULL_MODE_NONE,
                    DepthClipEnable: true.into(),
                    DepthBias: if shadow { 100 } else { 0 },
                    SlopeScaledDepthBias: if shadow { 1.25 } else { 0.0 },
                    ..Default::default()
                },
                DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
                    DepthEnable: depth.into(),
                    DepthWriteMask: if depth {
                        D3D12_DEPTH_WRITE_MASK_ALL
                    } else {
                        D3D12_DEPTH_WRITE_MASK_ZERO
                    },
                    DepthFunc: if shadow {
                        D3D12_COMPARISON_FUNC_LESS_EQUAL
                    } else {
                        D3D12_COMPARISON_FUNC_GREATER_EQUAL
                    },
                    FrontFace: stencil,
                    BackFace: stencil,
                    ..Default::default()
                },
                SampleMask: u32::MAX,
                PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                NumRenderTargets: (!shadow) as u32,
                DSVFormat: if depth {
                    DEPTH_FORMAT
                } else {
                    DXGI_FORMAT_UNKNOWN
                },
                SampleDesc: SAMPLE,
                ..Default::default()
            };
            desc.RTVFormats[0] = format;
            desc
        };
        let graphics = |mut desc: D3D12_GRAPHICS_PIPELINE_STATE_DESC, vs, ps, vertex_buffer| {
            desc.pRootSignature = ManuallyDrop::new(Some(root.clone()));
            desc.VS = vs;
            desc.PS = ps;
            if vertex_buffer {
                desc.InputLayout = D3D12_INPUT_LAYOUT_DESC {
                    pInputElementDescs: layout.as_ptr(),
                    NumElements: layout.len() as u32,
                };
            }
            let result = device.CreateGraphicsPipelineState(&desc);
            ManuallyDrop::drop(&mut desc.pRootSignature);
            result
        };
        let sdr = graphics(
            states(false, false, SDR_FORMAT),
            shader!("output_vs"),
            shader!("output_ps"),
            false,
        )?;
        let hdr = graphics(
            states(false, false, SCENE_FORMAT),
            shader!("output_vs"),
            shader!("output_ps"),
            false,
        )?;
        let terrain = {
            let terrain_pipeline =
                |desc, pixel| graphics(desc, shader!("terrain_vs"), pixel, false);
            TerrainGraphics {
                surface: SurfacePipelines::new(
                    states(true, false, SCENE_FORMAT),
                    shader!("terrain_ps"),
                    terrain_pipeline,
                )?,
                shadow: terrain_pipeline(
                    states(true, true, DXGI_FORMAT_UNKNOWN),
                    Default::default(),
                )?,
            }
        };
        let shadow = graphics(
            states(true, true, DXGI_FORMAT_UNKNOWN),
            shader!("shadow_vs"),
            Default::default(),
            true,
        )?;
        let scene = SurfacePipelines::new(
            states(true, false, SCENE_FORMAT),
            shader!("scene_ps"),
            |desc, pixel| graphics(desc, shader!("scene_vs"), pixel, true),
        )?;
        let grass = match grass_path {
            GrassPath::Mesh => Some(GrassPipeline::Mesh(SurfacePipelines::new(
                states(true, false, SCENE_FORMAT),
                shader!("grass_ps"),
                |desc, pixel| mesh_pipeline(device, &root, &desc, pixel),
            )?)),
            GrassPath::Instanced => Some(GrassPipeline::Instanced(SurfacePipelines::new(
                states(true, false, SCENE_FORMAT),
                shader!("grass_ps"),
                |desc, pixel| graphics(desc, shader!("grass_vs"), pixel, false),
            )?)),
            GrassPath::Auto | GrassPath::Off => None,
        };
        let alpine = if alpine {
            let mut params = [
                root_descriptor(D3D12_ROOT_PARAMETER_TYPE_CBV, 0),
                root_descriptor(D3D12_ROOT_PARAMETER_TYPE_SRV, 7),
                root_descriptor(D3D12_ROOT_PARAMETER_TYPE_SRV, 8),
                root_descriptor(D3D12_ROOT_PARAMETER_TYPE_UAV, 0),
                D3D12_ROOT_PARAMETER::default(),
            ];
            params[4] = D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    Constants: D3D12_ROOT_CONSTANTS {
                        ShaderRegister: 1,
                        RegisterSpace: 0,
                        Num32BitValues: 1,
                    },
                },
                ..Default::default()
            };
            let anchor_root = serialize_signature(
                device,
                &D3D12_ROOT_SIGNATURE_DESC {
                    NumParameters: params.len() as u32,
                    pParameters: params.as_ptr(),
                    ..Default::default()
                },
            )?;
            let mut anchor_desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
                pRootSignature: ManuallyDrop::new(Some(anchor_root.clone())),
                CS: shader!("anchor_trees"),
                ..Default::default()
            };
            let anchor_trees = device.CreateComputePipelineState(&anchor_desc);
            ManuallyDrop::drop(&mut anchor_desc.pRootSignature);
            let surface = |vs, ps| {
                SurfacePipelines::new(states(true, false, SCENE_FORMAT), ps, |desc, pixel| {
                    graphics(desc, vs, pixel, true)
                })
            };
            let reflection = |vs, ps| graphics(states(true, false, SCENE_FORMAT), vs, ps, true);
            Some(AlpinePipelines {
                anchor_root,
                anchor_trees: anchor_trees?,
                land: surface(shader!("land_vs"), shader!("land_ps"))?,
                lake: surface(shader!("lake_vs"), shader!("lake_ps"))?,
                pine: surface(shader!("pine_vs"), shader!("pine_ps"))?,
                pine_shadow: graphics(
                    states(true, true, DXGI_FORMAT_UNKNOWN),
                    shader!("pine_vs"),
                    Default::default(),
                    true,
                )?,
                range_shadow: graphics(
                    states(true, true, DXGI_FORMAT_UNKNOWN),
                    shader!("land_vs"),
                    Default::default(),
                    true,
                )?,
                cloud_shadow: graphics(
                    states(false, false, DXGI_FORMAT_R8_UNORM),
                    shader!("output_vs"),
                    shader!("cloud_shadow_ps"),
                    false,
                )?,
                land_reflection: reflection(shader!("land_vs"), shader!("land_reflection_ps"))?,
                terrain_reflection: graphics(
                    states(true, false, SCENE_FORMAT),
                    shader!("terrain_vs"),
                    shader!("terrain_reflection_ps"),
                    false,
                )?,
                pine_reflection: reflection(shader!("pine_vs"), shader!("pine_reflection_ps"))?,
                sky_reflection: graphics(
                    states(false, false, SCENE_FORMAT),
                    shader!("output_vs"),
                    shader!("sky_reflection_ps"),
                    false,
                )?,
            })
        } else {
            None
        };
        let scene_pipelines = ScenePipelines {
            cube: scene,
            cube_shadow: shadow,
            grass,
            terrain,
            alpine,
        };
        let lighting = graphics(
            states(false, false, SCENE_FORMAT),
            shader!("output_vs"),
            shader!("deferred_ps"),
            false,
        )?;
        let clouds = graphics(
            states(false, false, SCENE_FORMAT),
            shader!("output_vs"),
            shader!("clouds_ps"),
            false,
        )?;
        Ok((
            Self {
                root,
                sdr,
                hdr,
                lighting,
                clouds,
            },
            scene_pipelines,
        ))
    }
}
