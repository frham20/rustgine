use super::sampled_texture::{rgba_mips, upload_mips};
use crate::gpu::texture;
use windows::{
    core::Result,
    Win32::Graphics::{Direct3D12::*, Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM},
};

pub(super) unsafe fn load(
    device: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    srv: D3D12_CPU_DESCRIPTOR_HANDLE,
    staging: &mut Vec<ID3D12Resource>,
) -> Result<ID3D12Resource> {
    const SIDE: u32 = 64;
    const DATA: &[u8; 64 * 64 * 64 * 4] = include_bytes!("../../assets/cloud-noise.rgba8");
    let mut desc = texture(
        SIDE,
        SIDE,
        DXGI_FORMAT_R8G8B8A8_UNORM,
        D3D12_RESOURCE_FLAG_NONE,
    );
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE3D;
    desc.DepthOrArraySize = SIDE as u16;
    desc.MipLevels = (SIDE.ilog2() + 1) as u16;
    let data = rgba_mips(DATA.to_vec(), SIDE as usize, true);
    upload_mips(device, list, srv, staging, &desc, &[&data])
}
