use gfx_hal::{adapter::PhysicalDevice,
              command::{ClearColor, ClearValue, CommandBuffer, MultiShot, Primary},
              device::Device,
              format::{AsFormat, Aspects, ChannelType, Format, Rgba8Srgb as ColorFormat, Swizzle},
              image::{Access, Extent, Kind, Layout, SubresourceRange, Tiling, Usage,
                      ViewCapabilities, ViewKind},
              pass::{Attachment, AttachmentLoadOp, AttachmentOps, AttachmentStoreOp, Subpass,
                     SubpassDependency, SubpassDesc, SubpassRef},
              pool::CommandPoolCreateFlags,
              pso::{BlendState, ColorBlendDesc, ColorMask, EntryPoint, GraphicsPipelineDesc,
                    GraphicsShaderSet, PipelineStage, Rasterizer, Rect, Viewport},
              queue::family::QueueFamily,
              queue::Submission,
              window::Surface,
              window::{Extent2D, PresentMode},
              Adapter,
              Backbuffer,
              Backend,
              CommandPool,
              Features,
              FrameSync,
              Gpu,
              Graphics,
              Instance,
              Primitive,
              QueueGroup,
              SwapImageIndex,
              Swapchain,
              SwapchainConfig};

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};
use std::error::Error;

use winit::Window;

use arrayvec::ArrayVec;

use std::mem::ManuallyDrop;
use std::ptr::read;

#[cfg(feature = "dx12")]
use gfx_backend_dx12 as back;
#[cfg(feature = "metal")]
use gfx_backend_metal as back;
#[cfg(feature = "vulkan")]
use gfx_backend_vulkan as back;

const WINDOW_NAME: &str = "NiceGfx Window";

pub struct HalState {
    current_frame: usize,
    frames_in_flight: usize,
    in_flight_fences: Vec<<back::Backend as Backend>::Fence>,
    render_finished_semaphores: Vec<<back::Backend as Backend>::Semaphore>,
    image_available_semaphores: Vec<<back::Backend as Backend>::Semaphore>,
    command_buffers: Vec<CommandBuffer<back::Backend, Graphics, MultiShot, Primary>>,
    command_pool: ManuallyDrop<CommandPool<back::Backend, Graphics>>,
    framebuffers: Vec<<back::Backend as Backend>::Framebuffer>,
    image_views: Vec<(<back::Backend as Backend>::ImageView)>,
    render_pass: ManuallyDrop<<back::Backend as Backend>::RenderPass>,
    pub render_area: Rect,
    queue_group: QueueGroup<back::Backend, Graphics>,
    swapchain: ManuallyDrop<<back::Backend as Backend>::Swapchain>,
    device: ManuallyDrop<back::Device>,
    _adapter: Adapter<back::Backend>,
    _surface: <back::Backend as Backend>::Surface,
    _instance: ManuallyDrop<back::Instance>,
}

impl HalState {
    pub fn new(window: &Window) -> Result<Self, &'static str> {
        let instance = back::Instance::create(WINDOW_NAME, 1);
        let mut surface = instance.create_surface(window);
        let adapter = instance
            .enumerate_adapters()
            .into_iter()
            .find(|a| {
                a.queue_families
                    .iter()
                    .any(|qf| qf.supports_graphics() && surface.supports_queue_family(qf))
            })
            .ok_or("Couldn't find a graphical adapter")?;

        let (device, queue_group) = {
            let queue_family = adapter
                .queue_families
                .iter()
                .find(|qf| qf.supports_graphics() && surface.supports_queue_family(qf))
                .ok_or("Couldn't find a QueueFamily with graphics")?;

            let Gpu { device, mut queues } = unsafe {
                adapter
                    .physical_device
                    .open(&[(&queue_family, &[1.0; 1])], Features::empty())
                    .map_err(|_| "Couldn't open the PhysicalDevice")?
            };
            let queue_group = queues
                .take::<Graphics>(queue_family.id())
                .ok_or("Couldn't take ownership of the QueueGroup")?;
            let _ = if queue_group.queues.len() > 0 {
                Ok(())
            } else {
                Err("The QueueGroup did not have any CommandQueues available")
            }?;
            (device, queue_group)
        };

