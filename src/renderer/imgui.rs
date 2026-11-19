use crate::{
    gpu::{buffer, resource, texture, transition, upload, SAMPLE, SCENE_FORMAT, SDR_FORMAT},
    pipeline::{serialize_signature, shader},
};
use imgui::{BackendFlags, DrawCmd, DrawData, DrawIdx, DrawVert, FontAtlas, TextureId};
use std::{mem::ManuallyDrop, ptr};
use windows::{
    core::{s, Result},
    Win32::{
        Foundation::RECT,
        Graphics::{Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Direct3D12::*, Dxgi::Common::*},
    },
};

const FONT_TEXTURE: TextureId = TextureId::new(1);
const CONSTANT_COUNT: u32 = 5;

pub(super) struct Overlay {
    root: ID3D12RootSignature,
    sdr: ID3D12PipelineState,
    hdr: ID3D12PipelineState,
    _font: ID3D12Resource,
    font_srv: D3D12_GPU_DESCRIPTOR_HANDLE,
}

impl Overlay {
    pub(super) unsafe fn new(
        device: &ID3D12Device,
        list: &ID3D12GraphicsCommandList,
        fonts: &mut FontAtlas,
        font_cpu: D3D12_CPU_DESCRIPTOR_HANDLE,
        font_srv: D3D12_GPU_DESCRIPTOR_HANDLE,
        staging: &mut Vec<ID3D12Resource>,
    ) -> Result<Self> {
        let texture_range = D3D12_DESCRIPTOR_RANGE {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
            NumDescriptors: 1,
            ..Default::default()
        };
        let params = [
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    Constants: D3D12_ROOT_CONSTANTS {
                        Num32BitValues: CONSTANT_COUNT,
                        ..Default::default()
                    },
                },
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                        NumDescriptorRanges: 1,
                        pDescriptorRanges: &texture_range,
                    },
                },
            },
        ];
        let sampler = D3D12_STATIC_SAMPLER_DESC {
            Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            MaxAnisotropy: 1,
            ComparisonFunc: D3D12_COMPARISON_FUNC_ALWAYS,
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
            ..Default::default()
        };
        let root = serialize_signature(
            device,
            &D3D12_ROOT_SIGNATURE_DESC {
                NumParameters: params.len() as u32,
                pParameters: params.as_ptr(),
                NumStaticSamplers: 1,
                pStaticSamplers: &sampler,
                Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
            },
        )?;
        let sdr = create_pipeline(device, &root, SDR_FORMAT)?;
        let hdr = create_pipeline(device, &root, SCENE_FORMAT)?;
        let font = upload_font(device, list, fonts, staging)?;
        device.CreateShaderResourceView(&font, None, font_cpu);
        Ok(Self {
            root,
            sdr,
            hdr,
            _font: font,
            font_srv,
        })
    }

    pub(super) unsafe fn draw(
        &self,
        device: &ID3D12Device,
        list: &ID3D12GraphicsCommandList,
        mesh: &mut Option<ID3D12Resource>,
        data: &DrawData,
        hdr: bool,
    ) -> Result<()> {
        if data.total_idx_count == 0 || data.total_vtx_count == 0 {
            return Ok(());
        }
        let vertex_bytes = data.total_vtx_count as usize * size_of::<DrawVert>();
        let index_bytes = data.total_idx_count as usize * size_of::<DrawIdx>();
        let bytes = (vertex_bytes + index_bytes) as u64;
        if mesh
            .as_ref()
            .is_none_or(|buffer| buffer.GetDesc().Width < bytes)
        {
            *mesh = Some(buffer(
                device,
                bytes.next_power_of_two(),
                D3D12_HEAP_TYPE_UPLOAD,
                D3D12_RESOURCE_STATE_GENERIC_READ,
            )?);
        }
        // This frame slot's fence has completed, so the CPU can write its upload buffer.
        let mesh = mesh.as_ref().unwrap();
        let mut mapped = ptr::null_mut();
        mesh.Map(0, Some(&D3D12_RANGE::default()), Some(&mut mapped))?;
        let mut vertices = mapped.cast::<DrawVert>();
        let mut indices = mapped.cast::<u8>().add(vertex_bytes).cast::<DrawIdx>();
        for draw_list in data.draw_lists() {
            let vtx = draw_list.vtx_buffer();
            let idx = draw_list.idx_buffer();
            ptr::copy_nonoverlapping(vtx.as_ptr(), vertices, vtx.len());
            ptr::copy_nonoverlapping(idx.as_ptr(), indices, idx.len());
            vertices = vertices.add(vtx.len());
            indices = indices.add(idx.len());
        }
        mesh.Unmap(
            0,
            Some(&D3D12_RANGE {
                Begin: 0,
                End: bytes as usize,
            }),
        );

        let [x, y] = data.display_pos;
        let [width, height] = data.display_size;
        let [dpi_x, dpi_y] = data.framebuffer_scale;
        let viewport = [width * dpi_x, height * dpi_y];
        let constants = [
            2.0 / width,
            -2.0 / height,
            -1.0 - x * 2.0 / width,
            1.0 + y * 2.0 / height,
            f32::from(hdr),
        ];
        list.SetGraphicsRootSignature(&self.root);
        list.SetPipelineState(if hdr { &self.hdr } else { &self.sdr });
        list.SetGraphicsRoot32BitConstants(0, CONSTANT_COUNT, constants.as_ptr().cast(), 0);
        list.SetGraphicsRootDescriptorTable(1, self.font_srv);
        list.RSSetViewports(&[D3D12_VIEWPORT {
            Width: viewport[0],
            Height: viewport[1],
            MaxDepth: 1.0,
            ..Default::default()
        }]);
        list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        let address = mesh.GetGPUVirtualAddress();
        list.IASetVertexBuffers(
            0,
            Some(&[D3D12_VERTEX_BUFFER_VIEW {
                BufferLocation: address,
                SizeInBytes: vertex_bytes as u32,
                StrideInBytes: size_of::<DrawVert>() as u32,
            }]),
        );
        list.IASetIndexBuffer(Some(&D3D12_INDEX_BUFFER_VIEW {
            BufferLocation: address + vertex_bytes as u64,
            SizeInBytes: index_bytes as u32,
            Format: if size_of::<DrawIdx>() == 2 {
                DXGI_FORMAT_R16_UINT
            } else {
                DXGI_FORMAT_R32_UINT
            },
        }));
        let mut base_vertex = 0;
        let mut base_index = 0;
        for draw_list in data.draw_lists() {
            for cmd in draw_list.commands() {
                // The settings UI uses only atlas-backed widgets and submits no callbacks.
                let DrawCmd::Elements { count, cmd_params } = cmd else {
                    continue;
                };
                debug_assert_eq!(cmd_params.texture_id, FONT_TEXTURE);
                let [left, top, right, bottom] = cmd_params.clip_rect;
                let clip = RECT {
                    left: ((left - x) * dpi_x).clamp(0.0, viewport[0]) as i32,
                    top: ((top - y) * dpi_y).clamp(0.0, viewport[1]) as i32,
                    right: ((right - x) * dpi_x).clamp(0.0, viewport[0]) as i32,
                    bottom: ((bottom - y) * dpi_y).clamp(0.0, viewport[1]) as i32,
                };
                if clip.right > clip.left && clip.bottom > clip.top {
                    list.RSSetScissorRects(&[clip]);
                    list.DrawIndexedInstanced(
                        count as u32,
                        1,
                        base_index + cmd_params.idx_offset as u32,
                        base_vertex + cmd_params.vtx_offset as i32,
                        0,
                    );
                }
            }
            base_vertex += draw_list.vtx_buffer().len() as i32;
            base_index += draw_list.idx_buffer().len() as u32;
        }
        Ok(())
    }
}

