use crate::gpu::{buffer, resource, texture, transition, upload};
use std::mem::ManuallyDrop;
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
    let noise = resource(
        device,
        D3D12_HEAP_TYPE_DEFAULT,
        &desc,
        D3D12_RESOURCE_STATE_COPY_DEST,
        None,
    )?;
    let source = buffer(
        device,
        DATA.len() as u64,
        D3D12_HEAP_TYPE_UPLOAD,
        D3D12_RESOURCE_STATE_GENERIC_READ,
    )?;
    upload(&source, DATA)?;
    // Each 64-texel RGBA row already meets D3D12's 256-byte pitch alignment.
    let mut layout = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    device.GetCopyableFootprints(&desc, 0, 1, 0, Some(&mut layout), None, None, None);
    let mut from = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(source.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: layout,
        },
    };
    let mut to = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(noise.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
    ManuallyDrop::drop(&mut from.pResource);
    ManuallyDrop::drop(&mut to.pResource);
    transition(
        list,
        &noise,
        D3D12_RESOURCE_STATE_COPY_DEST,
        D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
    );
    device.CreateShaderResourceView(&noise, None, srv);
    staging.push(source);
    Ok(noise)
}
