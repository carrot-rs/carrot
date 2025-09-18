mod render_pass;
mod setup;
mod types;

use super::metal_atlas::MetalAtlas;
use anyhow::Result;
#[cfg(any(test, feature = "test-support"))]
use image::RgbaImage;
use inazuma::{
    AtlasTextureId, Background, Bounds, ContentMask, DevicePixels, MonochromeSprite, PaintSurface,
    Path, Point, PolychromeSprite, PrimitiveBatch, Quad, ScaledPixels, Scene, Shadow, Size,
    Surface, Underline, point, size,
};

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_video::{
    CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBufferGetHeight,
    CVPixelBufferGetHeightOfPlane, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferGetWidthOfPlane, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, kCVReturnSuccess,
};
use objc2_foundation::NSRange;
use objc2_metal::*;
use objc2_quartz_core::*;
use parking_lot::Mutex;

use std::ptr::NonNull;
use std::{ffi::c_void, mem, ptr, sync::Arc};

#[cfg(any(test, feature = "test-support"))]
pub use render_pass::MetalHeadlessRenderer;
pub(crate) use setup::MetalRenderer;
use types::*;
pub(crate) use types::{
    Context, InstanceBuffer, InstanceBufferPool, PointF, Renderer, new_renderer,
};
pub use types::{PathRasterizationVertex, PathSprite, SurfaceBounds};
