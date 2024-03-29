use crate::core::{Color, Size};
use crate::graphics::backend;
use crate::graphics::color;
use crate::graphics::{Antialiasing, Target};
use crate::primitive::pipeline;
use crate::primitive::{self, Primitive};
use crate::quad;
use crate::text;
use crate::triangle;
use crate::window;
use crate::Layer;

#[cfg(feature = "tracing")]
use tracing::info_span;

#[cfg(any(feature = "image", feature = "svg"))]
use crate::image;

use std::borrow::Cow;

/// A [`wgpu`] graphics backend for [`iced`].
///
/// [`wgpu`]: https://github.com/gfx-rs/wgpu-rs
/// [`iced`]: https://github.com/iced-rs/iced
#[allow(missing_debug_implementations)]
pub struct Backend {
    quad_pipeline: quad::Pipeline,
    text_pipeline: text::Pipeline,
    triangle_pipeline: triangle::Pipeline,
    pipeline_storage: pipeline::Storage,
    #[cfg(any(feature = "image", feature = "svg"))]
    image_pipeline: image::Pipeline,
    staging_belt: wgpu::util::StagingBelt,
}

impl Backend {
    /// Creates a new [`Backend`].
    pub fn new(
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        let text_pipeline = text::Pipeline::new(device, queue, format);
        let quad_pipeline = quad::Pipeline::new(device, format);
        let triangle_pipeline = triangle::Pipeline::new(adapter, format);

        #[cfg(any(feature = "image", feature = "svg"))]
        let image_pipeline = {
            let backend = adapter.get_info().backend;

            image::Pipeline::new(device, format, backend)
        };

        Self {
            quad_pipeline,
            text_pipeline,
            triangle_pipeline,
            pipeline_storage: pipeline::Storage::default(),

            #[cfg(any(feature = "image", feature = "svg"))]
            image_pipeline,

            // TODO: Resize belt smartly (?)
            // It would be great if the `StagingBelt` API exposed methods
            // for introspection to detect when a resize may be worth it.
            staging_belt: wgpu::util::StagingBelt::new(1024 * 100),
        }
    }