        let (swapchain, extent, backbuffer, format, frames_in_flight) = {
            let (caps, preferred_formats, present_modes, composite_alphas) =
                surface.compatibility(&adapter.physical_device);
            warn!("{:?}", caps);
            info!("Preferred Formats: {:?}", preferred_formats);
            info!("Present Modes: {:?}", present_modes);
            info!("Composite Alphas: {:?}", composite_alphas);
            //
            let present_mode = {
                use gfx_hal::window::PresentMode::*;
                [Mailbox, Fifo, Relaxed, Immediate]
                    .iter()
                    .cloned()
                    .find(|pm| present_modes.contains(pm))
                    .ok_or("No PresentMode values specified!")?
            };
            let composite_alpha = {
                use gfx_hal::window::CompositeAlpha::*;
                [Opaque, Inherit, PreMultiplied, PostMultiplied]
                    .iter()
                    .cloned()
                    .find(|ca| composite_alphas.contains(ca))
                    .ok_or("No CompositeAlpha values specified")?
            };
            let format = match preferred_formats {
                None => Format::Rgba8Srgb,
                Some(formats) => match formats
                    .iter()
                    .find(|format| format.base_format().1 == ChannelType::Srgb)
                    .cloned()
                {
                    Some(srgb_format) => srgb_format,
                    None => formats
                        .get(0)
                        .cloned()
                        .ok_or("Preffered format list was empty")?,
                },
            };
            let extent = caps.extents.end;
            let image_count = if present_mode == PresentMode::Mailbox {
                (caps.image_count.end - 1).min(3)
            } else {
                (caps.image_count.end - 1).min(2)
            };
            let image_layers = 1;
            let image_usage = if caps.usage.contains(Usage::COLOR_ATTACHMENT) {
                Usage::COLOR_ATTACHMENT
            } else {
                Err("The surfade isn't capable of supporting color")?
            };
            let swapchain_config = SwapchainConfig {
                present_mode,
                composite_alpha,
                format,
                extent,
                image_count,
                image_layers,
                image_usage,
            };
            info!("{:?}", swapchain_config);
            //
            let (swapchain, backbuffer) = unsafe {
                device
                    .create_swapchain(&mut surface, swapchain_config, None)
                    .map_err(|_| "Failed to create the swapchain")?
            };
            (swapchain, extent, backbuffer, format, image_count as usize)
        };

        let (image_available_semaphores, render_finished_semaphores, in_flight_fences) = {
            let mut image_available_semaphores: Vec<
                <back::Backend as Backend>::Semaphore,
            > = vec![];
            let mut render_finished_semaphores: Vec<
                <back::Backend as Backend>::Semaphore,
            > = vec![];
            let mut in_flight_fences: Vec<<back::Backend as Backend>::Fence> = vec![];
            for _ in 0..frames_in_flight {
                in_flight_fences.push(device
                    .create_fence(true)
                    .map_err(|_| "Could not create a fence")?);
                image_available_semaphores.push(device
                    .create_semaphore()
                    .map_err(|_| "Could not create a semaphore")?);
                render_finished_semaphores.push(device
                    .create_semaphore()
                    .map_err(|_| "Could not create a semaphore")?);
            }
            (
                image_available_semaphores,
                render_finished_semaphores,
                in_flight_fences,
            )
        };

        let render_pass = {
            let color_attachment = Attachment {
                format: Some(format),
                samples: 1,
                ops: AttachmentOps {
                    load: AttachmentLoadOp::Clear,
                    store: AttachmentStoreOp::Store,
                },
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
            unsafe {
                device
                    .create_render_pass(&[color_attachment], &[subpass], &[])
                    .map_err(|_| "Couldn't create a render pass")?
            }
        };

        let image_views: Vec<_> = match backbuffer {
            Backbuffer::Images(images) => images
                .into_iter()
                .map(|image| unsafe {
                    device
                        .create_image_view(
                            &image,
                            ViewKind::D2,
                            format,
                            Swizzle::NO,
                            SubresourceRange {
                                aspects: Aspects::COLOR,
                                levels: 0..1,
                                layers: 0..1,
                            },
                        )
                        .map_err(|_| "Couldn't create the image_view for the image")
                })
                .collect::<Result<Vec<_>, &str>>()?,
            Backbuffer::Framebuffer(_) => unimplemented!("Can't handle framebuffer backbuffer"),
        };

        let framebuffers: Vec<<back::Backend as Backend>::Framebuffer> = {
            image_views
                .iter()
                .map(|image_view| unsafe {
                    device
                        .create_framebuffer(
                            &render_pass,
                            vec![image_view],
                            Extent {
                                width: extent.width as u32,
                                height: extent.height as u32,
                                depth: 1,
                            },
                        )
                        .map_err(|_| "Failed to create a framebuffer")
                })
                .collect::<Result<Vec<_>, &str>>()?
        };

        let mut command_pool = unsafe {
            device
                .create_command_pool_typed(&queue_group, CommandPoolCreateFlags::RESET_INDIVIDUAL)
                .map_err(|_| "Could not create the raw command pool")?
        };

        let command_buffers: Vec<_> = framebuffers
            .iter()
            .map(|_| command_pool.acquire_command_buffer())
            .collect();

        Ok(Self {
            _instance: ManuallyDrop::new(instance),
            _surface: surface,
            _adapter: adapter,
            device: ManuallyDrop::new(device),
            queue_group,
            swapchain: ManuallyDrop::new(swapchain),
            render_area: extent.to_extent().rect(),
            render_pass: ManuallyDrop::new(render_pass),
            image_views,
            framebuffers,
            command_pool: ManuallyDrop::new(command_pool),
            command_buffers,
            image_available_semaphores,
            render_finished_semaphores,
            in_flight_fences,
            frames_in_flight,
            current_frame: 0,
        })
    }
    pub fn draw_clear_frame(&mut self, color: [f32; 4]) -> Result<(), &'static str> {
        // SETUP FOR THIS FRAME
        let flight_fence = &self.in_flight_fences[self.current_frame];
        let image_available = &self.image_available_semaphores[self.current_frame];
        let render_finished = &self.render_finished_semaphores[self.current_frame];
        // Advance the frame _before_ we start using the `?` operator
        self.current_frame = (self.current_frame + 1) % self.frames_in_flight;