pub(super) fn configure_backend(context: &mut imgui::Context) {
    context.io_mut().backend_flags |= BackendFlags::RENDERER_HAS_VTX_OFFSET;
}

unsafe fn create_pipeline(
    device: &ID3D12Device,
    root: &ID3D12RootSignature,
    format: DXGI_FORMAT,
) -> Result<ID3D12PipelineState> {
    let layout = [
        (
            s!("POSITION"),
            DXGI_FORMAT_R32G32_FLOAT,
            std::mem::offset_of!(DrawVert, pos),
        ),
        (
            s!("TEXCOORD"),
            DXGI_FORMAT_R32G32_FLOAT,
            std::mem::offset_of!(DrawVert, uv),
        ),
        (
            s!("COLOR"),
            DXGI_FORMAT_R8G8B8A8_UNORM,
            std::mem::offset_of!(DrawVert, col),
        ),
    ]
    .map(|(name, format, offset)| D3D12_INPUT_ELEMENT_DESC {
        SemanticName: name,
        Format: format,
        AlignedByteOffset: offset as u32,
        ..Default::default()
    });
    let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
        pRootSignature: ManuallyDrop::new(Some(root.clone())),
        VS: shader!("imgui_vs"),
        PS: shader!("imgui_ps"),
        InputLayout: D3D12_INPUT_LAYOUT_DESC {
            pInputElementDescs: layout.as_ptr(),
            NumElements: layout.len() as u32,
        },
        BlendState: D3D12_BLEND_DESC {
            RenderTarget: [D3D12_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D12_BLEND_SRC_ALPHA,
                DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D12_BLEND_OP_ADD,
                SrcBlendAlpha: D3D12_BLEND_ONE,
                DestBlendAlpha: D3D12_BLEND_INV_SRC_ALPHA,
                BlendOpAlpha: D3D12_BLEND_OP_ADD,
                LogicOp: D3D12_LOGIC_OP_NOOP,
                RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
                ..Default::default()
            }; 8],
            ..Default::default()
        },
        RasterizerState: D3D12_RASTERIZER_DESC {
            FillMode: D3D12_FILL_MODE_SOLID,
            CullMode: D3D12_CULL_MODE_NONE,
            DepthClipEnable: true.into(),
            ..Default::default()
        },
        SampleMask: u32::MAX,
        PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
        NumRenderTargets: 1,
        SampleDesc: SAMPLE,
        ..Default::default()
    };
    desc.RTVFormats[0] = format;
    let result = device.CreateGraphicsPipelineState(&desc);
    ManuallyDrop::drop(&mut desc.pRootSignature);
    result
}

