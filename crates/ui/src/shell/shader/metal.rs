//! Runs a translated background shader on our own Metal pipeline
//! (LLM-578). Each frame renders into a BGRA texture. From there it goes
//! out two ways:
//!
//! - As YCbCr surfaces (LLM-579): a second pass blends the frame with the
//!   flat color under the slot and writes an IOSurface-backed
//!   `CVPixelBuffer`, which GPUI's `surface()` draws. Nothing passes
//!   through the CPU. A surface is opaque and ignores element opacity, so
//!   the opacity is part of the blend.
//! - As BGRA bytes read back, for an image the app paints with opacity, a
//!   fade and a clip. The app takes this way only when what lies under
//!   the slot is not one flat color, or while it fades.
//!
//! Patterned on the offscreen render and `draw_surfaces` in GPUI's
//! `metal_renderer.rs` (Apache-2.0); no code is copied.

use std::ptr::NonNull;
use std::sync::OnceLock;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_video::{
    CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBuffer,
    CVPixelBufferGetHeightOfPlane, CVPixelBufferGetWidthOfPlane, CVPixelBufferPool,
    kCVMetalTextureUsage, kCVPixelBufferHeightKey, kCVPixelBufferIOSurfacePropertiesKey,
    kCVPixelBufferMetalCompatibilityKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, kCVReturnSuccess,
};
use objc2_foundation::NSString;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice,
    MTLFunction, MTLLibrary, MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRegion,
    MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineDescriptor,
    MTLRenderPipelineState, MTLSize, MTLStorageMode, MTLStoreAction, MTLTexture,
    MTLTextureDescriptor, MTLTextureType, MTLTextureUsage,
};

use super::translate::{Inputs, Translated, VERTEX};
use super::{Blend, Frame, Surface};

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

/// The pass that turns a frame into the two planes of a YCbCr 4:2:0
/// buffer: full-size luma, and chroma at half size each way. The matrix
/// is the inverse of the BT.601 full-range one GPUI's `surface_fragment`
/// applies, so a color comes back as it went in.
const CONVERT_MSL: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct ConvertOut { float4 position [[position]]; float2 uv; };

