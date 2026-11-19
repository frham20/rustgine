use std::{
    mem::{size_of_val, ManuallyDrop},
    ptr,
};
use windows::{
    core::{Error, Result},
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0},
        Graphics::{Direct3D12::*, Dxgi::Common::*},
        System::Threading::{CreateEventW, WaitForSingleObject, INFINITE},
    },
};

pub const SCENE_FORMAT: DXGI_FORMAT = DXGI_FORMAT_R16G16B16A16_FLOAT;
pub const GBUFFER_FORMATS: [DXGI_FORMAT; 2] = [
    DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
    DXGI_FORMAT_R16G16B16A16_FLOAT,
];
pub const SDR_FORMAT: DXGI_FORMAT = DXGI_FORMAT_R8G8B8A8_UNORM;
pub const DEPTH_FORMAT: DXGI_FORMAT = DXGI_FORMAT_D32_FLOAT;
pub const SAMPLE: DXGI_SAMPLE_DESC = DXGI_SAMPLE_DESC {
    Count: 1,
    Quality: 0,
};

pub unsafe fn readback_u32(resource: &ID3D12Resource) -> Result<u32> {
    let mut map = ptr::null_mut();
    resource.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 4 }), Some(&mut map))?;
    let value = *map.cast::<u32>();
    resource.Unmap(0, Some(&D3D12_RANGE::default()));
    Ok(value)
}

pub struct Descriptors {
    pub heap: ID3D12DescriptorHeap,
    stride: u32,
}

impl Descriptors {
    pub unsafe fn new(
        device: &ID3D12Device,
        kind: D3D12_DESCRIPTOR_HEAP_TYPE,
        count: u32,
    ) -> Result<Self> {
        let visible = kind == D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
        Ok(Self {
            heap: device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: kind,
                NumDescriptors: count,
                Flags: if visible {
                    D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
                } else {
                    D3D12_DESCRIPTOR_HEAP_FLAG_NONE
                },
                NodeMask: 0,
            })?,
            stride: device.GetDescriptorHandleIncrementSize(kind),
        })
    }

    pub unsafe fn cpu(&self, i: usize) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        let mut h = self.heap.GetCPUDescriptorHandleForHeapStart();
        h.ptr += i * self.stride as usize;
        h
    }

    pub unsafe fn gpu(&self, i: usize) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        let mut h = self.heap.GetGPUDescriptorHandleForHeapStart();
        h.ptr += i as u64 * self.stride as u64;
        h
    }
}

pub struct Timeline {
    fence: ID3D12Fence,
    event: HANDLE,
    next: u64,
}

impl Timeline {
    pub unsafe fn new(device: &ID3D12Device) -> Result<Self> {
        Ok(Self {
            fence: device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?,
            event: CreateEventW(None, false, false, None)?,
            next: 1,
        })
    }

    pub unsafe fn signal(&mut self, queue: &ID3D12CommandQueue) -> Result<u64> {
        let value = self.next;
        queue.Signal(&self.fence, value)?;
        self.next += 1;
        Ok(value)
    }

    pub unsafe fn wait(&self, value: u64) -> Result<()> {
        if self.fence.GetCompletedValue() < value {
            self.fence.SetEventOnCompletion(value, self.event)?;
            let result = WaitForSingleObject(self.event, INFINITE);
            if result == WAIT_FAILED {
                return Err(Error::from_win32());
            }
            debug_assert_eq!(result, WAIT_OBJECT_0);
        }
        Ok(())
    }
}

impl Drop for Timeline {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.event);
        }
    }
}

pub unsafe fn resource(
    device: &ID3D12Device,
    heap: D3D12_HEAP_TYPE,
    desc: &D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
    clear: Option<&D3D12_CLEAR_VALUE>,
) -> Result<ID3D12Resource> {
    let props = D3D12_HEAP_PROPERTIES {
        Type: heap,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
        ..Default::default()
    };
    let mut res = None;
    device.CreateCommittedResource(
        &props,
        D3D12_HEAP_FLAG_NONE,
        desc,
        state,
        clear.map(ptr::from_ref),
        &mut res,
    )?;
    Ok(res.unwrap())
}

