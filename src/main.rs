use std::time::Instant;
use thiserror::Error;
use anyhow::{anyhow, Context, Result};
use log::{debug, error};
use futures::executor::block_on;
use winit::{
    event::*,
    event_loop::{EventLoop, ControlFlow},
    window::{Window, WindowBuilder},
    dpi::PhysicalSize,
};
use imgui::im_str;

#[allow(dead_code)]
struct Renderer {
    instance: wgpu::Instance,
    surface: wgpu::Surface,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    swapchain_desc: wgpu::SwapChainDescriptor,
    swapchain: wgpu::SwapChain,
    size: PhysicalSize<u32>,
    last_frame_ts: Instant,
    last_cursor: Option<Option<imgui::MouseCursor>>,
}

#[derive(Error, Debug)]
enum RenderError {
    #[error("getting a swapchain frame failed")]
    SwapChainError(#[from] wgpu::SwapChainError),

    #[error("error preparing imgui frame")]
    ImguiFramePrepError { source: winit::error::ExternalError },

    #[error("failed to render imgui frame")]
    ImguiRendererError(imgui_wgpu::RendererError),
}

impl Renderer {
    async fn new(window: &Window) -> Result<Self> {
        let size = window.inner_size();

        // PRIMARY => VK, Metal, DX12, BWebGpu
        let instance = wgpu::Instance::new(wgpu::BackendBit::PRIMARY);

        let surface = unsafe { instance.create_surface(window) };

        // adapter just identifies the device we want to talk to
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
        }).await.ok_or(anyhow!("couldn't find an adapter!"))?;

        // and the device is an open connection to it
        let (device, queue) = adapter.request_device(
            &wgpu::DeviceDescriptor {
                features: wgpu::Features::empty(),
                limits: wgpu::Limits::default(),
                shader_validation: true,
            },
            None
        ).await?;

        let swapchain_desc = wgpu::SwapChainDescriptor {
            usage: wgpu::TextureUsage::OUTPUT_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Fifo,
        };
        let swapchain = device.create_swap_chain(&surface, &swapchain_desc);

        let last_frame_ts = Instant::now();

        Ok(Renderer {
            instance,
            surface,
            adapter,
            device,
            queue,
            swapchain_desc,
            swapchain,
            size,
            last_frame_ts,
            last_cursor: None,
        })
    }

    fn resize(&mut self, new_size: PhysicalSize<u32>) {
        debug!("resizing to {:?}", new_size);
        self.size = new_size;
        self.swapchain_desc.width = new_size.width;
        self.swapchain_desc.height = new_size.height;
        self.swapchain = self.device.create_swap_chain(&self.surface, &self.swapchain_desc);
    }

    fn render(&mut self, window: &Window, imstate: &mut ImguiState) -> Result<(), RenderError> {
        let now = Instant::now();
        let delta_t = now - self.last_frame_ts;
        let frame = self.swapchain.get_current_frame()?;

        imstate.ctx.io_mut().update_delta_time(delta_t);
        self.last_frame_ts = now;

        imstate.platform.prepare_frame(imstate.ctx.io_mut(), window).map_err(|e| RenderError::ImguiFramePrepError { source: e })?;

        let ui = imstate.ctx.frame();

        {
            let window = imgui::Window::new(im_str!("Hello world"));
            window
                .size([300.0, 100.0], imgui::Condition::FirstUseEver)
                .build(&ui, || {
                    ui.text(im_str!("Hello world!"));
                    ui.text(im_str!("This...is...imgui-rs on WGPU!"));
                    ui.separator();
                    let mouse_pos = ui.io().mouse_pos;
                    ui.text(im_str!(
                        "Mouse Position: ({:.1},{:.1})",
                        mouse_pos[0],
                        mouse_pos[1]
                    ));
                });

            let window = imgui::Window::new(im_str!("Hello too"));
            window
                .size([400.0, 200.0], imgui::Condition::FirstUseEver)
                .position([400.0, 200.0], imgui::Condition::FirstUseEver)
                .build(&ui, || {
                    ui.text(im_str!("Frametime: {:?}", delta_t));
                });

            ui.show_demo_window(&mut true);
        }

        // update mouse cursor if we need to
        if self.last_cursor != Some(ui.mouse_cursor()) {
            self.last_cursor = Some(ui.mouse_cursor());
            imstate.platform.prepare_render(&ui, &window);
        }

        // used to encode series of gpu operations!
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("render encoder"),
        });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[
                    wgpu::RenderPassColorAttachmentDescriptor {
                        attachment: &frame.output.view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.1,
                                g: 0.2,
                                b: 0.3,
                                a: 1.0,
                            }),
                            store: true,
                        }
                    }
                ],
                depth_stencil_attachment: None,
            });

            imstate.renderer.render(ui.render(), &self.queue, &self.device, &mut rpass).map_err(RenderError::ImguiRendererError)
        }?;


        self.queue.submit(std::iter::once(encoder.finish()));

        Ok(())
    }
}

struct ImguiState {
    ctx: imgui::Context,
    platform: imgui_winit_support::WinitPlatform,
    renderer: imgui_wgpu::Renderer,
}

fn main() -> Result<()> {
    env_logger::init();

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .build(&event_loop)?;

    let mut renderer = block_on(Renderer::new(&window))?;

    let mut imstate = {
        let mut ctx = imgui::Context::create();
        let mut platform = imgui_winit_support::WinitPlatform::init(&mut ctx);
        platform.attach_window(ctx.io_mut(),
            &window,
            imgui_winit_support::HiDpiMode::Default);
        ctx.set_ini_filename(None);
    
        let rend_config = imgui_wgpu::RendererConfig::new().set_texture_format(renderer.swapchain_desc.format);
        let mut renderer = imgui_wgpu::Renderer::new(&mut ctx, &renderer.device, &renderer.queue, rend_config);

        ImguiState { ctx, platform, renderer }
    };


    let font_size = (13. * window.scale_factor()) as f32;
    imstate.ctx.io_mut().font_global_scale = (1.0/window.scale_factor()) as f32;

    imstate.ctx.fonts().add_font(&[imgui::FontSource::DefaultFontData {
        config: Some(imgui::FontConfig {
            oversample_h: 1,
            pixel_snap_h: true,
            size_pixels: font_size,
            ..Default::default()
        }),
    }]);

    event_loop.run(move |event, _, control_flow| {
        match event {
            Event::WindowEvent {ref event, window_id} if window_id == window.id() => match event {
                WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,

                WindowEvent::Resized(size) => renderer.resize(*size),
                WindowEvent::ScaleFactorChanged {new_inner_size, ..} => renderer.resize(**new_inner_size),

                _ => ()
            },
            Event::MainEventsCleared => window.request_redraw(),
            Event::RedrawRequested(window_id) if window_id == window.id() => {
                match renderer.render(&window, &mut imstate) {
                    Ok(_) => (),
                    Err(RenderError::SwapChainError(e)) => match e {
                        wgpu::SwapChainError::Lost => renderer.resize(renderer.size),
                        wgpu::SwapChainError::OutOfMemory => {
                            error!("swapchain error: out of memory");
                            *control_flow = ControlFlow::Exit;
                        },
                        e => error!("swapchain error (not fatal, just framedrop): {:?}", e),
                    },
                    r @ Err(_) => r.unwrap(),
                }
            },
            _ => (),
        }

        imstate.platform.handle_event(imstate.ctx.io_mut(), &window, &event);
    });
}