    /// Draws the provided primitives in the given `TextureView`.
    ///
    /// The text provided as overlay will be rendered on top of the primitives.
    /// This is useful for rendering debug information.
    pub fn present<T: AsRef<str>>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        clear_color: Option<Color>,
        format: wgpu::TextureFormat,
        frame: &wgpu::TextureView,
        antialiasing: Antialiasing,
        target: &Target,
        primitives: &[Primitive],
        overlay_text: &[T],
    ) {
        log::debug!("Drawing");
        #[cfg(feature = "tracing")]
        let _ = info_span!("Wgpu::Backend", "PRESENT").entered();

        let mut layers = Layer::generate(primitives, &target.viewport);

        if !overlay_text.is_empty() {
            layers.push(Layer::overlay(overlay_text, &target.viewport));
        }

        self.prepare(
            device,
            queue,
            format,
            encoder,
            antialiasing,
            target,
            &layers,
        );

        self.staging_belt.finish();

        self.render(device, encoder, frame, clear_color, target, &layers);

        self.quad_pipeline.end_frame();
        self.text_pipeline.end_frame();
        self.triangle_pipeline.end_frame();

        #[cfg(any(feature = "image", feature = "svg"))]
        self.image_pipeline.end_frame();
    }

    /// Recalls staging memory for future uploads.
    ///
    /// This method should be called after the command encoder
    /// has been submitted.
    pub fn recall(&mut self) {
        self.staging_belt.recall();
    }

    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        encoder: &mut wgpu::CommandEncoder,
        antialiasing: Antialiasing,
        target: &Target,
        layers: &[Layer<'_>],
    ) {
        let target_size = target.viewport.physical_size();
        let scale_factor = target.viewport.scale_factor() as f32;
        let projection = target.viewport.projection();
        let _scaled_projection = target.viewport.scaled_projection();

        for layer in layers {
            let bounds = (layer.bounds * scale_factor).snap();

            if bounds.width < 1 || bounds.height < 1 {
                continue;
            }

            if !layer.quads.is_empty() {
                self.quad_pipeline.prepare(
                    device,
                    encoder,
                    &mut self.staging_belt,
                    &layer.quads,
                    projection,
                    scale_factor,
                );
            }

            if !layer.meshes.is_empty() {
                self.triangle_pipeline.prepare(
                    device,
                    encoder,
                    &mut self.staging_belt,
                    antialiasing,
                    target,
                    &layer.meshes,
                );
            }

            #[cfg(any(feature = "image", feature = "svg"))]
            {
                if !layer.images.is_empty() {
                    self.image_pipeline.prepare(
                        device,
                        encoder,
                        &mut self.staging_belt,
                        &layer.images,
                        _scaled_projection,
                        scale_factor,
                    );
                }
            }

            if !layer.text.is_empty() {
                self.text_pipeline.prepare(
                    device,
                    queue,
                    encoder,
                    &layer.text,
                    layer.bounds,
                    scale_factor,
                    target_size,
                );
            }

            if !layer.pipelines.is_empty() {
                for pipeline in &layer.pipelines {
                    pipeline.primitive.prepare(
                        format,
                        device,
                        queue,
                        pipeline.bounds,
                        target_size,
                        scale_factor,
                        &mut self.pipeline_storage,
                    );
                }
            }
        }
    }

    fn render(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wgpu::TextureView,
        clear_color: Option<Color>,
        target: &Target,
        layers: &[Layer<'_>],
    ) {
        use std::mem::ManuallyDrop;

        let mut quad_layer = 0;
        let mut triangle_layer = 0;
        #[cfg(any(feature = "image", feature = "svg"))]
        let mut image_layer = 0;
        let mut text_layer = 0;

        let mut render_pass = ManuallyDrop::new(encoder.begin_render_pass(
            &wgpu::RenderPassDescriptor {
                label: Some("iced_wgpu render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: frame,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: match clear_color {
                            Some(background_color) => wgpu::LoadOp::Clear({
                                let [r, g, b, a] =
                                    color::pack(background_color).components();

                                wgpu::Color {
                                    r: f64::from(r),
                                    g: f64::from(g),
                                    b: f64::from(b),
                                    a: f64::from(a),
                                }
                            }),
                            None => wgpu::LoadOp::Load,
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            },
        ));

        let target_size = target.viewport.physical_size();
        let scale_factor = target.viewport.scale_factor() as f32;

        for layer in layers {
            let bounds = (layer.bounds * scale_factor).snap();

            if bounds.width < 1 || bounds.height < 1 {
                continue;
            }

            if !layer.quads.is_empty() {
                self.quad_pipeline.render(
                    quad_layer,
                    bounds,
                    &layer.quads,
                    &mut render_pass,
                );

                quad_layer += 1;
            }

            if !layer.meshes.is_empty() {
                let _ = ManuallyDrop::into_inner(render_pass);

                self.triangle_pipeline.render(
                    device,
                    encoder,
                    frame,
                    target,
                    triangle_layer,
                    &layer.meshes,
                );

                triangle_layer += 1;

                render_pass = ManuallyDrop::new(encoder.begin_render_pass(
                    &wgpu::RenderPassDescriptor {
                        label: Some("iced_wgpu render pass"),
                        color_attachments: &[Some(
                            wgpu::RenderPassColorAttachment {
                                view: frame,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Load,
                                    store: wgpu::StoreOp::Store,
                                },
                            },
                        )],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    },
                ));
            }

            #[cfg(any(feature = "image", feature = "svg"))]
            {
                if !layer.images.is_empty() {
                    self.image_pipeline.render(
                        image_layer,
                        bounds,
                        &mut render_pass,
                    );

                    image_layer += 1;
                }
            }

            if !layer.text.is_empty() {
                self.text_pipeline
                    .render(text_layer, bounds, &mut render_pass);

                text_layer += 1;
            }

            if !layer.pipelines.is_empty() {
                let _ = ManuallyDrop::into_inner(render_pass);

                for pipeline in &layer.pipelines {
                    let viewport = (pipeline.viewport * scale_factor).snap();

                    if viewport.width < 1 || viewport.height < 1 {
                        continue;
                    }

                    pipeline.primitive.render(
                        &self.pipeline_storage,
                        frame,
                        target_size,
                        viewport,
                        encoder,
                    );
                }

                render_pass = ManuallyDrop::new(encoder.begin_render_pass(
                    &wgpu::RenderPassDescriptor {
                        label: Some("iced_wgpu render pass"),
                        color_attachments: &[Some(
                            wgpu::RenderPassColorAttachment {
                                view: frame,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Load,
                                    store: wgpu::StoreOp::Store,
                                },
                            },
                        )],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    },
                ));
            }
        }

        let _ = ManuallyDrop::into_inner(render_pass);
    }
}

impl backend::Backend for Backend {
    type Primitive = primitive::Custom;
    type Compositor = window::Compositor;
}

impl backend::Text for Backend {
    fn load_font(&mut self, font: Cow<'static, [u8]>) {
        self.text_pipeline.load_font(font);
    }
}

#[cfg(feature = "image")]
impl backend::Image for Backend {
    fn dimensions(&self, handle: &crate::core::image::Handle) -> Size<u32> {
        self.image_pipeline.dimensions(handle)
    }
}

#[cfg(feature = "svg")]
impl backend::Svg for Backend {
    fn viewport_dimensions(
        &self,
        handle: &crate::core::svg::Handle,
    ) -> Size<u32> {
        self.image_pipeline.viewport_dimensions(handle)
    }
}

#[cfg(feature = "geometry")]
impl crate::graphics::geometry::Backend for Backend {
    type Frame = crate::geometry::Frame;

    fn new_frame(&self, size: Size) -> Self::Frame {
        crate::geometry::Frame::new(size)
    }
}
