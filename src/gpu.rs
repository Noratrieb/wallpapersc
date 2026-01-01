use std::ptr::NonNull;

use eyre::{Context, Result};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use wayland_client::{Proxy, protocol::wl_surface::WlSurface};
use wgpu::util::DeviceExt;

pub struct AppGpuState {
    wgpu_instance: wgpu::Instance,
    wgpu_adapter: wgpu::Adapter,
    wgpu_device: wgpu::Device,
    wgpu_queue: wgpu::Queue,
    wgpu_render_pipeline: wgpu::RenderPipeline,
    wgpu_screen_size_bind_group_layout: wgpu::BindGroupLayout,
}

pub struct SurfaceGpuState {
    wgpu_surface: wgpu::Surface<'static>,
    wgpu_screen_size_buffer: wgpu::Buffer,
    wgpu_screen_size_bind_group: wgpu::BindGroup,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ScreenSizeUniform {
    size: [f32; 2], // width, height
}

impl AppGpuState {
    pub fn new() -> Result<Self> {
        let wgpu_instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());

        let wgpu_adapter = pollster::block_on(
            wgpu_instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        )
        .wrap_err("failed to request adapter")?;

        let (wgpu_device, wgpu_queue) =
            pollster::block_on(wgpu_adapter.request_device(&Default::default()))
                .wrap_err("failed to request device")?;

        let shader = wgpu_device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));
        let wgpu_screen_size_bind_group_layout =
            wgpu_device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let render_pipeline_layout =
            wgpu_device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Render Pipeline Layout"),
                bind_group_layouts: &[&wgpu_screen_size_bind_group_layout],
                immediate_size: 0,
            });

        let wgpu_render_pipeline =
            wgpu_device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Render Pipeline"),
                layout: Some(&render_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8UnormSrgb,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count: 1,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview_mask: None,
                cache: None,
            });

        Ok(Self {
            wgpu_instance,
            wgpu_adapter,
            wgpu_device,
            wgpu_queue,
            wgpu_render_pipeline,
            wgpu_screen_size_bind_group_layout,
        })
    }
}

impl SurfaceGpuState {
    pub fn new(
        gpu_state: &AppGpuState,
        wayland_backend: &wayland_backend::client::Backend,
        wl_surface: &WlSurface,
    ) -> Result<Self> {
        let wgpu_surface = unsafe {
            gpu_state
                .wgpu_instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
                        NonNull::new(wayland_backend.display_ptr().cast()).unwrap(),
                    )),
                    raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(
                        NonNull::new(wl_surface.id().as_ptr().cast()).unwrap(),
                    )),
                })
        }
        .wrap_err("failed to create wgpu surface")?;

        let wgpu_screen_size_buffer =
            gpu_state
                .wgpu_device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Screen Size Uniform Buffer"),
                    contents: bytemuck::bytes_of(&ScreenSizeUniform { size: [0.0, 0.0] }),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                });

        let wgpu_screen_size_bind_group =
            gpu_state
                .wgpu_device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    layout: &gpu_state.wgpu_screen_size_bind_group_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &wgpu_screen_size_buffer,
                            offset: 0,
                            size: None,
                        }),
                    }],
                    label: Some("screen_size_bind_group"),
                });

        Ok(Self {
            wgpu_surface,
            wgpu_screen_size_buffer,
            wgpu_screen_size_bind_group,
        })
    }

    pub fn resize(&self, gpu_state: &AppGpuState, width: u32, height: u32) {
        gpu_state.wgpu_queue.write_buffer(
            &self.wgpu_screen_size_buffer,
            0,
            bytemuck::bytes_of(&ScreenSizeUniform {
                size: [width as f32, height as f32],
            }),
        );

        let cap = self.wgpu_surface.get_capabilities(&gpu_state.wgpu_adapter);
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: cap.formats[0],
            view_formats: vec![cap.formats[0]],
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            width,
            height,
            desired_maximum_frame_latency: 2,
            // Wayland is inherently a mailbox system.
            present_mode: wgpu::PresentMode::Mailbox,
        };
        self.wgpu_surface
            .configure(&gpu_state.wgpu_device, &surface_config);

        let surface_texture = self
            .wgpu_surface
            .get_current_texture()
            .expect("failed to acquire next swapchain texture");
        let texture_view: wgpu::TextureView = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = gpu_state
            .wgpu_device
            .create_command_encoder(&Default::default());
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[
                    // This is what @location(0) in the fragment shader targets
                    Some(wgpu::RenderPassColorAttachment {
                        view: &texture_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.1,
                                g: 0.5,
                                b: 0.3,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    }),
                ],
                depth_stencil_attachment: None,
                ..Default::default()
            });

            render_pass.set_pipeline(&gpu_state.wgpu_render_pipeline);
            render_pass.set_bind_group(0, Some(&self.wgpu_screen_size_bind_group), &[]);
            render_pass.draw(0..6, 0..1);
        }

        gpu_state.wgpu_queue.submit(Some(encoder.finish()));
        surface_texture.present();
    }
}