        let (i_u32, i_usize) = unsafe {
            self.device
                .wait_for_fence(flight_fence, core::u64::MAX)
                .map_err(|_| "Failed to wait on the fence!")?;
            self.device
                .reset_fence(flight_fence)
                .map_err(|_| "Couldn't reset the fence!")?;
            let image_index = self.swapchain
                .acquire_image(core::u64::MAX, FrameSync::Semaphore(image_available))
                .map_err(|_| "Couldn't acquire an image from the swapchain!")?;
            (image_index, image_index as usize)
        };

        // RECORD COMMANDS
        unsafe {
            let buffer = &mut self.command_buffers[i_usize];
            let clear_values = [ClearValue::Color(ClearColor::Float(color))];
            buffer.begin(false);
            buffer.begin_render_pass_inline(
                &self.render_pass,
                &self.framebuffers[i_usize],
                self.render_area,
                clear_values.iter(),
            );
            buffer.finish();
        }

        // SUBMISSION AND PRESENT
        let command_buffers = &self.command_buffers[i_usize..=i_usize];
        let wait_semaphores: ArrayVec<[_; 1]> =
            [(image_available, PipelineStage::COLOR_ATTACHMENT_OUTPUT)].into();
        let signal_semaphores: ArrayVec<[_; 1]> = [render_finished].into();
        // yes, you have to write it twice like this. yes, it's silly.
        let present_wait_semaphores: ArrayVec<[_; 1]> = [render_finished].into();
        let submission = Submission {
            command_buffers,
            wait_semaphores,
            signal_semaphores,
        };
        let the_command_queue = &mut self.queue_group.queues[0];
        unsafe {
            the_command_queue.submit(submission, Some(flight_fence));
            self.swapchain
                .present(the_command_queue, i_u32, present_wait_semaphores)
                .map_err(|_| "Failed to present into the swapchain!")
        }
    }
    pub fn recreate_swapchain(&mut self, window: &Window) -> Result<(), &'static str> {
        self.cleanup_swapchain();

        unsafe {
            self.device.reset_fences(&self.in_flight_fences[self.current_frame..self.current_frame]).unwrap();
        }

        let (swapchain, extent, backbuffer, format, frames_in_flight) = {
            let (caps, preferred_formats, present_modes, composite_alphas) =
                self._surface.compatibility(&self._adapter.physical_device);
            info!("{:?}", caps);
            info!("Preferred Formats: {:?}", preferred_formats);
            info!("Present Modes: {:?}", present_modes);
            info!("Composite Alphas: {:?}", composite_alphas);
            //
            let present_mode = {
                use gfx_hal::window::PresentMode::*;
                [Mailbox, Fifo, Relaxed, Immediate]
                    .iter()
                    .cloned()
                    .find(|pm| present_modes.contains(pm))
                    .ok_or("No PresentMode values specified!")?
            };
            let composite_alpha = {
                use gfx_hal::window::CompositeAlpha::*;
                [Opaque, Inherit, PreMultiplied, PostMultiplied]
                    .iter()
                    .cloned()
                    .find(|ca| composite_alphas.contains(ca))
                    .ok_or("No CompositeAlpha values specified")?
            };
            let format = match preferred_formats {
                None => Format::Rgba8Srgb,
                Some(formats) => match formats
                    .iter()
                    .find(|format| format.base_format().1 == ChannelType::Srgb)
                    .cloned()
                {
                    Some(srgb_format) => srgb_format,
                    None => formats
                        .get(0)
                        .cloned()
                        .ok_or("Preffered format list was empty")?,
                },
            };
            let extent = dbg!(caps.extents.end);
            let image_count = if present_mode == PresentMode::Mailbox {
                (caps.image_count.end - 1).min(3)
            } else {
                (caps.image_count.end - 1).min(2)
            };
            let image_layers = 1;
            let image_usage = if caps.usage.contains(Usage::COLOR_ATTACHMENT) {
                Usage::COLOR_ATTACHMENT
            } else {
                Err("The surfade isn't capable of supporting color")?
            };
            let swapchain_config = SwapchainConfig {
                present_mode,
                composite_alpha,
                format,
                extent,
                image_count,
                image_layers,
                image_usage,
            };
            info!("{:?}", swapchain_config);
            //
            let (swapchain, backbuffer) = unsafe {
                self.device
                    .create_swapchain(&mut self._surface, swapchain_config, None)
                    .map_err(|_| "Failed to create the swapchain")?
            };
            (swapchain, extent, backbuffer, format, image_count as usize)
        };

        let render_pass = {
            let color_attachment = Attachment {
                format: Some(format),
                samples: 1,
                ops: AttachmentOps {
                    load: AttachmentLoadOp::Clear,
                    store: AttachmentStoreOp::Store,
                },
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
            unsafe {
                self.device
                    .create_render_pass(&[color_attachment], &[subpass], &[])
                    .map_err(|_| "Couldn't create a render pass")?
            }
        };

        let image_views: Vec<_> = match backbuffer {
            Backbuffer::Images(images) => images
                .into_iter()
                .map(|image| unsafe {
                    self.device
                        .create_image_view(
                            &image,
                            ViewKind::D2,
                            format,
                            Swizzle::NO,
                            SubresourceRange {
                                aspects: Aspects::COLOR,
                                levels: 0..1,
                                layers: 0..1,
                            },
                        )
                        .map_err(|_| "Couldn't create the image_view for the image")
                })
                .collect::<Result<Vec<_>, &str>>()?,
            Backbuffer::Framebuffer(_) => unimplemented!("Can't handle framebuffer backbuffer"),
        };

        let framebuffers: Vec<<back::Backend as Backend>::Framebuffer> = {
            image_views
                .iter()
                .map(|image_view| unsafe {
                    self.device
                        .create_framebuffer(
                            &render_pass,
                            vec![image_view],
                            Extent {
                                width: extent.width as u32,
                                height: extent.height as u32,
                                depth: 1,
                            },
                        )
                        .map_err(|_| "Failed to create a framebuffer")
                })
                .collect::<Result<Vec<_>, &str>>()?
        };


        self.swapchain = ManuallyDrop::new(swapchain);
        self.render_area = extent.to_extent().rect();
        self.render_pass = ManuallyDrop::new(render_pass);
        self.image_views = image_views;
        self.framebuffers = framebuffers;
        self.frames_in_flight = frames_in_flight;
        self.current_frame = (self.current_frame + 1) % self.frames_in_flight;
        Ok(())
    }
}

