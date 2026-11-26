//! Runs a translated background shader on our own Metal pipeline
//! (LLM-578). Each frame renders into a shared BGRA texture and is read
//! back, so the app paints it as an image: with opacity, a fade and a
//! clip, like any background image. GPUI's `surface()` takes only opaque
//! YCbCr frames, so it could not honor opacity.
//!
//! Patterned on the offscreen render in GPUI's `metal_renderer.rs`
//! (Apache-2.0); no code is copied.

use std::ptr::NonNull;
use std::sync::{Mutex, OnceLock};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue,
    MTLCreateSystemDefaultDevice, MTLDevice, MTLLibrary, MTLLoadAction, MTLOrigin, MTLPixelFormat,
    MTLPrimitiveType, MTLRegion, MTLRenderCommandEncoder, MTLRenderPassDescriptor,
    MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLSize, MTLStorageMode, MTLStoreAction,
    MTLTexture, MTLTextureDescriptor, MTLTextureType, MTLTextureUsage,
};

use super::translate::{Inputs, Translated, VERTEX};

struct Gpu {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
}

// SAFETY: Metal devices and command queues are safe to use from any
// thread; Apple documents both as thread-safe.
unsafe impl Send for Gpu {}
unsafe impl Sync for Gpu {}

fn gpu() -> Result<&'static Gpu, String> {
    static GPU: OnceLock<Option<Gpu>> = OnceLock::new();
    GPU.get_or_init(|| {
        let device = MTLCreateSystemDefaultDevice()?;
        let queue = device.newCommandQueue()?;
        Some(Gpu { device, queue })
    })
    .as_ref()
    .ok_or_else(|| "no Metal device".to_string())
}

/// A compiled shader, with the texture its last frame drew into.
pub struct Pipeline {
    state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    target: Mutex<Option<Retained<ProtocolObject<dyn MTLTexture>>>>,
}

// SAFETY: a render pipeline state is immutable and thread-safe. The
// texture is only touched under the mutex.
unsafe impl Send for Pipeline {}
unsafe impl Sync for Pipeline {}

impl std::fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pipeline")
    }
}

pub fn compile(t: &Translated) -> Result<Pipeline, String> {
    let gpu = gpu()?;
    let library = gpu
        .device
        .newLibraryWithSource_options_error(&NSString::from_str(&t.msl), None)
        .map_err(|e| format!("Metal: {}", e.localizedDescription()))?;
    let function = |name: &str| {
        library
            .newFunctionWithName(&NSString::from_str(name))
            .ok_or_else(|| format!("Metal: no function {name}"))
    };
    let vertex = function(VERTEX)?;
    let fragment = function(&t.fragment)?;
    let descriptor = MTLRenderPipelineDescriptor::new();
    descriptor.setVertexFunction(Some(&vertex));
    descriptor.setFragmentFunction(Some(&fragment));
    // SAFETY: index 0 is within the color attachment array.
    let color = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    color.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
    let state = gpu
        .device
        .newRenderPipelineStateWithDescriptor_error(&descriptor)
        .map_err(|e| format!("Metal: {}", e.localizedDescription()))?;
    Ok(Pipeline {
        state,
        target: Mutex::new(None),
    })
}

impl Pipeline {
    /// Renders one frame of `width` × `height` px and returns its BGRA
    /// bytes, row by row. Blocks until the GPU is done, so call it off
    /// the main thread.
    pub fn render(&self, width: u32, height: u32, inputs: &Inputs) -> Result<Vec<u8>, String> {
        let gpu = gpu()?;
        let mut target = self.target.lock().map_err(|_| "poisoned".to_string())?;
        let reuse = target
            .as_ref()
            .is_some_and(|t| t.width() == width as usize && t.height() == height as usize);
        if !reuse {
            let descriptor = MTLTextureDescriptor::new();
            descriptor.setTextureType(MTLTextureType::Type2D);
            // SAFETY: both sides are at least 1.
            unsafe {
                descriptor.setWidth(width.max(1) as usize);
                descriptor.setHeight(height.max(1) as usize);
            }
            descriptor.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
            descriptor.setUsage(MTLTextureUsage::RenderTarget);
            // Apple GPUs share memory with the CPU, so the read back below
            // needs no blit.
            descriptor.setStorageMode(MTLStorageMode::Shared);
            *target = Some(
                gpu.device
                    .newTextureWithDescriptor(&descriptor)
                    .ok_or("Metal: no texture")?,
            );
        }
        let texture = target.as_ref().expect("just made");

        let pass = MTLRenderPassDescriptor::new();
        // SAFETY: index 0 is within the color attachment array.
        let attachment = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        attachment.setTexture(Some(texture));
        attachment.setLoadAction(MTLLoadAction::Clear);
        attachment.setClearColor(MTLClearColor {
            red: 0.,
            green: 0.,
            blue: 0.,
            alpha: 0.,
        });
        attachment.setStoreAction(MTLStoreAction::Store);

        let buffer = gpu
            .queue
            .commandBuffer()
            .ok_or("Metal: no command buffer")?;
        let encoder = buffer
            .renderCommandEncoderWithDescriptor(&pass)
            .ok_or("Metal: no encoder")?;
        encoder.setRenderPipelineState(&self.state);
        // SAFETY: `inputs` is `repr(C)` and matches the shader's struct;
        // Metal copies the bytes before this call returns.
        unsafe {
            encoder.setFragmentBytes_length_atIndex(
                NonNull::from(inputs).cast(),
                std::mem::size_of::<Inputs>(),
                0,
            );
            encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3);
        }
        encoder.endEncoding();
        buffer.commit();
        buffer.waitUntilCompleted();

        let row = width as usize * 4;
        let mut pixels = vec![0u8; row * height as usize];
        let region = MTLRegion {
            origin: MTLOrigin { x: 0, y: 0, z: 0 },
            size: MTLSize {
                width: width as usize,
                height: height as usize,
                depth: 1,
            },
        };
        // SAFETY: `pixels` holds exactly `row * height` bytes, the region
        // lies inside the texture, and the GPU finished writing it.
        unsafe {
            texture.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                NonNull::from(pixels.as_mut_slice()).cast(),
                row,
                region,
                0,
            );
        }
        Ok(pixels)
    }
}
