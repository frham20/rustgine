use crate::gpu::{buffer, resource, texture, transition, upload};
use std::mem::ManuallyDrop;
use windows::{
    core::{Error, Result},
    Win32::{
        Foundation::E_INVALIDARG,
        Graphics::{Direct3D12::*, Dxgi::Common::DXGI_FORMAT_BC1_UNORM_SRGB},
    },
};

// Slice order matches the four material groups in terrain.hlsl.
const TILES: [&[u8]; 16] = [
    include_bytes!("../../assets/terrain/meadow-sod.dds"),
    include_bytes!("../../assets/terrain/fine-fescue.dds"),
    include_bytes!("../../assets/terrain/clover-meadow.dds"),
    include_bytes!("../../assets/terrain/dry-grass.dds"),
    include_bytes!("../../assets/terrain/damp-moss.dds"),
    include_bytes!("../../assets/terrain/mossy-soil.dds"),
    include_bytes!("../../assets/terrain/leaf-litter.dds"),
    include_bytes!("../../assets/terrain/dark-loam.dds"),
    include_bytes!("../../assets/terrain/compacted-earth.dds"),
    include_bytes!("../../assets/terrain/sandy-grit.dds"),
    include_bytes!("../../assets/terrain/river-pebbles.dds"),
    include_bytes!("../../assets/terrain/angular-gravel.dds"),
    include_bytes!("../../assets/terrain/weathered-limestone.dds"),
    include_bytes!("../../assets/terrain/laminated-shale.dds"),
    include_bytes!("../../assets/terrain/lichen-bedrock.dds"),
    include_bytes!("../../assets/terrain/coarse-scree.dds"),
];
const SIZE: u32 = 512;
const MIPS: usize = SIZE.ilog2() as usize + 1;
const SUBRESOURCES: usize = TILES.len() * MIPS;
const FORMAT: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT = DXGI_FORMAT_BC1_UNORM_SRGB;

fn mip_data(tile: &[u8]) -> Result<&[u8]> {
    let invalid = || {
        Error::new(
            E_INVALIDARG,
            "Expected a 512x512 sRGB BC1 terrain DDS with a full mip chain",
        )
    };
    let header = tile.get(..148).ok_or_else(invalid)?;
    let word = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
    if &header[..4] != b"DDS "
        || word(4) != 124
        || word(12) != SIZE
        || word(16) != SIZE
        || word(28) != MIPS as u32
        || word(76) != 32
        || &header[84..88] != b"DX10"
        || word(128) != FORMAT.0 as u32
        || word(132) != 3
        || word(140) != 1
    {
        return Err(invalid());
    }
    Ok(&tile[header.len()..])
}

pub(super) unsafe fn load(
    device: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    srv: D3D12_CPU_DESCRIPTOR_HANDLE,
    staging: &mut Vec<ID3D12Resource>,
) -> Result<ID3D12Resource> {
    let mut desc = texture(SIZE, SIZE, FORMAT, D3D12_RESOURCE_FLAG_NONE);
    desc.DepthOrArraySize = TILES.len() as u16;
    desc.MipLevels = MIPS as u16;

    let mut layouts = [D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); SUBRESOURCES];
    let mut rows = [0; SUBRESOURCES];
    let mut row_bytes = [0; SUBRESOURCES];
    let mut size_bytes = 0;
    device.GetCopyableFootprints(
        &desc,
        0,
        SUBRESOURCES as u32,
        0,
        Some(layouts.as_mut_ptr()),
        Some(rows.as_mut_ptr()),
        Some(row_bytes.as_mut_ptr()),
        Some(&mut size_bytes),
    );
    let mut bytes = vec![0; size_bytes as usize];
    let wrong_size = || Error::new(E_INVALIDARG, "Unexpected terrain DDS mip data length");
    for (layer, tile) in TILES.iter().enumerate() {
        let mut data = mip_data(tile)?;
        for i in layer * MIPS..(layer + 1) * MIPS {
            let row_size = row_bytes[i] as usize;
            let (mip, rest) = data
                .split_at_checked(row_size * rows[i] as usize)
                .ok_or_else(wrong_size)?;
            data = rest;
            let offset = layouts[i].Offset as usize;
            let pitch = layouts[i].Footprint.RowPitch as usize;
            for (y, row) in mip.chunks_exact(row_size).enumerate() {
                let start = offset + y * pitch;
                bytes[start..start + row_size].copy_from_slice(row);
            }
        }
        if !data.is_empty() {
            return Err(wrong_size());
        }
    }

    let upload_buffer = buffer(
        device,
        size_bytes,
        D3D12_HEAP_TYPE_UPLOAD,
        D3D12_RESOURCE_STATE_GENERIC_READ,
    )?;
    upload(&upload_buffer, &bytes)?;
    let textures = resource(
        device,
        D3D12_HEAP_TYPE_DEFAULT,
        &desc,
        D3D12_RESOURCE_STATE_COPY_DEST,
        None,
    )?;
    for (index, layout) in layouts.into_iter().enumerate() {
        let mut from = D3D12_TEXTURE_COPY_LOCATION {
            pResource: ManuallyDrop::new(Some(upload_buffer.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: layout,
            },
        };
        let mut to = D3D12_TEXTURE_COPY_LOCATION {
            pResource: ManuallyDrop::new(Some(textures.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: index as u32,
            },
        };
        list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
        ManuallyDrop::drop(&mut from.pResource);
        ManuallyDrop::drop(&mut to.pResource);
    }
    transition(
        list,
        &textures,
        D3D12_RESOURCE_STATE_COPY_DEST,
        D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
    );
    device.CreateShaderResourceView(&textures, None, srv);
    staging.push(upload_buffer);
    Ok(textures)
}
