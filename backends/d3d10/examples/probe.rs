//! Asks the machine, without a window: can the installed driver make a Direct3D 10 hardware
//! device at feature level 10_0, and can a clear be read back from it?
//!
//! `cargo run -p weft_backend_d3d10 --example probe`

fn main() {
    #[cfg(windows)]
    run::report();
    #[cfg(not(windows))]
    println!("the Direct3D 10 probe only means something on Windows");
}

#[cfg(windows)]
mod run {
    use windows::core::Interface;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D10::D3D10_DRIVER_TYPE;
    use windows::Win32::Graphics::Direct3D10::{
        D3D10CreateDevice, ID3D10DepthStencilView, ID3D10Device, ID3D10Texture2D, D3D10_BIND_RENDER_TARGET, D3D10_DRIVER_TYPE_HARDWARE,
        D3D10_DRIVER_TYPE_REFERENCE, D3D10_MAP_READ, D3D10_SDK_VERSION, D3D10_TEXTURE2D_DESC, D3D10_USAGE_DEFAULT, D3D10_USAGE_STAGING,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC};
    use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};

    fn device(driver: D3D10_DRIVER_TYPE) -> Result<(ID3D10Device, String), String> {
        let mut out: Option<ID3D10Device> = None;
        let created = unsafe { D3D10CreateDevice(None, driver, HMODULE::default(), 0, D3D10_SDK_VERSION, Some(&mut out)) };
        match (created.is_ok(), out) {
            (true, Some(device)) => {
                let adapter = describe(&device);
                Ok((device, adapter))
            }
            (true, None) => Err("the call succeeded but handed back no device".into()),
            (false, _) => Err(format!("{}", created.unwrap_err())),
        }
    }

    fn describe(device: &ID3D10Device) -> String {
        let read = || -> windows::core::Result<String> {
            let dxgi: IDXGIDevice = device.cast()?;
            let adapter: IDXGIAdapter = unsafe { dxgi.GetParent()? };
            let desc = unsafe { adapter.GetDesc()? };
            let label = String::from_utf16_lossy(&desc.Description)
                .split('\0')
                .next()
                .unwrap_or("")
                .to_string();
            Ok(format!(
                "{label} VEN_{:04X}&DEV_{:04X} rev {:04X}, {} MB dedicated",
                desc.VendorId,
                desc.DeviceId,
                desc.Revision,
                desc.DedicatedVideoMemory / 1024 / 1024
            ))
        };
        read().unwrap_or_else(|e| format!("adapter unnamed: {e}"))
    }

    /// Clears an offscreen render target and copies it back to the CPU; returns the first pixel.
    fn round_trip(device: &ID3D10Device) -> Result<[u8; 4], String> {
        let desc = D3D10_TEXTURE2D_DESC {
            Width: 8,
            Height: 8,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D10_USAGE_DEFAULT,
            BindFlags: D3D10_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let target: ID3D10Texture2D = unsafe { device.CreateTexture2D(&desc, None) }.map_err(|e| format!("render target: {e}"))?;
        let mut view = None;
        unsafe { device.CreateRenderTargetView(&target, None, Some(&mut view)) }.map_err(|e| format!("render target view: {e}"))?;
        let views = [view.clone()];
        unsafe {
            device.OMSetRenderTargets(Some(&views), None::<&ID3D10DepthStencilView>);
            device.ClearRenderTargetView(view.as_ref(), &[0.53, 0.81, 0.92, 1.0]);
            device.Flush();
        }
        let staged_desc = D3D10_TEXTURE2D_DESC { Usage: D3D10_USAGE_STAGING, BindFlags: 0, CPUAccessFlags: 0x20000, ..desc };
        let staged = unsafe { device.CreateTexture2D(&staged_desc, None) }.map_err(|e| format!("staging texture: {e}"))?;
        unsafe {
            device.CopyResource(&staged, &target);
            device.Flush();
        }
        let mapped = unsafe { staged.Map(0, D3D10_MAP_READ, 0) }.map_err(|e| format!("map: {e}"))?;
        // R8G8B8A8_UNORM is stored as one byte per channel in that order.
        let bytes = mapped.pData as *const u8;
        let pixel = unsafe { [bytes.read(), bytes.add(1).read(), bytes.add(2).read(), bytes.add(3).read()] };
        unsafe { staged.Unmap(0) };
        Ok(pixel)
    }

    pub fn report() {
        for (kind, driver) in [("hardware", D3D10_DRIVER_TYPE_HARDWARE), ("reference", D3D10_DRIVER_TYPE_REFERENCE)] {
            match device(driver) {
                Ok((device, adapter)) => match round_trip(&device) {
                    Ok(pixel) => println!("{kind}: D3D10 device made at feature level 10_0 [{adapter}] first pixel rgba {pixel:?}"),
                    Err(error) => println!("{kind}: D3D10 device made [{adapter}] but readback failed: {error}"),
                },
                Err(error) => println!("{kind}: no D3D10 device: {error}"),
            }
        }
        println!("expected sky rgba is roughly [135, 207, 235, 255]");
    }
}
