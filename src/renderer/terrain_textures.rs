use crate::gpu::texture;
use windows::{
    core::{Error, Result},
    Win32::{
        Foundation::E_INVALIDARG,
        Graphics::{
            Direct3D12::*,
            Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_BC1_UNORM_SRGB, DXGI_FORMAT_R8G8_SNORM},
        },
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
const SLOPES: [&[u8]; 3] = [
    include_bytes!("../../assets/terrain/weathered-limestone-slopes.dds"),
    include_bytes!("../../assets/terrain/laminated-shale-slopes.dds"),
    include_bytes!("../../assets/terrain/coarse-scree-slopes.dds"),
];
const SIZE: u32 = 512;
const MIPS: usize = SIZE.ilog2() as usize + 1;
pub(super) const ALBEDO: (&[&[u8]], DXGI_FORMAT) = (&TILES, DXGI_FORMAT_BC1_UNORM_SRGB);
pub(super) const DETAIL_SLOPES: (&[&[u8]], DXGI_FORMAT) = (&SLOPES, DXGI_FORMAT_R8G8_SNORM);
fn mip_data(tile: &[u8], format: DXGI_FORMAT) -> Result<&[u8]> {
    let invalid = || {
        Error::new(
            E_INVALIDARG,
            "Unexpected terrain DDS dimensions, format or mip chain",
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
        || word(128) != format.0 as u32
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
    (tiles, format): (&[&[u8]], DXGI_FORMAT),
) -> Result<ID3D12Resource> {
    let mut desc = texture(SIZE, SIZE, format, D3D12_RESOURCE_FLAG_NONE);
    desc.DepthOrArraySize = tiles.len() as u16;
    desc.MipLevels = MIPS as u16;

    let layers = tiles
        .iter()
        .map(|tile| mip_data(tile, format))
        .collect::<Result<Vec<_>>>()?;
    super::sampled_texture::upload_mips(device, list, srv, staging, &desc, &layers)
}
