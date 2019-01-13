use winit::{
    EventsLoop,
    WindowBuilder,
    Event,
    WindowEvent,
    VirtualKeyCode,
    KeyboardInput,
};
use gfx_hal::{
    Graphics,
    Instance,
    window::Surface,
    device::Device,
    Primitive,
    FrameSync,
    SwapImageIndex,
    Swapchain,
    SwapchainConfig,
    Backbuffer,
    queue::Submission,
    pool::CommandPoolCreateFlags,
    pass::{
        Attachment,
        AttachmentLoadOp,
        AttachmentStoreOp,
        AttachmentOps,
        Subpass, SubpassDesc, SubpassDependency, SubpassRef,
    },
    pso::{
        PipelineStage,
        EntryPoint,
        ColorBlendDesc,
        GraphicsPipelineDesc,
        GraphicsShaderSet,
        ColorMask,
        BlendState,
        Rasterizer,
        Rect,
        Viewport,
    },
    image::{
        Layout,
        Access,
        SubresourceRange,
        ViewKind,
    },
    format::{
        AsFormat,
        Format,
        ChannelType,
        Aspects,
        Swizzle,
        Rgba8Srgb as ColorFormat
    },
    command::{
        ClearColor,
        ClearValue,
    }
};
#[cfg(feature = "gl")]
use gfx_backend_gl as backend;
#[cfg(feature = "vulkan")]
use gfx_backend_vulkan as backend;
use std::error::Error;

static EXTENT: std::sync::Once = std::sync::Once::new();

