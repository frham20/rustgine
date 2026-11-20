use crate::gpu::{buffer, resource, transition, upload};
use std::mem::ManuallyDrop;
use windows::{
    core::{Error, Result},
    Win32::{Foundation::E_INVALIDARG, Graphics::Direct3D12::*},
};

// Box-filter linear data in 2×2 or 2×2×2 blocks. Keep accumulation in integers
// and round once per output texel so channels stay independent through 1×1.
pub(super) fn rgba_mips(mut data: Vec<u8>, mut side: usize, volume: bool) -> Vec<u8> {
    let depth = |side| if volume { side } else { 1 };
    assert!(side.is_power_of_two());
    assert_eq!(data.len(), side * side * depth(side) * 4);
    let mut start = 0;
    while side > 1 {
        let next = side / 2;
        let next_start = data.len();
        let planes = if volume { 2 } else { 1 };
        for z in 0..depth(next) {
            for y in 0..next {
                for x in 0..next {
                    let mut sums = [0u16; 4];
                    for dz in 0..planes {
                        for dy in 0..2 {
                            let i = start + 4 * (((2 * z + dz) * side + 2 * y + dy) * side + 2 * x);
                            for (c, sum) in sums.iter_mut().enumerate() {
                                *sum += data[i + c] as u16 + data[i + 4 + c] as u16;
                            }
                        }
                    }
                    let count = 4 * planes as u16;
                    data.extend(sums.map(|sum| ((sum + count / 2) / count) as u8));
                }
            }
        }
        start = next_start;
        side = next;
    }
    data
}

// Each layer contains tightly packed mips, largest first. A volume has one
// layer with all depth slices of each mip stored consecutively.
pub(super) unsafe fn upload_mips(
    device: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    srv: D3D12_CPU_DESCRIPTOR_HANDLE,
    staging: &mut Vec<ID3D12Resource>,
    desc: &D3D12_RESOURCE_DESC,
    layers: &[&[u8]],
) -> Result<ID3D12Resource> {
    let mips = desc.MipLevels as usize;
    let subresources = layers.len() * mips;
    let mut layouts = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); subresources];
    let mut rows = vec![0; subresources];
    let mut row_bytes = vec![0; subresources];
    let mut size_bytes = 0;
    device.GetCopyableFootprints(
        desc,
        0,
        subresources as u32,
        0,
        Some(layouts.as_mut_ptr()),
        Some(rows.as_mut_ptr()),
        Some(row_bytes.as_mut_ptr()),
        Some(&mut size_bytes),
    );
    let mut bytes = vec![0; size_bytes as usize];
    let wrong_size = || Error::new(E_INVALIDARG, "Unexpected texture mip data length");
    for (layer, mut data) in layers.iter().copied().enumerate() {
        for i in layer * mips..(layer + 1) * mips {
            let row_size = row_bytes[i] as usize;
            let (mip, rest) = data
                .split_at_checked(row_size * rows[i] as usize * layouts[i].Footprint.Depth as usize)
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
        desc,
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
        D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE | D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
    );
    device.CreateShaderResourceView(&textures, None, srv);
    staging.push(upload_buffer);
    Ok(textures)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_mip_chains_average_every_channel_and_depth_slice() {
        for volume in [false, true] {
            let depth = if volume { 4 } else { 1 };
            let mut data = Vec::new();
            for z in 0..depth {
                for y in 0..4 {
                    for x in 0..4 {
                        data.extend([x * 8, y * 10, z * 12, 255]);
                    }
                }
            }
            let mips = rgba_mips(data, 4, volume);
            let len = if volume {
                (64 + 8 + 1) * 4
            } else {
                (16 + 4 + 1) * 4
            };
            assert_eq!(mips.len(), len);
            assert_eq!(mips[len - 4..], [12, 15, if volume { 18 } else { 0 }, 255]);
        }
    }
}
