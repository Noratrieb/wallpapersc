use std::time::Duration;

use eyre::{Context, Result};
use log::{info, warn};
use palette::FromColor;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    output::{OutputHandler, OutputState},
    reexports::{calloop::EventLoop, calloop_wayland_source::WaylandSource},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
        },
    },
    shm::{Shm, ShmHandler, raw::RawPool},
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_buffer, wl_output::WlOutput, wl_shm},
};

fn main() -> Result<()> {
    env_logger::builder()
        .filter(None, log::LevelFilter::Info)
        .init();
    let conn = Connection::connect_to_env().wrap_err("can't connect to Wayland socket")?;

    let (globals, event_queue) = registry_queue_init(&conn).wrap_err("initializing connection")?;

    let mut event_loop: EventLoop<App> = EventLoop::try_new().wrap_err("creating event loop")?;
    let qh: &QueueHandle<App> = &event_queue.handle();

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, qh),
        compositor_state: CompositorState::bind(&globals, qh)
            .wrap_err("failed to bind wl_compositor global")?,
        layer_shell: LayerShell::bind(&globals, qh)
            .wrap_err("failed to bind zwlr_layer_shell_v1 global, does the compositor not support layer shell?")?,
        shm: Shm::bind(&globals, qh).wrap_err("failed to bind shm")?,

        layer_surfaces: Vec::new(),
    };

    WaylandSource::new(conn.clone(), event_queue)
        .insert(event_loop.handle())
        .wrap_err("failed to register wayland event source")?;

    loop {
        event_loop
            .dispatch(Duration::from_millis(16), &mut app)
            .wrap_err("error during event loop")?;
    }
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor_state: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,

    layer_surfaces: Vec<OutputSurface>,
}

struct OutputSurface {
    output: WlOutput,
    _layer_surface: LayerSurface,
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState,];
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        output: wayland_client::protocol::wl_output::WlOutput,
    ) {
        match self.output_state.info(&output) {
            None => warn!("New output connected, unknown information"),
            Some(info) => {
                info!(
                    "New output connected ({})",
                    info.description.unwrap_or_else(|| "<unknown>".into()),
                );
            }
        }
        let surface: wayland_client::protocol::wl_surface::WlSurface =
            self.compositor_state.create_surface(qh);
        let layer_surface = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Background,
            Some("wallpaper"),
            Some(&output),
        );
        layer_surface.set_anchor(Anchor::all());
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer_surface.wl_surface().commit();
        self.layer_surfaces.push(OutputSurface {
            output,
            _layer_surface: layer_surface,
        });
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wayland_client::protocol::wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wayland_client::protocol::wl_output::WlOutput,
    ) {
        match self.output_state.info(&output) {
            None => warn!("Output disconnected, unknown information"),
            Some(info) => {
                info!(
                    "Output disconnected ({})",
                    info.description.unwrap_or_else(|| "<unknown>".into()),
                );
            }
        }
        if let Some(suface_idx) = self
            .layer_surfaces
            .iter()
            .position(|surface| surface.output == output)
        {
            self.layer_surfaces.swap_remove(suface_idx);
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wayland_client::protocol::wl_surface::WlSurface,
        _new_factor: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wayland_client::protocol::wl_surface::WlSurface,
        _new_transform: wayland_client::protocol::wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wayland_client::protocol::wl_surface::WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wayland_client::protocol::wl_surface::WlSurface,
        _output: &wayland_client::protocol::wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wayland_client::protocol::wl_surface::WlSurface,
        _output: &wayland_client::protocol::wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for App {
    fn closed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &smithay_client_toolkit::shell::wlr_layer::LayerSurface,
    ) {
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        layer: &smithay_client_toolkit::shell::wlr_layer::LayerSurface,
        configure: smithay_client_toolkit::shell::wlr_layer::LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let (width, height) = configure.new_size;
        info!("Reconfiguring surface to {}x{}", width, height);
        let mut pool = RawPool::new(width as usize * height as usize * 4, &self.shm).unwrap();
        let canvas = pool.mmap();
        canvas
            .chunks_exact_mut(4)
            .enumerate()
            .for_each(|(index, chunk)| {
                let x = (index % width as usize) as u32;
                let y = (index / width as usize) as u32;

                let srgb = color_for_pixel(x, y, width, height);

                let a = 0xFF;
                let r = srgb.red as u32;
                let g = srgb.green as u32;
                let b = srgb.blue as u32;
                let color = (a << 24) + (r << 16) + (g << 8) + b;

                let array: &mut [u8; 4] = chunk.try_into().unwrap();
                *array = color.to_le_bytes();
            });

        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            width as i32 * 4,
            wl_shm::Format::Argb8888,
            (),
            qh,
        );

        layer.wl_surface().attach(Some(&buffer), 0, 0);
        layer.wl_surface().commit();

        buffer.destroy();
    }
}

fn color_for_pixel(x: u32, y: u32, width: u32, height: u32) -> palette::Srgb<u8> {
    let xf = x as f32 / width as f32;
    let yf = y as f32 / height as f32;

    palette::Srgb::from_color(palette::Oklab {
        l: 0.7,
        a: xf * 0.8 - 0.4,
        b: yf * 0.8 - 0.4,
    })
    .into_format::<u8>()
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

smithay_client_toolkit::delegate_registry!(App);
smithay_client_toolkit::delegate_output!(App);
smithay_client_toolkit::delegate_compositor!(App);
smithay_client_toolkit::delegate_layer!(App);
smithay_client_toolkit::delegate_shm!(App);
wayland_client::delegate_noop!(App: ignore wl_buffer::WlBuffer);