pub unsafe fn buffer(
    device: &ID3D12Device,
    bytes: u64,
    heap: D3D12_HEAP_TYPE,
    state: D3D12_RESOURCE_STATES,
) -> Result<ID3D12Resource> {
    buffer_with_flags(device, bytes, heap, state, D3D12_RESOURCE_FLAG_NONE)
}

pub unsafe fn buffer_with_flags(
    device: &ID3D12Device,
    bytes: u64,
    heap: D3D12_HEAP_TYPE,
    state: D3D12_RESOURCE_STATES,
    flags: D3D12_RESOURCE_FLAGS,
) -> Result<ID3D12Resource> {
    resource(
        device,
        heap,
        &D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: bytes,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: SAMPLE,
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: flags,
            ..Default::default()
        },
        state,
        None,
    )
}

pub unsafe fn uav_barrier(list: &ID3D12GraphicsCommandList) {
    list.ResourceBarrier(&[D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            UAV: ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER::default()),
        },
        ..Default::default()
    }]);
}

pub fn texture(
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    flags: D3D12_RESOURCE_FLAGS,
) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: width.into(),
        Height: height,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: format,
        SampleDesc: SAMPLE,
        Flags: flags,
        ..Default::default()
    }
}

pub fn depth_clear() -> D3D12_CLEAR_VALUE {
    D3D12_CLEAR_VALUE {
        Format: DEPTH_FORMAT,
        Anonymous: D3D12_CLEAR_VALUE_0 {
            DepthStencil: D3D12_DEPTH_STENCIL_VALUE {
                Depth: 1.0,
                Stencil: 0,
            },
        },
    }
}

// The caller supplies plain repr(C) data and owns a fence-safe upload allocation.
pub unsafe fn upload<T>(buffer: &ID3D12Resource, values: &[T]) -> Result<()> {
    let mut mapped = ptr::null_mut();
    let bytes = size_of_val(values);
    debug_assert!(bytes as u64 <= buffer.GetDesc().Width);
    buffer.Map(0, Some(&D3D12_RANGE::default()), Some(&mut mapped))?;
    ptr::copy_nonoverlapping(values.as_ptr().cast::<u8>(), mapped.cast(), bytes);
    buffer.Unmap(
        0,
        Some(&D3D12_RANGE {
            Begin: 0,
            End: bytes,
        }),
    );
    Ok(())
}

/// Keep the staging allocation alive until the upload commands finish.
pub unsafe fn upload_to_gpu<T>(
    device: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    values: &[T],
    final_state: D3D12_RESOURCE_STATES,
    staging_buffers: &mut Vec<ID3D12Resource>,
) -> Result<ID3D12Resource> {
    let bytes = size_of_val(values) as u64;
    let staging = buffer(
        device,
        bytes,
        D3D12_HEAP_TYPE_UPLOAD,
        D3D12_RESOURCE_STATE_GENERIC_READ,
    )?;
    upload(&staging, values)?;
    let gpu = buffer(
        device,
        bytes,
        D3D12_HEAP_TYPE_DEFAULT,
        D3D12_RESOURCE_STATE_COMMON,
    )?;
    list.CopyBufferRegion(&gpu, 0, &staging, 0, bytes);
    transition(list, &gpu, D3D12_RESOURCE_STATE_COPY_DEST, final_state);
    staging_buffers.push(staging);
    Ok(gpu)
}

pub unsafe fn transition(
    list: &ID3D12GraphicsCommandList,
    res: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) {
    let mut barrier = D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: ManuallyDrop::new(Some(res.clone())),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    };
    list.ResourceBarrier(std::slice::from_ref(&barrier));
    let transition = &mut *barrier.Anonymous.Transition;
    ManuallyDrop::drop(&mut transition.pResource);
}