impl Drop for HalState {
    fn drop(&mut self) {
        let _ = self.device.wait_idle();

        self.cleanup_swapchain();

        unsafe {
            for fence in self.in_flight_fences.drain(..) {
                self.device.destroy_fence(fence)
            }
            for semaphore in self.image_available_semaphores.drain(..) {
                self.device.destroy_semaphore(semaphore)
            }
            for semaphore in self.render_finished_semaphores.drain(..) {
                self.device.destroy_semaphore(semaphore)
            }
            self.device.destroy_command_pool(
                ManuallyDrop::into_inner(read(&mut self.command_pool)).into_raw(),
            );

            ManuallyDrop::drop(&mut self.device);
            ManuallyDrop::drop(&mut self._instance);
        }
    }
}

impl HalState {
    fn cleanup_swapchain(&mut self) {
        let _ = self.device.wait_idle();
        unsafe {
            for framebuffer in self.framebuffers.drain(..) {
                self.device.destroy_framebuffer(framebuffer)
            }

            self.command_pool.reset();

            self.device
                .destroy_render_pass(ManuallyDrop::into_inner(read(&mut self.render_pass)));

            for image_view in self.image_views.drain(..) {
                self.device.destroy_image_view(image_view)
            }

            // TODO: DESTROY GRAPHICAL PIPELINES AND LAYOUTS HERE

            self.device
                .destroy_swapchain(ManuallyDrop::into_inner(read(&mut self.swapchain)));
        }
    }
}
