mod desktop;
mod gpu;

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use eyre::{Context, Result, bail, eyre};
use freedesktop_file_parser::EntryType;
use log::{error, info, warn};
use palette::{FromColor, IntoColor, Oklab};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    output::{OutputHandler, OutputState},
    reexports::{calloop::EventLoop, calloop_wayland_source::WaylandSource},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        SeatHandler, SeatState,
        pointer::{BTN_LEFT, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
        },
    },
    shm::{Shm, ShmHandler},
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_buffer, wl_output::WlOutput, wl_pointer::WlPointer, wl_seat::WlSeat},
};

use crate::{
    desktop::DesktopEntries,
    gpu::{AppGpuState, SurfaceGpuState},
};

fn main() -> Result<()> {
    env_logger::builder()
        .filter(None, log::LevelFilter::Info)
        .init();

    let now = Instant::now();
    let desktop_files = desktop::find_desktop_files().wrap_err("loading .desktop files")?;
    info!(
        "Loaded {} desktop icons in {:?}",
        desktop_files.count(),
        now.elapsed()
    );

    let conn = Connection::connect_to_env().wrap_err("can't connect to Wayland socket")?;

    let (globals, event_queue) = registry_queue_init(&conn).wrap_err("initializing connection")?;

    let mut event_loop: EventLoop<App> = EventLoop::try_new().wrap_err("creating event loop")?;
    let qh: &QueueHandle<App> = &event_queue.handle();

    let mut app = App {
        conn: conn.clone(),
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, qh),
        compositor_state: CompositorState::bind(&globals, qh)
            .wrap_err("failed to bind wl_compositor global")?,
        layer_shell: LayerShell::bind(&globals, qh)
            .wrap_err("failed to bind zwlr_layer_shell_v1 global, does the compositor not support layer shell?")?,
        shm: Shm::bind(&globals, qh).wrap_err("failed to bind shm")?,
        seat_state: SeatState::new(&globals, qh),

        gpu: AppGpuState::new()?,

        desktop_files,
        pointers: HashMap::new(),
        layer_surfaces: Vec::new(),
    };

    WaylandSource::new(conn.clone(), event_queue)
        .insert(event_loop.handle())
        .map_err(|err| eyre!("{:?}", err))
        .wrap_err("failed to register wayland event source")?;

    let _ = sd_notify::notify(true, &[sd_notify::NotifyState::Ready]);

    loop {
        event_loop
            .dispatch(Duration::from_millis(16), &mut app)
            .wrap_err("error during event loop")?;
    }
}

struct App {
    conn: Connection,
    registry_state: RegistryState,
    output_state: OutputState,
    compositor_state: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    seat_state: SeatState,

    gpu: AppGpuState,

    desktop_files: DesktopEntries,
    pointers: HashMap<WlSeat, WlPointer>,
    layer_surfaces: Vec<OutputSurface>,
}

struct OutputSurface {
    // must be first to be dropped before the Wayland surface
    gpu: SurfaceGpuState,
    output: WlOutput,
    layer_surface: LayerSurface,
    width: u32,
    height: u32,
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
            surface.clone(),
            Layer::Background,
            Some("wallpaper"),
            Some(&output),
        );
        layer_surface.set_exclusive_zone(-1);
        layer_surface.set_anchor(Anchor::all());
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer_surface.wl_surface().commit();

        match SurfaceGpuState::new(&self.gpu, &self.conn.backend(), &surface) {
            Ok(gpu_state) => {
                self.layer_surfaces.push(OutputSurface {
                    gpu: gpu_state,
                    output,
                    layer_surface,
                    width: 0,
                    height: 0,
                });
            }
            Err(err) => error!(
                "Failed to create wgpu surface, log at prior logs for more detail {:?}",
                eyre!(err)
            ),
        }
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
        layer: &smithay_client_toolkit::shell::wlr_layer::LayerSurface,
    ) {
        if let Some(surface_idx) = self
            .layer_surfaces
            .iter()
            .position(|surface| surface.layer_surface == *layer)
        {
            self.layer_surfaces.swap_remove(surface_idx);
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &smithay_client_toolkit::shell::wlr_layer::LayerSurface,
        configure: smithay_client_toolkit::shell::wlr_layer::LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let (width, height) = configure.new_size;
        info!("Reconfiguring surface to {}x{}", width, height);

        let Some(surface) = self
            .layer_surfaces
            .iter_mut()
            .find(|surface| surface.layer_surface == *layer)
        else {
            return;
        };

        surface.width = width;
        surface.height = height;

        surface.gpu.resize(&self.gpu, width, height);
    }
}

