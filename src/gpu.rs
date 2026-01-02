use std::{mem::offset_of, ptr::NonNull};

use eyre::{Context, Result};
use palette::Oklab;
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use wayland_client::{Proxy, protocol::wl_surface::WlSurface};
use wgpu::util::DeviceExt;

pub struct AppGpuState {
    instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    render_pipeline: wgpu::RenderPipeline,
    screen_size_bind_group_layout: wgpu::BindGroupLayout,
    desktop_colors_bind_group: wgpu::BindGroup,
}

pub struct SurfaceGpuState {
    surface: wgpu::Surface<'static>,
    width: u32,
    height: u32,
    input_buffer: wgpu::Buffer,
    screen_size_bind_group: wgpu::BindGroup,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct InputUniform {
    size: [f32; 2], // width, height
    voronoi_progress: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DesktopColorsStorage {
    l: f32,
    a: f32,
    b: f32,
    _pad: f32,
}

impl AppGpuState {
    pub fn new(
        desktop_colors: impl IntoIterator<Item = Oklab> + ExactSizeIterator,
    ) -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());

        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .wrap_err("failed to request adapter")?;

        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))
            .wrap_err("failed to request device")?;

        let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));
        let screen_size_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        let desktop_colors_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("desktop_colors_bind_group_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Render Pipeline Layout"),
                bind_group_layouts: &[
                    &screen_size_bind_group_layout,
                    &desktop_colors_bind_group_layout,
                ],
                immediate_size: 0,
            });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                targets: &[Some(wgpu::TextureFormat::Bgra8UnormSrgb.into())],
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

        let desktop_colors = desktop_colors
            .into_iter()
            .map(|color| DesktopColorsStorage {
                l: color.l,
                a: color.a,
                b: color.b,
                _pad: 0.0,
            })
            .collect::<Vec<_>>();

        let desktop_colors_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("desktop_colors_buffer"),
            contents: bytemuck::cast_slice::<DesktopColorsStorage, u8>(&desktop_colors),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let desktop_colors_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &desktop_colors_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &desktop_colors_buffer,
                    offset: 0,
                    size: None,
                }),
            }],
            label: Some("desktop_colors_bind_group"),
        });

        Ok(Self {
            instance,
            device,
            queue,
            render_pipeline,
            screen_size_bind_group_layout,
            desktop_colors_bind_group,
        })
    }
}

impl SurfaceGpuState {
    pub fn new(
        gpu_state: &AppGpuState,
        wayland_backend: &wayland_backend::client::Backend,
        wl_surface: &WlSurface,
    ) -> Result<Self> {
        let surface = unsafe {
            gpu_state
                .instance
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

        let screen_size_buffer =
            gpu_state
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Screen Size Uniform Buffer"),
                    contents: bytemuck::bytes_of(&InputUniform {
                        size: [0.0, 0.0],
                        voronoi_progress: 0.0,
                        _pad: 0.0,
                    }),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                });

        let screen_size_bind_group =
            gpu_state
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    layout: &gpu_state.screen_size_bind_group_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &screen_size_buffer,
                            offset: 0,
                            size: None,
                        }),
                    }],
                    label: Some("screen_size_bind_group"),
                });

        Ok(Self {
            surface,
            input_buffer: screen_size_buffer,
            screen_size_bind_group,
            width: 0,
            height: 0,
        })
    }

    pub fn resize(&mut self, gpu_state: &AppGpuState, width: u32, height: u32) {
        self.width = width;
        self.height = height;

        gpu_state.queue.write_buffer(
            &self.input_buffer,
            0,
            bytemuck::bytes_of(&InputUniform {
                size: [width as f32, height as f32],
                voronoi_progress: 0.0,
                _pad: 0.0,
            }),
        );

        self.configure(gpu_state);
    }

    fn configure(&self, gpu_state: &AppGpuState) {
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            view_formats: vec![wgpu::TextureFormat::Bgra8UnormSrgb],
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            width: self.width,
            height: self.height,
            desired_maximum_frame_latency: 2,
            // Wayland is inherently a mailbox system.
            present_mode: wgpu::PresentMode::Mailbox,
        };
        self.surface.configure(&gpu_state.device, &surface_config);
    }

    pub fn set_voronoi_progress(&self, gpu_state: &AppGpuState, voronoi_progress: f32) {
        gpu_state.queue.write_buffer(
            &self.input_buffer,
            offset_of!(InputUniform, voronoi_progress) as u64,
            bytemuck::bytes_of(&voronoi_progress),
        );
    }

    pub fn draw(&self, gpu_state: &AppGpuState) {
        let surface_texture = match self.surface.get_current_texture() {
            Ok(texture) => texture,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                self.configure(gpu_state);
                self.surface.get_current_texture().unwrap()
            }
            Err(e) => panic!("failed to acquire next swapchain texture: {e}"),
        };

        let texture_view: wgpu::TextureView = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = gpu_state.device.create_command_encoder(&Default::default());
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

            render_pass.set_pipeline(&gpu_state.render_pipeline);
            render_pass.set_bind_group(0, &self.screen_size_bind_group, &[]);
            render_pass.set_bind_group(1, &gpu_state.desktop_colors_bind_group, &[]);
            render_pass.draw(0..6, 0..1);
        }

        gpu_state.queue.submit(Some(encoder.finish()));
        surface_texture.present();
    }
}
