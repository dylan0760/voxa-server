//! GPU setup for the window renderer.
//!
//! wgpu's default on Windows often ends up on Vulkan. Vulkan swap chains don't take part
//! in the Windows compositor's flip model, which makes the window stutter and trail the
//! mouse while it is dragged. DirectX 12 is the native path, so it is preferred (hardware
//! adapters first), with only one frame queued so the content follows the window.
//! `VOXA_GPU=dx12|vulkan|gl` forces a backend for troubleshooting.

use std::sync::{Arc, OnceLock};

use eframe::egui_wgpu::{self, wgpu};

static CHOSEN: OnceLock<String> = OnceLock::new();

/// "NVIDIA GeForce RTX 3080 (Dx12)" etc., once the window has been created.
#[allow(dead_code)] // shown in the client's About window
pub fn adapter_info() -> Option<&'static str> {
    CHOSEN.get().map(|s| s.as_str())
}

fn score(info: &wgpu::AdapterInfo, forced: Option<wgpu::Backend>) -> i32 {
    if let Some(b) = forced {
        if info.backend != b {
            return -1;
        }
    }
    let kind = match info.device_type {
        wgpu::DeviceType::DiscreteGpu => 3,
        wgpu::DeviceType::IntegratedGpu => 2,
        wgpu::DeviceType::VirtualGpu => 1,
        _ => 0, // software renderers (WARP, llvmpipe)
    };
    let backend = match info.backend {
        wgpu::Backend::Dx12 => 3,
        wgpu::Backend::Vulkan => 2,
        wgpu::Backend::Gl => 1,
        _ => 0,
    };
    kind * 10 + backend
}

fn pick(adapters: &[wgpu::Adapter], surface: Option<&wgpu::Surface<'_>>) -> Result<wgpu::Adapter, String> {
    let forced = match std::env::var("VOXA_GPU").unwrap_or_default().to_ascii_lowercase().as_str() {
        "dx12" | "d3d12" | "directx" => Some(wgpu::Backend::Dx12),
        "vulkan" | "vk" => Some(wgpu::Backend::Vulkan),
        "gl" | "opengl" => Some(wgpu::Backend::Gl),
        _ => None,
    };
    let usable: Vec<&wgpu::Adapter> = adapters.iter().filter(|a| surface.map(|s| a.is_surface_supported(s)).unwrap_or(true)).collect();
    let best = usable
        .iter()
        .copied()
        .filter(|a| score(&a.get_info(), forced) >= 0)
        .max_by_key(|a| score(&a.get_info(), forced))
        .or_else(|| usable.first().copied())
        .ok_or_else(|| "No graphics adapter can draw to this window".to_string())?;
    let info = best.get_info();
    let _ = CHOSEN.set(format!("{} ({:?})", info.name, info.backend));
    if cfg!(debug_assertions) {
        for a in adapters {
            let i = a.get_info();
            eprintln!("gpu: {} {:?} {:?} score {}", i.name, i.backend, i.device_type, score(&i, forced));
        }
        eprintln!("gpu: using {}", CHOSEN.get().map(String::as_str).unwrap_or("?"));
    }
    Ok(best.clone())
}

pub fn wgpu_options() -> egui_wgpu::WgpuConfiguration {
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.native_adapter_selector = Some(Arc::new(pick));
    egui_wgpu::WgpuConfiguration {
        surface: egui_wgpu::SurfaceConfig::LOW_LATENCY,
        wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}