unsafe fn upload_font(
    device: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    fonts: &mut FontAtlas,
    staging: &mut Vec<ID3D12Resource>,
) -> Result<ID3D12Resource> {
    let atlas = fonts.build_alpha8_texture();
    let format = DXGI_FORMAT_R8_UNORM;
    let desc = texture(atlas.width, atlas.height, format, D3D12_RESOURCE_FLAG_NONE);
    let pitch = atlas
        .width
        .next_multiple_of(D3D12_TEXTURE_DATA_PITCH_ALIGNMENT);
    let mut bytes = vec![0; (pitch * atlas.height) as usize];
    for (from, to) in atlas
        .data
        .chunks_exact(atlas.width as usize)
        .zip(bytes.chunks_exact_mut(pitch as usize))
    {
        to[..from.len()].copy_from_slice(from);
    }
    let upload_buffer = buffer(
        device,
        bytes.len() as u64,
        D3D12_HEAP_TYPE_UPLOAD,
        D3D12_RESOURCE_STATE_GENERIC_READ,
    )?;
    upload(&upload_buffer, &bytes)?;
    let font = resource(
        device,
        D3D12_HEAP_TYPE_DEFAULT,
        &desc,
        D3D12_RESOURCE_STATE_COPY_DEST,
        None,
    )?;
    let mut from = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(upload_buffer.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                Offset: 0,
                Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                    Format: format,
                    Width: atlas.width,
                    Height: atlas.height,
                    Depth: 1,
                    RowPitch: pitch,
                },
            },
        },
    };
    let mut to = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(font.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        ..Default::default()
    };
    list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
    ManuallyDrop::drop(&mut from.pResource);
    ManuallyDrop::drop(&mut to.pResource);
    transition(
        list,
        &font,
        D3D12_RESOURCE_STATE_COPY_DEST,
        D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
    );
    staging.push(upload_buffer);
    fonts.tex_id = FONT_TEXTURE;
    fonts.clear_tex_data();
    Ok(font)
}