vertex ConvertOut llmtodo_convert_vertex(uint vid [[vertex_id]]) {
    float2 p = float2(float((vid << 1) & 2), float(vid & 2));
    ConvertOut out;
    out.position = float4(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = float2(p.x, 1.0 - p.y);
    return out;
}

// `under.rgb` is the color under the slot, `under.a` the slot's opacity.
static float3 blended(texture2d<float> frame, float2 uv, float4 under) {
    constexpr sampler s(filter::linear, address::clamp_to_edge);
    float4 c = frame.sample(s, uv);
    return mix(under.rgb, c.rgb, saturate(c.a * under.a));
}

static float luma(float3 c) { return dot(c, float3(0.299, 0.587, 0.114)); }

fragment float4 llmtodo_convert_luma(ConvertOut in [[stage_in]],
                                     texture2d<float> frame [[texture(0)]],
                                     constant float4 &under [[buffer(0)]]) {
    return float4(luma(blended(frame, in.uv, under)), 0.0, 0.0, 1.0);
}

// Chroma renders at half size, so each sample falls between four frame
// pixels and the linear filter averages them.
fragment float4 llmtodo_convert_chroma(ConvertOut in [[stage_in]],
                                       texture2d<float> frame [[texture(0)]],
                                       constant float4 &under [[buffer(0)]]) {
    float3 c = blended(frame, in.uv, under);
    float y = luma(c);
    return float4((c.b - y) / 1.772 + 0.5, (c.r - y) / 1.402 + 0.5, 0.0, 1.0);
}
"#;

struct Convert {
    luma: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    chroma: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
}

// SAFETY: render pipeline states are immutable and thread-safe.
unsafe impl Send for Convert {}
unsafe impl Sync for Convert {}

fn convert() -> Result<&'static Convert, String> {
    static CONVERT: OnceLock<Result<Convert, String>> = OnceLock::new();
    CONVERT
        .get_or_init(|| {
            let gpu = gpu()?;
            let library = gpu
                .device
                .newLibraryWithSource_options_error(&NSString::from_str(CONVERT_MSL), None)
                .map_err(|e| format!("Metal: {}", e.localizedDescription()))?;
            let vertex = function(&library, "llmtodo_convert_vertex")?;
            let state = |name: &str, format| {
                pipeline_state(gpu, &vertex, &*function(&library, name)?, format)
            };
            Ok(Convert {
                luma: state("llmtodo_convert_luma", MTLPixelFormat::R8Unorm)?,
                chroma: state("llmtodo_convert_chroma", MTLPixelFormat::RG8Unorm)?,
            })
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn function(
    library: &ProtocolObject<dyn MTLLibrary>,
    name: &str,
) -> Result<Retained<ProtocolObject<dyn MTLFunction>>, String> {
    library
        .newFunctionWithName(&NSString::from_str(name))
        .ok_or_else(|| format!("Metal: no function {name}"))
}

fn pipeline_state(
    gpu: &Gpu,
    vertex: &ProtocolObject<dyn MTLFunction>,
    fragment: &ProtocolObject<dyn MTLFunction>,
    format: MTLPixelFormat,
) -> Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>, String> {
    let descriptor = MTLRenderPipelineDescriptor::new();
    descriptor.setVertexFunction(Some(vertex));
    descriptor.setFragmentFunction(Some(fragment));
    // SAFETY: index 0 is within the color attachment array.
    let color = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    color.setPixelFormat(format);
    gpu.device
        .newRenderPipelineStateWithDescriptor_error(&descriptor)
        .map_err(|e| format!("Metal: {}", e.localizedDescription()))
}

/// A compiled shader. What its frames draw into is a [`Target`], one per
/// slot, so two slots of different sizes that run the same shader do not
/// take turns making new textures and pools (LLM-585).
pub struct Pipeline {
    state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
}

/// What one slot's frames draw into, kept from frame to frame while the
/// size stays.
#[derive(Default)]
pub struct Target {
    texture: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    /// Hands out YCbCr buffers of the texture's size. A buffer goes back
    /// to the pool once GPUI has drawn it and lets it go.
    pool: Option<CFRetained<CVPixelBufferPool>>,
    cache: Option<CFRetained<CVMetalTextureCache>>,
}

// SAFETY: a render pipeline state is immutable and thread-safe.
unsafe impl Send for Pipeline {}
unsafe impl Sync for Pipeline {}

// SAFETY: Metal textures, CoreVideo pools and texture caches are
// reference counted with atomics and may move between threads. The
// target is used by one render at a time: `render` takes it by `&mut`.
unsafe impl Send for Target {}

impl Target {
    /// The texture's size, and the pool and the texture cache as
    /// addresses, so a test can tell which of them were made again.
    #[cfg(test)]
    pub fn ids(&self) -> (Option<(u32, u32)>, Option<usize>, Option<usize>) {
        (
            self.texture
                .as_ref()
                .map(|t| (t.width() as u32, t.height() as u32)),
            self.pool.as_deref().map(|p| std::ptr::from_ref(p) as usize),
            self.cache
                .as_deref()
                .map(|c| std::ptr::from_ref(c) as usize),
        )
    }
}

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
    let state = pipeline_state(
        gpu,
        &*function(&library, VERTEX)?,
        &*function(&library, &t.fragment)?,
        MTLPixelFormat::BGRA8Unorm,
    )?;
    Ok(Pipeline { state })
}

impl Pipeline {
    /// Renders one frame of `width` × `height` px. With `pixels`, it
    /// returns the BGRA bytes, row by row; for each blend, a YCbCr
    /// surface. Blocks until the GPU is done, so call it off the main
    /// thread.
    pub fn render(
        &self,
        target: &mut Target,
        width: u32,
        height: u32,
        inputs: &Inputs,
        pixels: bool,
        blends: &[Blend],
    ) -> Result<Frame, String> {
        if !pixels && blends.is_empty() {
            return Ok(Frame {
                pixels: None,
                surfaces: Vec::new(),
            });
        }
        // A worker thread has no run loop to drain autoreleased objects,
        // such as the command buffer and the pass descriptors, so each
        // frame drains its own, as GPUI's headless Metal renderer does.
        autoreleasepool(|_| self.render_in(target, width, height, inputs, pixels, blends))
    }

    fn render_in(
        &self,
        target: &mut Target,
        width: u32,
        height: u32,
        inputs: &Inputs,
        pixels: bool,
        blends: &[Blend],
    ) -> Result<Frame, String> {
        let gpu = gpu()?;
        let (width, height) = (width.max(1), height.max(1));
        let reuse = target
            .texture
            .as_ref()
            .is_some_and(|t| t.width() == width as usize && t.height() == height as usize);
        if !reuse {
            let descriptor = MTLTextureDescriptor::new();
            descriptor.setTextureType(MTLTextureType::Type2D);
            // SAFETY: both sides are at least 1.
            unsafe {
                descriptor.setWidth(width as usize);
                descriptor.setHeight(height as usize);
            }
            descriptor.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
            descriptor.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
            // Apple GPUs share memory with the CPU, so the read back below
            // needs no blit.
            descriptor.setStorageMode(MTLStorageMode::Shared);
            target.texture = Some(
                gpu.device
                    .newTextureWithDescriptor(&descriptor)
                    .ok_or("Metal: no texture")?,
            );
            target.pool = None;
        }
        let texture = target.texture.clone().expect("just made");

        let buffer = gpu
            .queue
            .commandBuffer()
            .ok_or("Metal: no command buffer")?;
        let encoder = pass(&buffer, &texture)?;
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

        // The plane textures must live until the GPU is done with them.
        let mut planes: Vec<CFRetained<CVMetalTexture>> = Vec::new();
        let mut surfaces = Vec::new();
        if !blends.is_empty() {
            let convert = convert()?;
            for blend in blends {
                let surface = new_surface(gpu, target, width, height)?;
                let under = [blend.under[0], blend.under[1], blend.under[2], blend.amount];
                for (plane, format, state) in [
                    (0, MTLPixelFormat::R8Unorm, &convert.luma),
                    (1, MTLPixelFormat::RG8Unorm, &convert.chroma),
                ] {
                    let cache = target.cache.as_ref().expect("made with the surface");
                    let wrapper = plane_texture(cache, &surface, plane, format)?;
                    let out =
                        CVMetalTextureGetTexture(&wrapper).ok_or("Metal: no plane texture")?;
                    planes.push(wrapper);
                    let encoder = pass(&buffer, &out)?;
                    encoder.setRenderPipelineState(state);
                    // SAFETY: the frame texture lives in `target` past the
                    // wait below; `under` is four floats, as the shader
                    // reads, and Metal copies them before this returns.
                    unsafe {
                        encoder.setFragmentTexture_atIndex(Some(&texture), 0);
                        encoder.setFragmentBytes_length_atIndex(
                            NonNull::from(&under).cast(),
                            std::mem::size_of_val(&under),
                            0,
                        );
                        encoder.drawPrimitives_vertexStart_vertexCount(
                            MTLPrimitiveType::Triangle,
                            0,
                            3,
                        );
                    }
                    encoder.endEncoding();
                }
                surfaces.push((*blend, Surface(surface)));
            }
        }
        buffer.commit();
        buffer.waitUntilCompleted();
        drop(planes);
        if let Some(cache) = &target.cache {
            cache.flush(0);
        }

        let pixels = pixels.then(|| {
            let row = width as usize * 4;
            let mut bytes = vec![0u8; row * height as usize];
            let region = MTLRegion {
                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                size: MTLSize {
                    width: width as usize,
                    height: height as usize,
                    depth: 1,
                },
            };
            // SAFETY: `bytes` holds exactly `row * height` bytes, the
            // region lies inside the texture, and the GPU finished
            // writing it.
            unsafe {
                texture.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                    NonNull::from(bytes.as_mut_slice()).cast(),
                    row,
                    region,
                    0,
                );
            }
            bytes
        });
        Ok(Frame { pixels, surfaces })
    }
}

/// A render pass that draws into `texture`. Each pass draws a triangle
/// over every pixel, so the old contents need no load or clear.
fn pass(
    buffer: &ProtocolObject<dyn MTLCommandBuffer>,
    texture: &ProtocolObject<dyn MTLTexture>,
) -> Result<Retained<ProtocolObject<dyn MTLRenderCommandEncoder>>, String> {
    let pass = MTLRenderPassDescriptor::new();
    // SAFETY: index 0 is within the color attachment array.
    let attachment = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
    attachment.setTexture(Some(texture));
    attachment.setLoadAction(MTLLoadAction::DontCare);
    attachment.setStoreAction(MTLStoreAction::Store);
    buffer
        .renderCommandEncoderWithDescriptor(&pass)
        .ok_or_else(|| "Metal: no encoder".to_string())
}

/// A YCbCr buffer of `width` × `height` from the target's pool. Makes the
/// pool and the texture cache first when needed.
fn new_surface(
    gpu: &Gpu,
    target: &mut Target,
    width: u32,
    height: u32,
) -> Result<CFRetained<CVPixelBuffer>, String> {
    if target.cache.is_none() {
        let mut cache = std::ptr::null_mut();
        let usage = CFNumber::new_i64(
            (MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead).0 as i64,
        );
        // SAFETY: the key is a CFString constant; the value a CFNumber.
        let attributes = unsafe {
            CFDictionary::<CFString, CFType>::from_slices(&[kCVMetalTextureUsage], &[&*usage])
        };
        // SAFETY: the output pointer is writable, the dictionary maps
        // CFStrings to CFTypes.
        let status = unsafe {
            CVMetalTextureCache::create(
                None,
                None,
                &gpu.device,
                Some(attributes.as_opaque()),
                NonNull::from(&mut cache),
            )
        };
        if status != kCVReturnSuccess {
            return Err(format!("CoreVideo: texture cache: status {status}"));
        }
        let cache = NonNull::new(cache).ok_or("CoreVideo: no texture cache")?;
        // SAFETY: a successful create hands over this reference.
        target.cache = Some(unsafe { CFRetained::from_raw(cache) });
    }
    if target.pool.is_none() {
        let number = |n: i64| CFNumber::new_i64(n);
        let (w, h, format) = (
            number(width as i64),
            number(height as i64),
            number(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange as i64),
        );
        let iosurface = CFDictionary::<CFType, CFType>::empty();
        // SAFETY: every key is a CFString constant, every value a CFType.
        let attributes = unsafe {
            CFDictionary::<CFString, CFType>::from_slices(
                &[
                    kCVPixelBufferWidthKey,
                    kCVPixelBufferHeightKey,
                    kCVPixelBufferPixelFormatTypeKey,
                    kCVPixelBufferIOSurfacePropertiesKey,
                    kCVPixelBufferMetalCompatibilityKey,
                ],
                &[&*w, &*h, &*format, &*iosurface, CFBoolean::new(true)],
            )
        };
        let mut pool = std::ptr::null_mut();
        // SAFETY: the output pointer is writable, the dictionary maps
        // CFStrings to CFTypes.
        let status = unsafe {
            CVPixelBufferPool::create(
                None,
                None,
                Some(attributes.as_opaque()),
                NonNull::from(&mut pool),
            )
        };
        if status != kCVReturnSuccess {
            return Err(format!("CoreVideo: pixel buffer pool: status {status}"));
        }
        let pool = NonNull::new(pool).ok_or("CoreVideo: no pixel buffer pool")?;
        // SAFETY: a successful create hands over this reference.
        target.pool = Some(unsafe { CFRetained::from_raw(pool) });
    }
    let pool = target.pool.as_ref().expect("just made");
    let mut buffer = std::ptr::null_mut();
    // SAFETY: the output pointer is writable.
    let status =
        unsafe { CVPixelBufferPool::create_pixel_buffer(None, pool, NonNull::from(&mut buffer)) };
    if status != kCVReturnSuccess {
        return Err(format!("CoreVideo: pixel buffer: status {status}"));
    }
    let buffer = NonNull::new(buffer).ok_or("CoreVideo: no pixel buffer")?;
    // SAFETY: a successful create hands over this reference.
    Ok(unsafe { CFRetained::from_raw(buffer) })
}

/// A Metal texture over one plane of `surface`, the same memory.
fn plane_texture(
    cache: &CVMetalTextureCache,
    surface: &CVPixelBuffer,
    plane: usize,
    format: MTLPixelFormat,
) -> Result<CFRetained<CVMetalTexture>, String> {
    let width = CVPixelBufferGetWidthOfPlane(surface, plane);
    let height = CVPixelBufferGetHeightOfPlane(surface, plane);
    let mut texture = std::ptr::null_mut();
    // SAFETY: the buffer has two planes in these formats and sizes, and
    // the output pointer is writable.
    let status = unsafe {
        CVMetalTextureCache::create_texture_from_image(
            None,
            cache,
            surface,
            None,
            format,
            width,
            height,
            plane,
            NonNull::from(&mut texture),
        )
    };
    if status != kCVReturnSuccess {
        return Err(format!("CoreVideo: plane {plane} texture: status {status}"));
    }
    let texture = NonNull::new(texture).ok_or("CoreVideo: no plane texture")?;
    // SAFETY: a successful create hands over this reference.
    Ok(unsafe { CFRetained::from_raw(texture) })
}
