use crate::gpu::{SCENE_FORMAT, SDR_FORMAT};
use windows::{
    core::{Error, Interface, Result},
    Win32::{
        Foundation::{BOOL, E_FAIL, HWND},
        Graphics::{
            Direct3D::D3D_FEATURE_LEVEL_11_0,
            Direct3D12::*,
            Dxgi::{Common::*, *},
            Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST},
        },
    },
};

pub unsafe fn supports_tearing(factory: &IDXGIFactory4) -> bool {
    let Ok(factory) = factory.cast::<IDXGIFactory5>() else {
        return false;
    };
    let mut supported = BOOL(0);
    factory
        .CheckFeatureSupport(
            DXGI_FEATURE_PRESENT_ALLOW_TEARING,
            std::ptr::from_mut(&mut supported).cast(),
            size_of::<BOOL>() as u32,
        )
        .is_ok()
        && supported.as_bool()
}

pub unsafe fn supports_mesh_shaders(device: &ID3D12Device) -> bool {
    let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS7::default();
    let mut model = D3D12_FEATURE_DATA_SHADER_MODEL {
        HighestShaderModel: D3D_SHADER_MODEL_6_5,
    };
    device
        .CheckFeatureSupport(
            D3D12_FEATURE_D3D12_OPTIONS7,
            std::ptr::from_mut(&mut options).cast(),
            std::mem::size_of_val(&options) as u32,
        )
        .is_ok()
        && options.MeshShaderTier == D3D12_MESH_SHADER_TIER_1
        && device
            .CheckFeatureSupport(
                D3D12_FEATURE_SHADER_MODEL,
                std::ptr::from_mut(&mut model).cast(),
                std::mem::size_of_val(&model) as u32,
            )
            .is_ok()
        && model.HighestShaderModel.0 >= D3D_SHADER_MODEL_6_5.0
}

unsafe fn device_for(adapter: &IDXGIAdapter1) -> Result<ID3D12Device> {
    let mut device = None;
    D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_11_0, &mut device)?;
    let device: ID3D12Device = device.unwrap();
    let mut sm = D3D12_FEATURE_DATA_SHADER_MODEL {
        HighestShaderModel: D3D_SHADER_MODEL_6_0,
    };
    device.CheckFeatureSupport(
        D3D12_FEATURE_SHADER_MODEL,
        (&mut sm as *mut D3D12_FEATURE_DATA_SHADER_MODEL).cast(),
        std::mem::size_of_val(&sm) as u32,
    )?;
    if sm.HighestShaderModel.0 < D3D_SHADER_MODEL_6_0.0 {
        return Err(Error::new(E_FAIL, "The renderer requires Shader Model 6.0"));
    }
    let description = adapter.GetDesc1()?.Description;
    let len = description
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(description.len());
    println!("Adapter: {}", String::from_utf16_lossy(&description[..len]));
    Ok(device)
}

pub unsafe fn choose_device(factory: &IDXGIFactory4, warp: bool) -> Result<ID3D12Device> {
    if !warp {
        let preferred = factory.cast::<IDXGIFactory6>().ok();
        for i in 0.. {
            let adapter: Result<IDXGIAdapter1> = match &preferred {
                Some(f) => f.EnumAdapterByGpuPreference(i, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE),
                None => factory.EnumAdapters1(i),
            };
            let adapter = match adapter {
                Ok(a) => a,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(e) => return Err(e),
            };
            if adapter.GetDesc1()?.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0 {
                if let Ok(device) = device_for(&adapter) {
                    return Ok(device);
                }
            }
        }
        eprintln!("No suitable hardware adapter. Trying WARP.");
    }
    device_for(&factory.EnumWarpAdapter()?)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Output {
    pub hdr: bool,
    pub peak_nits: f32,
}

impl Output {
    pub const SDR: Self = Self {
        hdr: false,
        peak_nits: 80.0,
    };

    pub fn format(self) -> DXGI_FORMAT {
        if self.hdr {
            SCENE_FORMAT
        } else {
            SDR_FORMAT
        }
    }

    pub unsafe fn detect(factory: &IDXGIFactory4, hwnd: HWND) -> Self {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        for a in 0.. {
            let Ok(adapter) = factory.EnumAdapters1(a) else {
                break;
            };
            for o in 0.. {
                let Ok(output) = adapter.EnumOutputs(o) else {
                    break;
                };
                let Ok(desc) = output
                    .cast::<IDXGIOutput6>()
                    .and_then(|output| output.GetDesc1())
                else {
                    continue;
                };
                if desc.Monitor == monitor {
                    if desc.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020 {
                        let peak = if desc.MaxLuminance.is_finite() && desc.MaxLuminance >= 200.0 {
                            desc.MaxLuminance
                        } else {
                            600.0
                        };
                        return Self {
                            hdr: true,
                            peak_nits: peak,
                        };
                    }
                    return Self::SDR;
                }
            }
        }
        Self::SDR
    }

    pub unsafe fn apply(self, swap: &IDXGISwapChain3) -> Result<()> {
        let space = if self.hdr {
            DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709
        } else {
            DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709
        };
        if swap.CheckColorSpaceSupport(space)?
            & DXGI_SWAP_CHAIN_COLOR_SPACE_SUPPORT_FLAG_PRESENT.0 as u32
            == 0
        {
            return Err(Error::new(
                E_FAIL,
                "The swap chain cannot present the requested color space",
            ));
        }
        swap.SetColorSpace1(space)
    }
}