// keep it in sync with the gpu implementation
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

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wayland_client::protocol::wl_seat::WlSeat,
    ) {
    }

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wayland_client::protocol::wl_seat::WlSeat,
        capability: smithay_client_toolkit::seat::Capability,
    ) {
        if capability == smithay_client_toolkit::seat::Capability::Pointer {
            self.pointers.insert(
                seat.clone(),
                self.seat_state.get_pointer(qh, &seat).unwrap(),
            );
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        seat: wayland_client::protocol::wl_seat::WlSeat,
        capability: smithay_client_toolkit::seat::Capability,
    ) {
        if capability == smithay_client_toolkit::seat::Capability::Pointer {
            self.pointers.remove(&seat);
        }
    }

    fn remove_seat(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wayland_client::protocol::wl_seat::WlSeat,
    ) {
    }
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wayland_client::protocol::wl_pointer::WlPointer,
        events: &[smithay_client_toolkit::seat::pointer::PointerEvent],
    ) {
        for event in events {
            if let PointerEventKind::Release {
                button: BTN_LEFT, ..
            } = event.kind
            {
                let Some(surface) = self
                    .layer_surfaces
                    .iter()
                    .find(|surface| *surface.layer_surface.wl_surface() == event.surface)
                else {
                    return;
                };

                let srgb = color_for_pixel(
                    event.position.0 as u32,
                    event.position.1 as u32,
                    surface.width,
                    surface.height,
                );

                let oklab: Oklab = srgb.into_format::<f32>().into_color();

                let best_match = self.desktop_files.find_entry(oklab);

                if let Some(best_match) = best_match
                    && let EntryType::Application(app) = &best_match.file.entry.entry_type
                    && let Some(exec) = &app.exec
                {
                    // lol terrible implementation that works well enough
                    // https://specifications.freedesktop.org/desktop-entry/latest/exec-variables.html
                    let exec = exec.replace("%U", "").replace("%F", "");
                    if exec.contains("%") {
                        warn!(
                            "Trying to execute insuffiently substituded command-line, refusing: {}",
                            exec
                        );
                        return;
                    }
                    if let Err(err) = spawn(&exec) {
                        error!("Failed to spawn program: {}: {:?}", exec, err);
                    }
                }
            }
        }
    }
}

fn spawn(cmd: &str) -> Result<()> {
    info!("Spawning program: {cmd}");
    let output = std::process::Command::new("niri")
        .arg("msg")
        .arg("action")
        .arg("spawn-sh")
        .arg("--")
        .arg(cmd)
        .output()
        .wrap_err("executing niri msg action spawn-sh")?;
    if !output.status.success() {
        bail!(
            "niri returned error: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

smithay_client_toolkit::delegate_registry!(App);
smithay_client_toolkit::delegate_output!(App);
smithay_client_toolkit::delegate_compositor!(App);
smithay_client_toolkit::delegate_layer!(App);
smithay_client_toolkit::delegate_shm!(App);
wayland_client::delegate_noop!(App: ignore wl_buffer::WlBuffer);
smithay_client_toolkit::delegate_seat!(App);
smithay_client_toolkit::delegate_pointer!(App);