fn main() -> Result<(), Box<dyn Error>> {

    let mut events_loop = EventsLoop::new();

    let window = WindowBuilder::new()
        .with_title("NiceGFX")
        .with_max_dimensions((256, 256).into())
        .with_decorations(false)
        .with_transparency(true)
        .with_always_on_top(true);

    #[cfg(not(feature = "gl"))]
    let (window, instance, mut adapter, mut surface) = {
        let _window = window.build(&events_loop).unwrap();
        let instance = backend::Instance::create("NiceGFX", 1);
        let surface = instance.create_surface(&_window);
        let adapter = instance.enumerate_adapters().remove(0);
        (_window, instance, adapter, surface)
    };
    #[cfg(feature = "gl")]
    let (mut adapter, mut surface) = {
        let window = {
            let builder =
                backend::config_context(backend::glutin::ContextBuilder::new(), ColorFormat::SELF, None)
                    .with_vsync(true);
            backend::glutin::GlWindow::new(window, builder, &events_loop).unwrap()
        };

        let surface = backend::Surface::from_window(window);
        let adapter = surface.enumerate_adapters().remove(0);
        (adapter, surface)
    };

/*
    let instance = backend::Instance::create("NiceGFX", 1);
    let mut surface = instance.create_surface(&window);
    let mut adapter = instance.enumerate_adapters().remove(0);
*/

    let num_queues = 1;
    let (device, mut queue_group) = adapter
        .open_with::<_, Graphics>(num_queues, |family| surface.supports_queue_family(family))
        .unwrap();

    let max_buffers = 16;
    let mut command_pool = device.create_command_pool_typed(
        &queue_group,
        CommandPoolCreateFlags::empty(),
        max_buffers,
    );

    let physical_device = &adapter.physical_device;

    let (caps, formats, _) = surface.compatibility(physical_device);

    let surface_color_format = {
        match formats {
            Some(choices) => choices
                .into_iter()
                .find(|format| format.base_format().1 == ChannelType::Srgb)
                .unwrap(),
            None => Format::Rgba8Srgb,
        }
    };


    let render_pass = {
        let color_attachment = Attachment {
            format: Some(surface_color_format),
            samples: 1,
            ops: AttachmentOps::new(AttachmentLoadOp::Clear, AttachmentStoreOp::Store),
            stencil_ops: AttachmentOps::DONT_CARE,
            layouts: Layout::Undefined..Layout::Present,
        };

        let subpass = SubpassDesc {
            colors: &[(0, Layout::ColorAttachmentOptimal)],
            depth_stencil: None,
            inputs: &[],
            resolves: &[],
            preserves: &[],
        };

        let dependency = SubpassDependency {
            passes: SubpassRef::External..SubpassRef::Pass(0),
            stages: PipelineStage::COLOR_ATTACHMENT_OUTPUT..PipelineStage::COLOR_ATTACHMENT_OUTPUT,
            accesses: Access::empty()..(Access::COLOR_ATTACHMENT_READ | Access::COLOR_ATTACHMENT_WRITE),
        };

        device.create_render_pass(&[color_attachment], &[subpass], &[dependency])
    };

    let pipeline_layout = device.create_pipeline_layout(&[], &[]);

    let vertex_shader_module = {
        let spirv = include_bytes!("../assets/shaders/simple.vert.spv");
        device.create_shader_module(spirv).unwrap()
    };

    let fragment_shader_module = {
        let spirv = include_bytes!("../assets/shaders/simple.frag.spv");
        device.create_shader_module(spirv).unwrap()
    };

    let pipeline = {
        let vs_entry = EntryPoint::<backend::Backend> {
            entry: "main",
            module: &vertex_shader_module,
            specialization: Default::default(),
        };
        let fs_entry = EntryPoint::<backend::Backend> {
            entry: "main",
            module: &fragment_shader_module,
            specialization: Default::default(),
        };

        let shader_entries = GraphicsShaderSet {
            vertex: vs_entry,
            hull: None,
            domain: None,
            geometry: None,
            fragment: Some(fs_entry),
        };

        let subpass = Subpass {
            index: 0,
            main_pass: &render_pass,
        };

        let mut pipeline_desc = GraphicsPipelineDesc::new(
            shader_entries,
            Primitive::TriangleList,
            Rasterizer::FILL,
            &pipeline_layout,
            subpass,
        );

        pipeline_desc
            .blender
            .targets
            .push(ColorBlendDesc(ColorMask::ALL, BlendState::ALPHA));

        device
            .create_graphics_pipeline(&pipeline_desc, None)
            .unwrap()
    };

    //////////


    let frame_semaphore = device.create_semaphore();
    let present_semaphore = device.create_semaphore();

    let mut swapchain_stuff: Option<(_, _, _, _)> = None;

    let mut rebuild_swapchain = false;

    // Main loop
    loop {
        let mut quitting = false;

        events_loop.poll_events(|event| {
            if let Event::WindowEvent { event, ..} = event {
                match event {
                    WindowEvent::CloseRequested => quitting = true,
                    WindowEvent::KeyboardInput {
                        input:
                            KeyboardInput {
                                virtual_keycode: Some(VirtualKeyCode::Escape),
                                ..
                            },
                        ..
                    } => quitting = true,
                    WindowEvent::Resized(_) => {
                        rebuild_swapchain = true;
                    },
                    WindowEvent::HiDpiFactorChanged(_) => {
                        rebuild_swapchain = true;
                    },
                    WindowEvent::Refresh => {
                        rebuild_swapchain = true;
                    },
                    _ => {}   
                }
            }
        });
        
        if quitting {
            break;
        }

        if (rebuild_swapchain || quitting) && swapchain_stuff.is_some() {
            let (swapchain, _extent, frame_views, framebuffers) = swapchain_stuff.take().unwrap();

            match device.wait_idle() {
                Ok(_) => {},
                Err(err) => { println!("Error: {:?}", err); continue; }
            };
            command_pool.reset();

            for framebuffer in framebuffers {
                device.destroy_framebuffer(framebuffer);
            }

            for image_view in frame_views {
                device.destroy_image_view(image_view);
            }

            device.destroy_swapchain(swapchain);
        }

        if swapchain_stuff.is_none() {
            rebuild_swapchain = false;

            let (caps, _, _) = surface.compatibility(physical_device);

            let swap_config = SwapchainConfig::from_caps(&caps, surface_color_format);
        
            let extent = swap_config.extent.to_extent();
        
            let (swapchain, backbuffer) = device.create_swapchain(&mut surface, swap_config, None);
        
        
            let (frame_views, framebuffers) = match backbuffer {
                Backbuffer::Images(images) => {
                    let color_range = SubresourceRange {
                        aspects: Aspects::COLOR,
                        levels: 0..1,
                        layers: 0..1,
                    };
        
                    let image_views = images
                        .iter()
                        .map(|image| {
                            device
                                .create_image_view(
                                    image,
                                    ViewKind::D2,
                                    surface_color_format,
                                    Swizzle::NO,
                                    color_range.clone(),
                                ).unwrap()
                        }).collect::<Vec<_>>();
        
                    let fbos = image_views
                        .iter()
                        .map(|image_view| {
                            device
                                .create_framebuffer(&render_pass, vec![image_view], extent)
                                .unwrap()
                        }).collect();
                    
                    (image_views, fbos)           
                }
        
                Backbuffer::Framebuffer(fbo) => (vec![], vec![fbo]),
            };

            swapchain_stuff = Some((swapchain, extent, frame_views, framebuffers))
        }

        let (swapchain, extent, _frame_views, framebuffers) = swapchain_stuff.as_mut().unwrap();

        EXTENT.call_once(|| {
            println!("{:?}", &extent);
        });

        // Rendering

        command_pool.reset();

        let frame_index: SwapImageIndex = 
            match swapchain.acquire_image(FrameSync::Semaphore(&frame_semaphore)) {
                Ok(i) => i,
                Err(_) => {
                    rebuild_swapchain = true;
                    continue;
                }
            };
        
        let finished_command_buffer = {
            let mut command_buffer = command_pool.acquire_command_buffer(false);

            //Viewport - in this case the whole screen
            let viewport = Viewport {
                rect: Rect {
                    x: 0,
                    y: 0,
                    w: extent.width as i16,
                    h: extent.height as i16,
                },
                depth: 0.0..1.0,
            };

            command_buffer.set_viewports(0, &[viewport.clone()]);
            command_buffer.set_scissors(0, &[viewport.rect]);

            command_buffer.bind_graphics_pipeline(&pipeline);

            {
                let mut encoder = command_buffer.begin_render_pass_inline(
                    &render_pass,
                    &framebuffers[frame_index as usize],
                    viewport.rect,
                    &[ClearValue::Color(ClearColor::Float([0.0, 0.0, 0.0, 0.0]))],
                );

                // Draw time
                // 0..3 - range of vertices
                // 0..1 - range of instances - irrelevant unless instanced rendering is used
                encoder.draw(0..3, 0..1);
            }

            command_buffer.finish()
        };

        let submission = Submission::new()
            .wait_on(&[(&frame_semaphore, PipelineStage::BOTTOM_OF_PIPE)])
            .signal(&[&present_semaphore])
            .submit(vec![finished_command_buffer]);
        
        queue_group.queues[0].submit(submission, None);

        let presented = swapchain.present(
                &mut queue_group.queues[0],
                frame_index,
                vec![&present_semaphore],
            );
        
        if presented.is_err() {
            rebuild_swapchain = true;
        }

    }

    //Cleanup

    device.destroy_graphics_pipeline(pipeline);
    device.destroy_pipeline_layout(pipeline_layout);

    device.destroy_render_pass(render_pass);

    device.destroy_shader_module(vertex_shader_module);
    device.destroy_shader_module(fragment_shader_module);
    device.destroy_command_pool(command_pool.into_raw());

    device.destroy_semaphore(frame_semaphore);
    device.destroy_semaphore(present_semaphore);


    Ok(())
}