use crate::gpu::buffer;
use std::{
    fs::File,
    io::{BufWriter, Write},
    mem::ManuallyDrop,
    path::Path,
    ptr, slice,
};
use windows::{core::Result, Win32::Graphics::Direct3D12::*};

pub struct SceneTimer {
    heap: ID3D12QueryHeap,
    readback: ID3D12Resource,
}

#[derive(Clone, Copy)]
pub enum GpuMark {
    Start,
    TerrainReady,
    GrassReady,
    ShadowsReady,
    ReflectionReady,
    SurfacesReady,
    GeometryReady,
    CloudsReady,
    SceneReady,
    OutputReady,
}

impl SceneTimer {
    const COUNT: u32 = GpuMark::OutputReady as u32 + 1;
    const BYTES: usize = Self::COUNT as usize * size_of::<u64>();

    pub unsafe fn new(device: &ID3D12Device) -> Result<Self> {
        let mut heap = None;
        device.CreateQueryHeap(
            &D3D12_QUERY_HEAP_DESC {
                Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP,
                Count: Self::COUNT,
                NodeMask: 0,
            },
            &mut heap,
        )?;
        Ok(Self {
            heap: heap.unwrap(),
            readback: buffer(
                device,
                Self::BYTES as u64,
                D3D12_HEAP_TYPE_READBACK,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )?,
        })
    }

    pub unsafe fn mark(&self, list: &ID3D12GraphicsCommandList, mark: GpuMark) {
        list.EndQuery(&self.heap, D3D12_QUERY_TYPE_TIMESTAMP, mark as u32);
    }

    pub unsafe fn finish(&self, list: &ID3D12GraphicsCommandList) {
        self.mark(list, GpuMark::OutputReady);
        list.ResolveQueryData(
            &self.heap,
            D3D12_QUERY_TYPE_TIMESTAMP,
            0,
            Self::COUNT,
            &self.readback,
            0,
        );
    }

    // Read after waiting for the capture fence.
    pub unsafe fn pass_milliseconds(&self, ticks_per_second: u64) -> Result<[f64; 9]> {
        let mut mapped = ptr::null_mut();
        self.readback.Map(
            0,
            Some(&D3D12_RANGE {
                Begin: 0,
                End: Self::BYTES,
            }),
            Some(&mut mapped),
        )?;
        let ticks = mapped.cast::<[u64; Self::COUNT as usize]>().read();
        self.readback.Unmap(0, Some(&D3D12_RANGE::default()));
        let scale = 1000.0 / ticks_per_second as f64;
        Ok(std::array::from_fn(|i| {
            ticks[i + 1].saturating_sub(ticks[i]) as f64 * scale
        }))
    }
}

// Each readback owns its footprint. D3D12 may pad a texture row to 256 bytes.
pub struct Readback {
    buffer: ID3D12Resource,
    layout: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    bytes: usize,
}

impl Readback {
    pub unsafe fn record(
        device: &ID3D12Device,
        list: &ID3D12GraphicsCommandList,
        src: &ID3D12Resource,
    ) -> Result<Self> {
        let mut layout = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut bytes = 0;
        device.GetCopyableFootprints(
            &src.GetDesc(),
            0,
            1,
            0,
            Some(&mut layout),
            None,
            None,
            Some(&mut bytes),
        );
        let buffer = buffer(
            device,
            bytes,
            D3D12_HEAP_TYPE_READBACK,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let mut from = D3D12_TEXTURE_COPY_LOCATION {
            pResource: ManuallyDrop::new(Some(src.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: 0,
            },
        };
        let mut to = D3D12_TEXTURE_COPY_LOCATION {
            pResource: ManuallyDrop::new(Some(buffer.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: layout,
            },
        };
        list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
        ManuallyDrop::drop(&mut to.pResource);
        ManuallyDrop::drop(&mut from.pResource);
        Ok(Self {
            buffer,
            layout,
            bytes: bytes as usize,
        })
    }

    unsafe fn with_rows<T>(&self, f: impl FnOnce(slice::Chunks<'_, u8>) -> T) -> Result<T> {
        let mut ptr = ptr::null_mut();
        self.buffer.Map(
            0,
            Some(&D3D12_RANGE {
                Begin: 0,
                End: self.bytes,
            }),
            Some(&mut ptr),
        )?;
        let data = slice::from_raw_parts(ptr.cast::<u8>(), self.bytes);
        let output =
            f(data[self.layout.Offset as usize..].chunks(self.layout.Footprint.RowPitch as usize));
        self.buffer.Unmap(0, Some(&D3D12_RANGE::default()));
        Ok(output)
    }

    pub unsafe fn write_ppm(&self, path: &Path) -> crate::AppResult<()> {
        let size = self.layout.Footprint;
        let mut file = BufWriter::new(File::create(path)?);
        write!(file, "P6\n{} {}\n255\n", size.Width, size.Height)?;
        self.with_rows(|rows| -> std::io::Result<()> {
            for row in rows {
                for pixel in row[..size.Width as usize * 4].as_chunks::<4>().0 {
                    file.write_all(&pixel[..3])?;
                }
            }
            file.flush()
        })??;
        Ok(())
    }

    pub unsafe fn scene_peak(&self) -> Result<(f32, usize)> {
        self.with_rows(|rows| {
            let mut peak = 0u16;
            let mut above_one = 0;
            for row in rows {
                for pixel in row[..self.layout.Footprint.Width as usize * 8]
                    .as_chunks::<8>()
                    .0
                {
                    let mut high = 0;
                    for &ch in pixel[..6].as_chunks::<2>().0 {
                        let bits = u16::from_le_bytes(ch);
                        if bits < 0x7c00 {
                            high = high.max(bits);
                        }
                    }
                    peak = peak.max(high);
                    above_one += (high > 0x3c00) as usize;
                }
            }
            let exponent = (peak >> 10) as i32;
            let mantissa = (peak & 1023) as f32 / 1024.0;
            let value = if exponent == 0 {
                mantissa * 2f32.powi(-14)
            } else {
                (1.0 + mantissa) * 2f32.powi(exponent - 15)
            };
            (value, above_one)
        })
    }
}
