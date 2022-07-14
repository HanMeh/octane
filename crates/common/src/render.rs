use crate::mesh::{Mesh, Vertex};

use math::prelude::{Matrix, Vector};

use std::cmp;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::iter;
use std::mem;
use std::rc::Rc;
use std::time;

use log::{error, info, trace, warn};
use raw_window_handle::HasRawWindowHandle;

pub const CHUNK_SIZE: usize = 32;

//temporary for here for now.
#[derive(Default, Clone, Copy)]
pub struct UniformBufferObject {
    pub model: Matrix<f32, 4, 4>,
    pub view: Matrix<f32, 4, 4>,
    pub proj: Matrix<f32, 4, 4>,
    pub resolution: Vector<f32, 2>,
    pub render_distance: u32,
}

pub struct RendererInfo<'a> {
    pub window: &'a dyn HasRawWindowHandle,
    pub render_distance: u32,
}

pub trait Renderer {
    fn draw_batch(&mut self, batch: Batch, entries: &'_ [Entry<'_>]);
    fn resize(&mut self, resolution: (u32, u32));
}

#[derive(Clone, Default)]
pub struct Batch {
    pub vertex_shader: &'static str,
    pub fragment_shader: &'static str,
    pub seed_shader: &'static str,
    pub jfa_shader: &'static str,
}

#[derive(Clone, Copy)]
pub struct Entry<'a> {
    pub mesh: &'a Mesh,
}

fn convert_bytes_to_spirv_data(bytes: Vec<u8>) -> Vec<u32> {
    let endian = mem::size_of::<u32>() / mem::size_of::<u8>();

    if bytes.len() % endian != 0 {
        panic!("cannot convert bytes to int; too few or too many")
    }

    let mut buffer = Vec::with_capacity(bytes.len() / endian);

    for slice in bytes.chunks(endian) {
        buffer.push(u32::from_le_bytes(slice.try_into().unwrap()));
    }

    buffer
}

fn debug_utils_messenger_callback(data: &vk::DebugUtilsMessengerCallbackData) -> bool {
    match data.message_severity {
        vk::DEBUG_UTILS_MESSAGE_SEVERITY_VERBOSE => trace!("{}", data.message),
        vk::DEBUG_UTILS_MESSAGE_SEVERITY_INFO => info!("{}", data.message),
        vk::DEBUG_UTILS_MESSAGE_SEVERITY_WARNING => warn!("{}", data.message),
        vk::DEBUG_UTILS_MESSAGE_SEVERITY_ERROR => error!("{}", data.message),
        _ => panic!("unrecognized message severity"),
    }

    false
}

fn create_compute_pipeline(
    device: Rc<vk::Device>,
    stage: vk::PipelineShaderStageCreateInfo<'_>,
    layout: &'_ vk::PipelineLayout,
) -> vk::Pipeline {
    let compute_pipeline_create_info = vk::ComputePipelineCreateInfo {
        stage,
        layout,
        base_pipeline: None,
        base_pipeline_index: -1,
    };

    vk::Pipeline::new_compute_pipelines(device, None, &[compute_pipeline_create_info])
        .expect("failed to create compute pipeline")
        .remove(0)
}

fn create_graphics_pipeline(
    device: Rc<vk::Device>,
    stages: &'_ [vk::PipelineShaderStageCreateInfo<'_>],
    render_pass: &'_ vk::RenderPass,
    layout: &'_ vk::PipelineLayout,
    extent: (u32, u32),
) -> vk::Pipeline {
    let vertex_binding = vk::VertexInputBindingDescription {
        binding: 0,
        stride: mem::size_of::<Vertex>(),
        input_rate: vk::VertexInputRate::Vertex,
    };

    let instance_binding = vk::VertexInputBindingDescription {
        binding: 1,
        stride: mem::size_of::<Vector<f32, 3>>(),
        input_rate: vk::VertexInputRate::Instance,
    };

    let position_attribute = vk::VertexInputAttributeDescription {
        binding: 0,
        location: 0,
        format: vk::Format::Rgb32Sfloat,
        offset: 0,
    };

    let normal_attribute = vk::VertexInputAttributeDescription {
        binding: 0,
        location: 1,
        format: vk::Format::Rgb32Sfloat,
        offset: mem::size_of::<[f32; 3]>() as u32,
    };

    let uv_attribute = vk::VertexInputAttributeDescription {
        binding: 0,
        location: 2,
        format: vk::Format::Rgb32Sfloat,
        offset: 2 * mem::size_of::<[f32; 3]>() as u32,
    };

    let chunk_position_attribute = vk::VertexInputAttributeDescription {
        binding: 1,
        location: 3,
        format: vk::Format::Rgb32Sfloat,
        offset: 0,
    };

    let vertex_input_info = vk::PipelineVertexInputStateCreateInfo {
        bindings: &[vertex_binding, instance_binding],
        attributes: &[
            position_attribute,
            normal_attribute,
            uv_attribute,
            chunk_position_attribute,
        ],
    };

    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo {
        topology: vk::PrimitiveTopology::TriangleList,
        primitive_restart_enable: false,
    };

    let tessellation_state = vk::PipelineTessellationStateCreateInfo {};

    let viewport = vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: extent.0 as _,
        height: extent.1 as _,
        min_depth: 0.0,
        max_depth: 1.0,
    };

    let scissor = vk::Rect2d {
        offset: (0, 0),
        extent,
    };

    let viewport_state = vk::PipelineViewportStateCreateInfo {
        viewports: &[viewport],
        scissors: &[scissor],
    };

    let rasterizer = vk::PipelineRasterizationStateCreateInfo {
        depth_clamp_enable: false,
        rasterizer_discard_enable: false,
        polygon_mode: vk::PolygonMode::Fill,
        //TODO change to front and project raymarch onto backface
        cull_mode: vk::CULL_MODE_FRONT,
        front_face: vk::FrontFace::CounterClockwise,
        depth_bias_enable: false,
        depth_bias_constant_factor: 0.0,
        depth_bias_clamp: 0.0,
        depth_bias_slope_factor: 0.0,
        line_width: 1.0,
    };

    let multisampling = vk::PipelineMultisampleStateCreateInfo {};

    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo {
        depth_test_enable: true,
        depth_write_enable: true,
        depth_compare_op: vk::CompareOp::Less,
        depth_bounds_test_enable: false,
        min_depth_bounds: 0.0,
        max_depth_bounds: 1.0,
    };

    let color_blend_attachment = vk::PipelineColorBlendAttachmentState {
        color_write_mask: vk::COLOR_COMPONENT_R
            | vk::COLOR_COMPONENT_G
            | vk::COLOR_COMPONENT_B
            | vk::COLOR_COMPONENT_A,
        blend_enable: true,
        src_color_blend_factor: vk::BlendFactor::SrcAlpha,
        dst_color_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        color_blend_op: vk::BlendOp::Add,
        src_alpha_blend_factor: vk::BlendFactor::One,
        dst_alpha_blend_factor: vk::BlendFactor::Zero,
        alpha_blend_op: vk::BlendOp::Add,
    };

    let color_blending = vk::PipelineColorBlendStateCreateInfo {
        logic_op_enable: false,
        logic_op: vk::LogicOp::Copy,
        attachments: &[color_blend_attachment],
        blend_constants: &[0.0, 0.0, 0.0, 0.0],
    };

    let dynamic_state = vk::PipelineDynamicStateCreateInfo {
        dynamic_states: &[],
    };

    let graphics_pipeline_create_info = vk::GraphicsPipelineCreateInfo {
        stages,
        vertex_input_state: &vertex_input_info,
        input_assembly_state: &input_assembly,
        tessellation_state: &tessellation_state,
        viewport_state: &viewport_state,
        rasterization_state: &rasterizer,
        multisample_state: &multisampling,
        depth_stencil_state: &depth_stencil,
        color_blend_state: &color_blending,
        dynamic_state: &dynamic_state,
        layout: &layout,
        render_pass: &render_pass,
        subpass: 0,
        base_pipeline: None,
        base_pipeline_index: -1,
    };

    vk::Pipeline::new_graphics_pipelines(device, None, &[graphics_pipeline_create_info])
        .expect("failed to create graphics pipeline")
        .remove(0)
}

pub struct Vulkan {
    pub ubo: UniformBufferObject,
    last_batch: Batch,
    cubelet_sdf_sampler: vk::Sampler,
    cubelet_sdf_view: vk::ImageView,
    cubelet_sdf_memory: vk::Memory,
    cubelet_sdf: vk::Image,
    cubelet_data_sampler: vk::Sampler,
    cubelet_data_view: vk::ImageView,
    cubelet_data_memory: vk::Memory,
    cubelet_data: vk::Image,
    instance_buffer_memory: vk::Memory,
    instance_buffer: vk::Buffer,
    data_buffer_memory: vk::Memory,
    data_buffer: vk::Buffer,
    staging_buffer_memory: vk::Memory,
    staging_buffer: vk::Buffer,
    image_available_semaphore: vk::Semaphore,
    render_finished_semaphore: vk::Semaphore,
    in_flight_fence: vk::Fence,
    command_buffer: vk::CommandBuffer,
    command_pool: vk::CommandPool,
    render_info: VulkanRenderInfo,
    render_data: Option<VulkanRenderData>,
    compute_data: Option<VulkanComputeData>,
    queue: vk::Queue,
    device: Rc<vk::Device>,
    physical_device: vk::PhysicalDevice,
    shaders: HashMap<&'static str, vk::ShaderModule>,
    shader_mod_time: HashMap<String, time::SystemTime>,
    surface: vk::Surface,
    #[cfg(debug_assertions)]
    debug_utils_messenger: vk::DebugUtilsMessenger,
    pub instance: Rc<vk::Instance>,
}

pub struct VulkanRenderInfo {
    image_count: u32,
    surface_format: vk::SurfaceFormat,
    surface_capabilities: vk::SurfaceCapabilities,
    present_mode: vk::PresentMode,
    extent: (u32, u32),
}

pub struct VulkanComputeData {
    seed_pipeline: vk::Pipeline,
    seed_pipeline_layout: vk::PipelineLayout,
    seed_descriptor_sets: Vec<vk::DescriptorSet>,
    seed_descriptor_pool: vk::DescriptorPool,
    seed_descriptor_set_layout: vk::DescriptorSetLayout,
    jfa_pipeline: vk::Pipeline,
    jfa_pipeline_layout: vk::PipelineLayout,
    jfa_descriptor_sets: Vec<vk::DescriptorSet>,
    jfa_descriptor_pool: vk::DescriptorPool,
    jfa_descriptor_set_layout: vk::DescriptorSetLayout,
}

impl VulkanComputeData {
    pub fn init(
        device: Rc<vk::Device>,
        seed_stage: vk::PipelineShaderStageCreateInfo<'_>,
        jfa_stage: vk::PipelineShaderStageCreateInfo<'_>,
    ) -> Self {
        /*let uniform_buffer_binding = vk::DescriptorSetLayoutBinding {
            binding: 0,
            descriptor_type: vk::DescriptorType::UniformBuffer,
            descriptor_count: 1,
            stage: vk::SHADER_STAGE_VERTEX | vk::SHADER_STAGE_FRAGMENT,
        };
        */

        let seed_descriptor_set_layout_create_info =
            vk::DescriptorSetLayoutCreateInfo { bindings: &[] };

        let seed_descriptor_set_layout =
            vk::DescriptorSetLayout::new(device.clone(), seed_descriptor_set_layout_create_info)
                .expect("failed to create descriptor set layout");

        /*let uniform_buffer_pool_size = vk::DescriptorPoolSize {
            descriptor_type: vk::DescriptorType::UniformBuffer,
            descriptor_count: swapchain_images.len() as _,
        };*/

        let seed_descriptor_pool_create_info = vk::DescriptorPoolCreateInfo {
            max_sets: 1,
            pool_sizes: &[],
        };

        let seed_descriptor_pool =
            vk::DescriptorPool::new(device.clone(), seed_descriptor_pool_create_info)
                .expect("failed to create descriptor pool");

        let seed_descriptor_set_allocate_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool: &seed_descriptor_pool,
            set_layouts: &[&seed_descriptor_set_layout],
        };

        let seed_descriptor_sets =
            vk::DescriptorSet::allocate(device.clone(), seed_descriptor_set_allocate_info)
                .expect("failed to allocate descriptor sets");

        let seed_pipeline_layout_create_info = vk::PipelineLayoutCreateInfo {
            set_layouts: &[&seed_descriptor_set_layout],
        };

        let seed_pipeline_layout =
            vk::PipelineLayout::new(device.clone(), seed_pipeline_layout_create_info)
                .expect("failed to create pipeline layout");

        let seed_pipeline =
            create_compute_pipeline(device.clone(), seed_stage, &seed_pipeline_layout);

        /*let uniform_buffer_binding = vk::DescriptorSetLayoutBinding {
            binding: 0,
            descriptor_type: vk::DescriptorType::UniformBuffer,
            descriptor_count: 1,
            stage: vk::SHADER_STAGE_VERTEX | vk::SHADER_STAGE_FRAGMENT,
        };
        */

        let jfa_descriptor_set_layout_create_info =
            vk::DescriptorSetLayoutCreateInfo { bindings: &[] };

        let jfa_descriptor_set_layout =
            vk::DescriptorSetLayout::new(device.clone(), jfa_descriptor_set_layout_create_info)
                .expect("failed to create descriptor set layout");

        /*let uniform_buffer_pool_size = vk::DescriptorPoolSize {
            descriptor_type: vk::DescriptorType::UniformBuffer,
            descriptor_count: swapchain_images.len() as _,
        };*/

        let jfa_descriptor_pool_create_info = vk::DescriptorPoolCreateInfo {
            max_sets: 1,
            pool_sizes: &[],
        };

        let jfa_descriptor_pool =
            vk::DescriptorPool::new(device.clone(), jfa_descriptor_pool_create_info)
                .expect("failed to create descriptor pool");

        let descriptor_set_allocate_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool: &jfa_descriptor_pool,
            set_layouts: &[&jfa_descriptor_set_layout],
        };

        let jfa_descriptor_sets =
            vk::DescriptorSet::allocate(device.clone(), descriptor_set_allocate_info)
                .expect("failed to allocate descriptor sets");

        let jfa_pipeline_layout_create_info = vk::PipelineLayoutCreateInfo {
            set_layouts: &[&jfa_descriptor_set_layout],
        };

        let jfa_pipeline_layout =
            vk::PipelineLayout::new(device.clone(), jfa_pipeline_layout_create_info)
                .expect("failed to create pipeline layout");

        let jfa_pipeline = create_compute_pipeline(device.clone(), jfa_stage, &jfa_pipeline_layout);

        Self {
            seed_pipeline,
            seed_pipeline_layout,
            seed_descriptor_sets,
            seed_descriptor_pool,
            seed_descriptor_set_layout,
            jfa_pipeline,
            jfa_pipeline_layout,
            jfa_descriptor_sets,
            jfa_descriptor_pool,
            jfa_descriptor_set_layout,
        }
    }
}

pub struct VulkanRenderData {
    depth_view: vk::ImageView,
    depth_memory: vk::Memory,
    depth: vk::Image,
    framebuffers: Vec<vk::Framebuffer>,
    graphics_pipeline: vk::Pipeline,
    graphics_pipeline_layout: vk::PipelineLayout,
    descriptor_sets: Vec<vk::DescriptorSet>,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set_layout: vk::DescriptorSetLayout,
    render_pass: vk::RenderPass,
    swapchain_image_views: Vec<vk::ImageView>,
    swapchain: vk::Swapchain,
}

impl VulkanRenderData {
    pub fn init(
        device: Rc<vk::Device>,
        physical_device: &vk::PhysicalDevice,
        surface: &vk::Surface,
        shader_stages: &'_ [vk::PipelineShaderStageCreateInfo<'_>],
        old_swapchain: Option<vk::Swapchain>,
        render_info: &VulkanRenderInfo,
    ) -> Self {
        let depth_create_info = vk::ImageCreateInfo {
            image_type: vk::ImageType::TwoDim,
            format: vk::Format::D32Sfloat,
            extent: (render_info.extent.0, render_info.extent.1, 1),
            mip_levels: 1,
            array_layers: 1,
            samples: vk::SAMPLE_COUNT_1,
            tiling: vk::ImageTiling::Optimal,
            image_usage: vk::IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT,
            initial_layout: vk::ImageLayout::Undefined,
        };

        let mut depth =
            vk::Image::new(device.clone(), depth_create_info).expect("failed to allocate image");

        let depth_memory_allocate_info = vk::MemoryAllocateInfo {
            property_flags: vk::MEMORY_PROPERTY_DEVICE_LOCAL,
        };

        let depth_memory = vk::Memory::allocate(
            device.clone(),
            depth_memory_allocate_info,
            depth.memory_requirements(),
            physical_device.memory_properties(),
        )
        .expect("failed to allocate memory");

        depth
            .bind_memory(&depth_memory)
            .expect("failed to bind image to memory");

        let depth_view_create_info = vk::ImageViewCreateInfo {
            image: &depth,
            view_type: vk::ImageViewType::TwoDim,
            format: vk::Format::D32Sfloat,
            components: vk::ComponentMapping {
                r: vk::ComponentSwizzle::Identity,
                g: vk::ComponentSwizzle::Identity,
                b: vk::ComponentSwizzle::Identity,
                a: vk::ComponentSwizzle::Identity,
            },
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: vk::IMAGE_ASPECT_DEPTH,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            },
        };

        let depth_view = vk::ImageView::new(device.clone(), depth_view_create_info)
            .expect("failed to create image view");

        let swapchain_create_info = vk::SwapchainCreateInfo {
            surface,
            min_image_count: render_info.image_count,
            image_format: render_info.surface_format.format,
            image_color_space: render_info.surface_format.color_space,
            image_extent: render_info.extent,
            image_array_layers: 1,
            image_usage: vk::IMAGE_USAGE_COLOR_ATTACHMENT,
            //TODO support concurrent image sharing mode
            image_sharing_mode: vk::SharingMode::Exclusive,
            queue_family_indices: &[],
            pre_transform: render_info.surface_capabilities.current_transform,
            composite_alpha: vk::CompositeAlpha::Opaque,
            present_mode: render_info.present_mode,
            clipped: true,
            old_swapchain,
        };

        let mut swapchain = vk::Swapchain::new(device.clone(), swapchain_create_info)
            .expect("failed to create swapchain");

        let swapchain_images = swapchain.images();

        let swapchain_image_views = swapchain_images
            .iter()
            .map(|image| {
                let create_info = vk::ImageViewCreateInfo {
                    image,
                    view_type: vk::ImageViewType::TwoDim,
                    format: render_info.surface_format.format,
                    components: vk::ComponentMapping {
                        r: vk::ComponentSwizzle::Identity,
                        g: vk::ComponentSwizzle::Identity,
                        b: vk::ComponentSwizzle::Identity,
                        a: vk::ComponentSwizzle::Identity,
                    },
                    subresource_range: vk::ImageSubresourceRange {
                        aspect_mask: vk::IMAGE_ASPECT_COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    },
                };

                vk::ImageView::new(device.clone(), create_info)
                    .expect("failed to create image view")
            })
            .collect::<Vec<_>>();

        let uniform_buffer_binding = vk::DescriptorSetLayoutBinding {
            binding: 0,
            descriptor_type: vk::DescriptorType::UniformBuffer,
            descriptor_count: 1,
            stage: vk::SHADER_STAGE_VERTEX | vk::SHADER_STAGE_FRAGMENT,
        };

        let cubelet_data_binding = vk::DescriptorSetLayoutBinding {
            binding: 1,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 1,
            stage: vk::SHADER_STAGE_FRAGMENT,
        };

        let cubelet_sdf_binding = vk::DescriptorSetLayoutBinding {
            binding: 2,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 1,
            stage: vk::SHADER_STAGE_FRAGMENT,
        };

        let descriptor_set_layout_create_info = vk::DescriptorSetLayoutCreateInfo {
            bindings: &[
                uniform_buffer_binding,
                cubelet_data_binding,
                cubelet_sdf_binding,
            ],
        };

        let descriptor_set_layout =
            vk::DescriptorSetLayout::new(device.clone(), descriptor_set_layout_create_info)
                .expect("failed to create descriptor set layout");

        let uniform_buffer_pool_size = vk::DescriptorPoolSize {
            descriptor_type: vk::DescriptorType::UniformBuffer,
            descriptor_count: swapchain_images.len() as _,
        };

        let cubelet_data_pool_size = vk::DescriptorPoolSize {
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: swapchain_images.len() as _,
        };

        let cubelet_sdf_pool_size = vk::DescriptorPoolSize {
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: swapchain_images.len() as _,
        };

        let descriptor_pool_create_info = vk::DescriptorPoolCreateInfo {
            max_sets: swapchain_images.len() as _,
            pool_sizes: &[
                uniform_buffer_pool_size,
                cubelet_data_pool_size,
                cubelet_sdf_pool_size,
            ],
        };

        let descriptor_pool = vk::DescriptorPool::new(device.clone(), descriptor_pool_create_info)
            .expect("failed to create descriptor pool");

        let set_layouts = iter::repeat(&descriptor_set_layout)
            .take(swapchain_images.len() as _)
            .collect::<Vec<_>>();

        let descriptor_set_allocate_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool: &descriptor_pool,
            set_layouts: &set_layouts,
        };

        let descriptor_sets =
            vk::DescriptorSet::allocate(device.clone(), descriptor_set_allocate_info)
                .expect("failed to allocate descriptor sets");

        let graphics_pipeline_layout_create_info = vk::PipelineLayoutCreateInfo {
            set_layouts: &[&descriptor_set_layout],
        };

        let graphics_pipeline_layout =
            vk::PipelineLayout::new(device.clone(), graphics_pipeline_layout_create_info)
                .expect("failed to create pipeline layout");

        let depth_attachment_description = vk::AttachmentDescription {
            format: vk::Format::D32Sfloat,
            samples: vk::SAMPLE_COUNT_1,
            load_op: vk::AttachmentLoadOp::Clear,
            store_op: vk::AttachmentStoreOp::DontCare,
            stencil_load_op: vk::AttachmentLoadOp::DontCare,
            stencil_store_op: vk::AttachmentStoreOp::DontCare,
            initial_layout: vk::ImageLayout::Undefined,
            final_layout: vk::ImageLayout::DepthStencilAttachment,
        };

        let color_attachment_description = vk::AttachmentDescription {
            format: render_info.surface_format.format,
            samples: vk::SAMPLE_COUNT_1,
            load_op: vk::AttachmentLoadOp::Clear,
            store_op: vk::AttachmentStoreOp::Store,
            stencil_load_op: vk::AttachmentLoadOp::DontCare,
            stencil_store_op: vk::AttachmentStoreOp::DontCare,
            initial_layout: vk::ImageLayout::Undefined,
            final_layout: vk::ImageLayout::PresentSrc,
        };

        let color_attachment_reference = vk::AttachmentReference {
            attachment: 0,
            layout: vk::ImageLayout::ColorAttachment,
        };

        let depth_attachment_reference = vk::AttachmentReference {
            attachment: 1,
            layout: vk::ImageLayout::DepthStencilAttachment,
        };

        let subpass_description = vk::SubpassDescription {
            pipeline_bind_point: vk::PipelineBindPoint::Graphics,
            input_attachments: &[],
            color_attachments: &[color_attachment_reference],
            resolve_attachments: &[],
            depth_stencil_attachment: Some(&depth_attachment_reference),
            preserve_attachments: &[],
        };

        let subpass_dependency = vk::SubpassDependency {
            src_subpass: vk::SUBPASS_EXTERNAL,
            dst_subpass: 0,
            src_stage_mask: vk::PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT
                | vk::PIPELINE_STAGE_EARLY_FRAGMENT_TESTS,
            src_access_mask: 0,
            dst_stage_mask: vk::PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT
                | vk::PIPELINE_STAGE_EARLY_FRAGMENT_TESTS,
            dst_access_mask: vk::ACCESS_COLOR_ATTACHMENT_WRITE
                | vk::ACCESS_DEPTH_STENCIL_ATTACHMENT_WRITE,
        };

        let render_pass_create_info = vk::RenderPassCreateInfo {
            attachments: &[color_attachment_description, depth_attachment_description],
            subpasses: &[subpass_description],
            dependencies: &[subpass_dependency],
        };

        let render_pass = vk::RenderPass::new(device.clone(), render_pass_create_info)
            .expect("failed to create render pass");

        let framebuffers = swapchain_image_views
            .iter()
            .map(|image_view| {
                let framebuffer_create_info = vk::FramebufferCreateInfo {
                    render_pass: &render_pass,
                    attachments: &[image_view, &depth_view],
                    width: render_info.extent.0,
                    height: render_info.extent.1,
                    layers: 1,
                };

                vk::Framebuffer::new(device.clone(), framebuffer_create_info)
                    .expect("failed to create framebuffer")
            })
            .collect::<Vec<_>>();

        let graphics_pipeline = create_graphics_pipeline(
            device.clone(),
            shader_stages,
            &render_pass,
            &graphics_pipeline_layout,
            render_info.extent,
        );

        Self {
            depth_view,
            depth_memory,
            depth,
            swapchain,
            swapchain_image_views,
            render_pass,
            descriptor_set_layout,
            descriptor_pool,
            descriptor_sets,
            graphics_pipeline_layout,
            graphics_pipeline,
            framebuffers,
        }
    }
}

impl Vulkan {
    pub fn init(info: RendererInfo<'_>) -> Self {
        let application_info = vk::ApplicationInfo {
            application_name: "Octane",
            application_version: (0, 1, 0).into(),
            engine_name: "Octane",
            engine_version: (0, 1, 0).into(),
            api_version: (1, 0, 0).into(),
        };

        let mut extensions = vec![vk::KHR_SURFACE, vk::KHR_XLIB_SURFACE];
        let mut layers = vec![];

        let mut debug_utils_messenger_create_info = None;

        #[cfg(debug_assertions)]
        {
            extensions.push(vk::EXT_DEBUG_UTILS);
            layers.push(vk::LAYER_KHRONOS_VALIDATION);

            debug_utils_messenger_create_info = Some(vk::DebugUtilsMessengerCreateInfo {
                message_severity: vk::DEBUG_UTILS_MESSAGE_SEVERITY_VERBOSE
                    | vk::DEBUG_UTILS_MESSAGE_SEVERITY_INFO
                    | vk::DEBUG_UTILS_MESSAGE_SEVERITY_WARNING
                    | vk::DEBUG_UTILS_MESSAGE_SEVERITY_ERROR,
                message_type: vk::DEBUG_UTILS_MESSAGE_TYPE_GENERAL
                    | vk::DEBUG_UTILS_MESSAGE_TYPE_VALIDATION
                    | vk::DEBUG_UTILS_MESSAGE_TYPE_PERFORMANCE,
                user_callback: debug_utils_messenger_callback,
            });
        }

        let instance_create_info = vk::InstanceCreateInfo {
            application_info: &application_info,
            extensions: &extensions[..],
            layers: &layers[..],
            debug_utils: &debug_utils_messenger_create_info,
        };

        let instance = vk::Instance::new(instance_create_info).expect("failed to create instance");

        #[cfg(debug_assertions)]
        let debug_utils_messenger = vk::DebugUtilsMessenger::new(
            instance.clone(),
            debug_utils_messenger_create_info.unwrap(),
        )
        .expect("failed to create debug utils messenger");

        let surface = vk::Surface::new(instance.clone(), &info.window);

        let physical_device = {
            let mut candidates = vk::PhysicalDevice::enumerate(instance.clone())
                .into_iter()
                .map(|x| (0, x.properties(), x)) // suitability of 0, pd properties, pd
                .collect::<Vec<_>>();

            if candidates.len() == 0 {
                panic!("no suitable gpu");
            }

            for (suitability, properties, _) in &mut candidates {
                if properties.device_type == vk::PhysicalDeviceType::Discrete {
                    *suitability += 420;
                }

                *suitability += properties.limits.max_image_dimension_2d;

                trace!(
                    "Found GPU \"{}\" with suitability of {}",
                    properties.device_name,
                    suitability
                );
            }

            candidates.sort_by(|(a, _, _), (b, _, _)| a.cmp(b));

            let (_, properties, physical_device) = candidates.remove(0);

            info!("Selected GPU \"{}\"", properties.device_name);

            physical_device
        };

        let queue_families = physical_device.queue_families();

        let mut queue_family_index = None;

        for (i, queue_family) in queue_families.iter().enumerate() {
            if queue_family.queue_flags & vk::QUEUE_GRAPHICS == 0 {
                continue;
            }
            if queue_family.queue_flags & vk::QUEUE_COMPUTE == 0 {
                continue;
            }
            if !physical_device
                .surface_supported(&surface, i as _)
                .expect("failed to query surface support")
            {
                continue;
            }
            queue_family_index = Some(i as u32);
            break;
        }

        let queue_family_index = queue_family_index.expect("failed to find suitable queue");

        let queue_create_info = vk::DeviceQueueCreateInfo {
            queue_family_index,
            queue_priorities: &[1.0],
        };

        let physical_device_features = vk::PhysicalDeviceFeatures {};

        let device_create_info = vk::DeviceCreateInfo {
            queues: &[queue_create_info],
            enabled_features: &physical_device_features,
            extensions: &[vk::KHR_SWAPCHAIN],
            layers: &layers[..],
        };

        let device = vk::Device::new(&physical_device, device_create_info)
            .expect("failed to create logical device");

        let mut queue = device.queue(queue_family_index);

        let shaders = HashMap::new();
        let shader_mod_time = HashMap::new();

        let surface_capabilities = physical_device.surface_capabilities(&surface);

        //TODO query and choose system compatible
        let surface_format = vk::SurfaceFormat {
            format: vk::Format::Bgra8Srgb,
            color_space: vk::ColorSpace::SrgbNonlinear,
        };

        //TODO query and choose system compatible
        let present_mode = vk::PresentMode::Fifo;

        let image_count = surface_capabilities.min_image_count + 1;

        let render_info = VulkanRenderInfo {
            image_count,
            surface_format,
            surface_capabilities,
            present_mode,
            extent: (960, 540),
        };

        let render_data = None;

        let compute_data = None;

        let command_pool_create_info = vk::CommandPoolCreateInfo { queue_family_index };

        let command_pool = vk::CommandPool::new(device.clone(), command_pool_create_info)
            .expect("failed to create command pool");

        let command_buffer_allocate_info = vk::CommandBufferAllocateInfo {
            command_pool: &command_pool,
            level: vk::CommandBufferLevel::Primary,
            count: 1,
        };

        let mut command_buffer =
            vk::CommandBuffer::allocate(device.clone(), command_buffer_allocate_info)
                .expect("failed to create command buffer")
                .remove(0);

        let semaphore_create_info = vk::SemaphoreCreateInfo {};

        let mut image_available_semaphore =
            vk::Semaphore::new(device.clone(), semaphore_create_info)
                .expect("failed to create semaphore");

        let semaphore_create_info = vk::SemaphoreCreateInfo {};

        let mut render_finished_semaphore =
            vk::Semaphore::new(device.clone(), semaphore_create_info)
                .expect("failed to create semaphore");

        let fence_create_info = vk::FenceCreateInfo {};

        let mut in_flight_fence =
            vk::Fence::new(device.clone(), fence_create_info).expect("failed to create fence");

        let last_batch = Batch::default();

        let mut instance_buffer = vk::Buffer::new(
            device.clone(),
            32768,
            vk::BUFFER_USAGE_TRANSFER_DST | vk::BUFFER_USAGE_VERTEX,
        )
        .expect("failed to create buffer");

        let instance_buffer_memory_allocate_info = vk::MemoryAllocateInfo {
            property_flags: vk::MEMORY_PROPERTY_DEVICE_LOCAL,
        };

        let instance_buffer_memory = vk::Memory::allocate(
            device.clone(),
            instance_buffer_memory_allocate_info,
            instance_buffer.memory_requirements(),
            physical_device.memory_properties(),
        )
        .expect("failed to allocate memory");

        instance_buffer.bind_memory(&instance_buffer_memory);

        let mut data_buffer = vk::Buffer::new(
            device.clone(),
            32768,
            vk::BUFFER_USAGE_TRANSFER_DST
                | vk::BUFFER_USAGE_VERTEX
                | vk::BUFFER_USAGE_INDEX
                | vk::BUFFER_USAGE_UNIFORM,
        )
        .expect("failed to create buffer");

        let data_buffer_memory_allocate_info = vk::MemoryAllocateInfo {
            property_flags: vk::MEMORY_PROPERTY_DEVICE_LOCAL,
        };

        let data_buffer_memory = vk::Memory::allocate(
            device.clone(),
            data_buffer_memory_allocate_info,
            data_buffer.memory_requirements(),
            physical_device.memory_properties(),
        )
        .expect("failed to allocate memory");

        data_buffer.bind_memory(&data_buffer_memory);

        let mut staging_buffer =
            vk::Buffer::new(device.clone(), 3200000000, vk::BUFFER_USAGE_TRANSFER_SRC)
                .expect("failed to create buffer");

        let staging_buffer_memory_allocate_info = vk::MemoryAllocateInfo {
            property_flags: vk::MEMORY_PROPERTY_HOST_VISIBLE | vk::MEMORY_PROPERTY_HOST_COHERENT,
        };

        let staging_buffer_memory = vk::Memory::allocate(
            device.clone(),
            staging_buffer_memory_allocate_info,
            staging_buffer.memory_requirements(),
            physical_device.memory_properties(),
        )
        .expect("failed to allocate memory");

        staging_buffer
            .bind_memory(&staging_buffer_memory)
            .expect("failed to bind buffer");

        let mut ubo = UniformBufferObject::default();
        ubo.resolution = Vector::<f32, 2>::new([960.0, 540.0]);

        let render_distance = info.render_distance;

        ubo.render_distance = render_distance as u32;

        let cubelet_size = 2 * render_distance as usize * CHUNK_SIZE;

        let cubelet_data_create_info = vk::ImageCreateInfo {
            image_type: vk::ImageType::ThreeDim,
            format: vk::Format::Rgba32Sfloat,
            extent: (cubelet_size as _, cubelet_size as _, cubelet_size as _),
            mip_levels: 1,
            array_layers: 1,
            samples: vk::SAMPLE_COUNT_1,
            tiling: vk::ImageTiling::Optimal,
            image_usage: vk::IMAGE_USAGE_TRANSFER_DST | vk::IMAGE_USAGE_SAMPLED,
            initial_layout: vk::ImageLayout::Undefined,
        };

        let mut cubelet_data = vk::Image::new(device.clone(), cubelet_data_create_info)
            .expect("failed to allocate image");

        let cubelet_data_memory_allocate_info = vk::MemoryAllocateInfo {
            property_flags: vk::MEMORY_PROPERTY_DEVICE_LOCAL,
        };

        let cubelet_data_memory = vk::Memory::allocate(
            device.clone(),
            cubelet_data_memory_allocate_info,
            cubelet_data.memory_requirements(),
            physical_device.memory_properties(),
        )
        .expect("failed to allocate memory");

        cubelet_data
            .bind_memory(&cubelet_data_memory)
            .expect("failed to bind image to memory");

        let cubelet_data_view_create_info = vk::ImageViewCreateInfo {
            image: &cubelet_data,
            view_type: vk::ImageViewType::ThreeDim,
            format: vk::Format::Rgba32Sfloat,
            components: vk::ComponentMapping {
                r: vk::ComponentSwizzle::Identity,
                g: vk::ComponentSwizzle::Identity,
                b: vk::ComponentSwizzle::Identity,
                a: vk::ComponentSwizzle::Identity,
            },
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: vk::IMAGE_ASPECT_COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            },
        };

        let cubelet_data_view = vk::ImageView::new(device.clone(), cubelet_data_view_create_info)
            .expect("failed to create image view");

        let cubelet_data_sampler_create_info = vk::SamplerCreateInfo {
            mag_filter: vk::Filter::Nearest,
            min_filter: vk::Filter::Nearest,
            mipmap_mode: vk::SamplerMipmapMode::Nearest,
            address_mode_u: vk::SamplerAddressMode::ClampToBorder,
            address_mode_v: vk::SamplerAddressMode::ClampToBorder,
            address_mode_w: vk::SamplerAddressMode::ClampToBorder,
            mip_lod_bias: 0.0,
            anisotropy_enable: false,
            max_anisotropy: 0.0,
            compare_enable: false,
            compare_op: vk::CompareOp::Always,
            min_lod: 0.0,
            max_lod: 0.0,
            border_color: vk::BorderColor::IntTransparentBlack,
            unnormalized_coordinates: false,
        };

        let cubelet_data_sampler =
            vk::Sampler::new(device.clone(), cubelet_data_sampler_create_info)
                .expect("failed to create sampler");

        let cubelet_sdf_create_info = vk::ImageCreateInfo {
            image_type: vk::ImageType::ThreeDim,
            format: vk::Format::Rgba32Sfloat,
            extent: (cubelet_size as _, cubelet_size as _, cubelet_size as _),
            mip_levels: 1,
            array_layers: 1,
            samples: vk::SAMPLE_COUNT_1,
            tiling: vk::ImageTiling::Optimal,
            image_usage: vk::IMAGE_USAGE_TRANSFER_DST | vk::IMAGE_USAGE_SAMPLED,
            initial_layout: vk::ImageLayout::Undefined,
        };

        let mut cubelet_sdf = vk::Image::new(device.clone(), cubelet_sdf_create_info)
            .expect("failed to allocate image");

        let cubelet_sdf_memory_allocate_info = vk::MemoryAllocateInfo {
            property_flags: vk::MEMORY_PROPERTY_DEVICE_LOCAL,
        };

        let cubelet_sdf_memory = vk::Memory::allocate(
            device.clone(),
            cubelet_sdf_memory_allocate_info,
            cubelet_sdf.memory_requirements(),
            physical_device.memory_properties(),
        )
        .expect("failed to allocate memory");

        cubelet_sdf
            .bind_memory(&cubelet_sdf_memory)
            .expect("failed to bind memory to image");

        let cubelet_sdf_view_create_info = vk::ImageViewCreateInfo {
            image: &cubelet_sdf,
            view_type: vk::ImageViewType::ThreeDim,
            format: vk::Format::Rgba32Sfloat,
            components: vk::ComponentMapping {
                r: vk::ComponentSwizzle::Identity,
                g: vk::ComponentSwizzle::Identity,
                b: vk::ComponentSwizzle::Identity,
                a: vk::ComponentSwizzle::Identity,
            },
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: vk::IMAGE_ASPECT_COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            },
        };

        let cubelet_sdf_view = vk::ImageView::new(device.clone(), cubelet_sdf_view_create_info)
            .expect("failed to create image view");

        let cubelet_sdf_sampler_create_info = vk::SamplerCreateInfo {
            mag_filter: vk::Filter::Nearest,
            min_filter: vk::Filter::Nearest,
            mipmap_mode: vk::SamplerMipmapMode::Nearest,
            address_mode_u: vk::SamplerAddressMode::ClampToBorder,
            address_mode_v: vk::SamplerAddressMode::ClampToBorder,
            address_mode_w: vk::SamplerAddressMode::ClampToBorder,
            mip_lod_bias: 0.0,
            anisotropy_enable: false,
            max_anisotropy: 0.0,
            compare_enable: false,
            compare_op: vk::CompareOp::Always,
            min_lod: 0.0,
            max_lod: 0.0,
            border_color: vk::BorderColor::IntTransparentBlack,
            unnormalized_coordinates: false,
        };

        let cubelet_sdf_sampler = vk::Sampler::new(device.clone(), cubelet_sdf_sampler_create_info)
            .expect("failed to create sampler");

        //let mut rgba_data = [[[[0_f32; 4]; CHUNK_SIZE]; CHUNK_SIZE]; CHUNK_SIZE];
        //let mut sdf_data = [[[0_f32; CHUNK_SIZE]; CHUNK_SIZE]; CHUNK_SIZE];

        let ct = 2 * ubo.render_distance as usize * CHUNK_SIZE;
        let mut voxels = 0;

        use noise::NoiseFn;
        let perlin = noise::Perlin::new();

        let mut pool: Vec<Vec<Vec<f32>>> = vec![];

        staging_buffer_memory
            .write(0, |data: &'_ mut [[f32; 4]]| {
                for x in 0..ct {
                    pool.push(vec![]);
                    for y in 0..ct {
                        pool[x].push(vec![]);
                        for z in 0..ct {
                            let max_y = ((ct / 3) as isize
                                + (10.0 * perlin.get([x as f64 / 32.0, z as f64 / 32.0])) as isize)
                                as usize;
                            if y < max_y {
                                let color: [f32; 4] = [0.0, 0.6, 0.1, 1.0];

                                pool[x][y].push(0.0);
                                data[voxels..voxels + 1].copy_from_slice(&[color]);
                            } else {
                                pool[x][y].push(100000.0);
                            }

                            voxels += 1;
                        }
                    }
                }
            })
            .expect("failed to write to buffer");

        command_buffer
            .record(|commands| {
                let barrier = vk::ImageMemoryBarrier {
                    old_layout: vk::ImageLayout::Undefined,
                    new_layout: vk::ImageLayout::TransferDst,
                    src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                    dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                    image: &cubelet_data,
                    src_access_mask: 0,
                    dst_access_mask: 0,
                    subresource_range: vk::ImageSubresourceRange {
                        aspect_mask: vk::IMAGE_ASPECT_COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    },
                };

                commands.pipeline_barrier(
                    vk::PIPELINE_STAGE_TOP_OF_PIPE,
                    vk::PIPELINE_STAGE_TRANSFER,
                    0,
                    &[],
                    &[],
                    &[barrier],
                );

                let buffer_image_copy = vk::BufferImageCopy {
                    buffer_offset: 0,
                    buffer_row_length: 0,
                    buffer_image_height: 0,
                    image_subresource: vk::ImageSubresourceLayers {
                        aspect_mask: vk::IMAGE_ASPECT_COLOR,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: 1,
                    },
                    image_offset: (0, 0, 0),
                    image_extent: (ct as _, ct as _, ct as _),
                };

                commands.copy_buffer_to_image(
                    &staging_buffer,
                    &mut cubelet_data,
                    vk::ImageLayout::TransferDst,
                    &[buffer_image_copy],
                );

                let barrier = vk::ImageMemoryBarrier {
                    old_layout: vk::ImageLayout::TransferDst,
                    new_layout: vk::ImageLayout::ShaderReadOnly,
                    src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                    dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                    image: &cubelet_data,
                    src_access_mask: 0,
                    dst_access_mask: 0,
                    subresource_range: vk::ImageSubresourceRange {
                        aspect_mask: vk::IMAGE_ASPECT_COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    },
                };

                commands.pipeline_barrier(
                    vk::PIPELINE_STAGE_TRANSFER,
                    vk::PIPELINE_STAGE_FRAGMENT_SHADER,
                    0,
                    &[],
                    &[],
                    &[barrier],
                );
            })
            .expect("failed to record command buffer");

        let submit_info = vk::SubmitInfo {
            wait_semaphores: &[],
            wait_stages: &[],
            command_buffers: &[&command_buffer],
            signal_semaphores: &[],
        };

        queue
            .submit(&[submit_info], None)
            .expect("failed to submit buffer copy command buffer");

        queue.wait_idle().expect("failed to wait on queue");

        Self {
            instance,
            #[cfg(debug_assertions)]
            debug_utils_messenger,
            surface,
            physical_device,
            device,
            queue,
            shaders,
            shader_mod_time,
            render_info,
            render_data,
            compute_data,
            command_pool,
            command_buffer,
            in_flight_fence,
            render_finished_semaphore,
            image_available_semaphore,
            last_batch,
            instance_buffer,
            instance_buffer_memory,
            data_buffer,
            data_buffer_memory,
            staging_buffer,
            staging_buffer_memory,
            ubo,
            cubelet_data,
            cubelet_data_memory,
            cubelet_data_view,
            cubelet_data_sampler,
            cubelet_sdf,
            cubelet_sdf_memory,
            cubelet_sdf_view,
            cubelet_sdf_sampler,
        }
    }
}

impl Renderer for Vulkan {
    fn draw_batch(&mut self, batch: Batch, entries: &'_ [Entry<'_>]) {
        self.device.wait_idle().expect("failed to wait on device");

        let mut vertex_count = 0;

        self.staging_buffer_memory
            .write(0, |data: &'_ mut [Vertex]| {
                for entry in entries {
                    let (vertices, _) = entry.mesh.get();

                    data[vertex_count..vertex_count + vertices.len()].copy_from_slice(&vertices);

                    vertex_count += vertices.len();
                }
            })
            .expect("failed to write to buffer");

        let mut index_count = 0;

        self.staging_buffer_memory
            .write(
                vertex_count * mem::size_of::<Vertex>(),
                |data: &'_ mut [u16]| {
                    for entry in entries {
                        let (_, indices) = entry.mesh.get();

                        data[index_count..index_count + indices.len()].copy_from_slice(&indices);

                        index_count += indices.len();
                    }
                },
            )
            .expect("failed to write to buffer");

        let ubo_offset =
            vertex_count * mem::size_of::<Vertex>() + index_count * mem::size_of::<u16>();

        let ubo_offset = ((ubo_offset as f64 / 64.0).ceil() * 64.0) as _;

        self.staging_buffer_memory
            .write(ubo_offset, |data: &'_ mut [UniformBufferObject]| {
                data[0..1].copy_from_slice(&[self.ubo]);
            })
            .expect("failed to write to buffer");

        self.command_buffer
            .record(|commands| {
                let buffer_copy = vk::BufferCopy {
                    src_offset: 0,
                    dst_offset: 0,
                    size: 32768,
                };

                commands.copy_buffer(&self.staging_buffer, &mut self.data_buffer, &[buffer_copy]);
            })
            .expect("failed to record command buffer");

        let submit_info = vk::SubmitInfo {
            wait_semaphores: &[],
            wait_stages: &[],
            command_buffers: &[&self.command_buffer],
            signal_semaphores: &[],
        };

        self.queue
            .submit(&[submit_info], None)
            .expect("failed to submit buffer copy command buffer");

        self.queue.wait_idle().expect("failed to wait on queue");

        let ct = 2 * self.ubo.render_distance as usize;
        let mut instance_data = vec![];

        for cx in 0..ct {
            for cy in 0..ct {
                for cz in 0..ct {
                    instance_data.push(Vector::<f32, 3>::new([cx as _, cy as _, cz as _]));
                }
            }
        }

        self.staging_buffer_memory
            .write(0, |data: &'_ mut [Vector<f32, 3>]| {
                data[..instance_data.len()].copy_from_slice(&instance_data[..]);
            })
            .expect("failed to write to buffer");

        self.command_buffer
            .record(|commands| {
                let buffer_copy = vk::BufferCopy {
                    src_offset: 0,
                    dst_offset: 0,
                    size: 32768,
                };

                commands.copy_buffer(
                    &self.staging_buffer,
                    &mut self.instance_buffer,
                    &[buffer_copy],
                );
            })
            .expect("failed to record command buffer");

        let submit_info = vk::SubmitInfo {
            wait_semaphores: &[],
            wait_stages: &[],
            command_buffers: &[&self.command_buffer],
            signal_semaphores: &[],
        };

        self.queue
            .submit(&[submit_info], None)
            .expect("failed to submit buffer copy command buffer");

        self.queue.wait_idle().expect("failed to wait on queue");

        {
            let base_path = "/home/brynn/dev/octane";
            let resources_path = format!("{}/{}/", base_path, "resources");
            let assets_path = format!("{}/{}/", base_path, "assets");

            for entry in fs::read_dir(resources_path).expect("failed to read directory") {
                let entry = entry.expect("failed to get directory entry");

                if entry
                    .file_type()
                    .expect("failed to get file type")
                    .is_file()
                {
                    let in_path = entry.path();

                    let out_path = format!(
                        "{}{}.spirv",
                        assets_path,
                        in_path.file_name().unwrap().to_string_lossy(),
                    );

                    let metadata = fs::metadata(&in_path);

                    if let Err(_) = metadata {
                        continue;
                    }

                    let mod_time = metadata
                        .unwrap()
                        .modified()
                        .expect("modified on unsupported platform");

                    let last_mod_time = *self
                        .shader_mod_time
                        .entry(out_path.clone())
                        .or_insert(time::SystemTime::now());

                    if mod_time != last_mod_time {
                        let shader_type = in_path.extension().and_then(|ext| {
                            match ext.to_string_lossy().as_ref() {
                                "vs" => Some(glsl_to_spirv::ShaderType::Vertex),
                                "fs" => Some(glsl_to_spirv::ShaderType::Fragment),
                                "cs" => Some(glsl_to_spirv::ShaderType::Compute),
                                _ => None,
                            }
                        });

                        if let None = shader_type {
                            continue;
                        }
                        dbg!(&shader_type);
                        let source =
                            fs::read_to_string(&in_path).expect("failed to read shader source");

                        info!("compiling shader...");

                        let compilation_result =
                            glsl_to_spirv::compile(&source, shader_type.unwrap());

                        if let Err(e) = compilation_result {
                            error!("failed to compile shader: {}", e);
                            self.shader_mod_time.insert(out_path.clone(), mod_time);
                            return;
                        }

                        let mut compilation = compilation_result.unwrap();

                        let mut compiled_bytes = vec![];

                        compilation
                            .read_to_end(&mut compiled_bytes)
                            .expect("failed to read compilation to buffer");

                        if fs::metadata(&assets_path).is_err() {
                            fs::create_dir("/home/brynn/dev/octane/assets/")
                                .expect("failed to create assets directory");
                        }

                        if fs::metadata(&out_path).is_ok() {
                            fs::remove_file(&out_path).expect("failed to remove file");
                        }

                        fs::write(&out_path, &compiled_bytes).expect("failed to write shader");

                        self.shader_mod_time.insert(out_path.clone(), mod_time);
                        self.shaders.remove(out_path.as_str());
                    }
                }
            }
        }

        let mut reload_graphics = false;
        let mut reload_compute = false;

        self.shaders.entry(batch.vertex_shader).or_insert_with(|| {
            info!("loading vertex shader");

            reload_graphics = true;

            let bytes = fs::read(batch.vertex_shader).unwrap();

            let code = convert_bytes_to_spirv_data(bytes);

            let shader_module_create_info = vk::ShaderModuleCreateInfo { code: &code[..] };

            let shader_module =
                vk::ShaderModule::new(self.device.clone(), shader_module_create_info)
                    .expect("failed to create shader module");

            shader_module
        });

        self.shaders
            .entry(batch.fragment_shader)
            .or_insert_with(|| {
                info!("loading fragment shader");

                reload_graphics = true;

                let bytes = fs::read(batch.fragment_shader).unwrap();

                let code = convert_bytes_to_spirv_data(bytes);

                let shader_module_create_info = vk::ShaderModuleCreateInfo { code: &code[..] };

                let shader_module =
                    vk::ShaderModule::new(self.device.clone(), shader_module_create_info)
                        .expect("failed to create shader module");

                shader_module
            });

        self.shaders.entry(batch.seed_shader).or_insert_with(|| {
            info!("loading seed compute shader");

            reload_compute = true;

            let bytes = fs::read(batch.seed_shader).unwrap();

            let code = convert_bytes_to_spirv_data(bytes);

            let shader_module_create_info = vk::ShaderModuleCreateInfo { code: &code[..] };

            let shader_module =
                vk::ShaderModule::new(self.device.clone(), shader_module_create_info)
                    .expect("failed to create shader module");

            shader_module
        });

        self.shaders.entry(batch.jfa_shader).or_insert_with(|| {
            info!("loading jfa compute shader");

            reload_compute = true;

            let bytes = fs::read(batch.jfa_shader).unwrap();

            let code = convert_bytes_to_spirv_data(bytes);

            let shader_module_create_info = vk::ShaderModuleCreateInfo { code: &code[..] };

            let shader_module =
                vk::ShaderModule::new(self.device.clone(), shader_module_create_info)
                    .expect("failed to create shader module");

            shader_module
        });

        if reload_graphics
            || self.last_batch.vertex_shader != batch.vertex_shader
            || self.last_batch.fragment_shader != batch.fragment_shader
        {
            self.device.wait_idle().expect("failed to wait on device");

            let shaders = [
                vk::PipelineShaderStageCreateInfo {
                    stage: vk::SHADER_STAGE_VERTEX,
                    module: &self.shaders[batch.vertex_shader],
                    entry_point: "main",
                },
                vk::PipelineShaderStageCreateInfo {
                    stage: vk::SHADER_STAGE_FRAGMENT,
                    module: &self.shaders[batch.fragment_shader],
                    entry_point: "main",
                },
            ];

            trace!("making new graphics pipeline...");

            let old_swapchain = self.render_data.take().map(|data| data.swapchain);

            self.render_data = Some(VulkanRenderData::init(
                self.device.clone(),
                &self.physical_device,
                &self.surface,
                &shaders,
                old_swapchain,
                &self.render_info,
            ));
        }

        if reload_compute || self.last_batch.jfa_shader != batch.jfa_shader {
            self.device.wait_idle().expect("failed to wait on device");

            let seed_shader = vk::PipelineShaderStageCreateInfo {
                stage: vk::SHADER_STAGE_COMPUTE,
                module: &self.shaders[batch.seed_shader],
                entry_point: "main",
            };

            let jfa_shader = vk::PipelineShaderStageCreateInfo {
                stage: vk::SHADER_STAGE_COMPUTE,
                module: &self.shaders[batch.jfa_shader],
                entry_point: "main",
            };

            trace!("making new compute pipelines...");

            self.compute_data = Some(VulkanComputeData::init(
                self.device.clone(),
                seed_shader,
                jfa_shader,
            ));
        }

        self.last_batch = batch;

        let render_data = self
            .render_data
            .as_mut()
            .expect("failed to retrieve render data");

        vk::Fence::wait(&[&mut self.in_flight_fence], true, u64::MAX)
            .expect("failed to wait for fence");

        vk::Fence::reset(&[&mut self.in_flight_fence]).expect("failed to reset fence");

        let image_index_result = render_data.swapchain.acquire_next_image(
            u64::MAX,
            Some(&mut self.image_available_semaphore),
            None,
        );

        let image_index = match image_index_result {
            Ok(i) => i,
            Err(e) => {
                warn!("failed to acquire next image: {:?}", e);
                return;
            }
        };

        for i in 0..render_data.descriptor_sets.len() {
            let uniform_buffer_info = vk::DescriptorBufferInfo {
                buffer: &self.data_buffer,
                offset: ubo_offset as _,
                range: mem::size_of::<UniformBufferObject>(),
            };

            let uniform_buffer_descriptor_write = vk::WriteDescriptorSet {
                dst_set: &render_data.descriptor_sets[image_index as usize],
                dst_binding: 0,
                dst_array_element: 0,
                descriptor_count: 1,
                descriptor_type: vk::DescriptorType::UniformBuffer,
                buffer_infos: &[uniform_buffer_info],
                image_infos: &[],
            };

            let cubelet_data_info = vk::DescriptorImageInfo {
                sampler: &self.cubelet_data_sampler,
                image_view: &self.cubelet_data_view,
                image_layout: vk::ImageLayout::ShaderReadOnly,
            };

            let cubelet_data_descriptor_write = vk::WriteDescriptorSet {
                dst_set: &render_data.descriptor_sets[image_index as usize],
                dst_binding: 1,
                dst_array_element: 0,
                descriptor_count: 1,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                buffer_infos: &[],
                image_infos: &[cubelet_data_info],
            };

            let cubelet_sdf_info = vk::DescriptorImageInfo {
                sampler: &self.cubelet_sdf_sampler,
                image_view: &self.cubelet_sdf_view,
                image_layout: vk::ImageLayout::ShaderReadOnly,
            };

            let cubelet_sdf_descriptor_write = vk::WriteDescriptorSet {
                dst_set: &render_data.descriptor_sets[image_index as usize],
                dst_binding: 2,
                dst_array_element: 0,
                descriptor_count: 1,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                buffer_infos: &[],
                image_infos: &[cubelet_sdf_info],
            };

            vk::DescriptorSet::update(
                &[
                    uniform_buffer_descriptor_write,
                    cubelet_data_descriptor_write,
                    cubelet_sdf_descriptor_write,
                ],
                &[],
            );
        }

        self.command_buffer
            .reset()
            .expect("failed to reset command buffer");

        self.command_buffer
            .record(|commands| {
                let render_pass_begin_info = vk::RenderPassBeginInfo {
                    render_pass: &render_data.render_pass,
                    framebuffer: &render_data.framebuffers[image_index as usize],
                    render_area: vk::Rect2d {
                        offset: (0, 0),
                        extent: self.render_info.extent,
                    },
                    color_clear_values: &[[0.0385, 0.0385, 0.0385, 1.0]],
                    depth_stencil_clear_value: Some((1.0, 0)),
                };

                commands.begin_render_pass(render_pass_begin_info);

                commands.bind_pipeline(
                    vk::PipelineBindPoint::Graphics,
                    &render_data.graphics_pipeline,
                );

                commands.bind_vertex_buffers(
                    0,
                    2,
                    &[&self.data_buffer, &self.instance_buffer],
                    &[0, 0],
                );

                commands.bind_index_buffer(
                    &self.data_buffer,
                    vertex_count * mem::size_of::<Vertex>(),
                    vk::IndexType::Uint16,
                );

                commands.bind_descriptor_sets(
                    vk::PipelineBindPoint::Graphics,
                    &render_data.graphics_pipeline_layout,
                    0,
                    &[&render_data.descriptor_sets[image_index as usize]],
                    &[],
                );

                let chunks = 2 * self.ubo.render_distance as u32;

                let volume = chunks * chunks * chunks;

                commands.draw_indexed(index_count as _, volume, 0, 0, 0);

                commands.end_render_pass();
            })
            .expect("failed to record command buffer");

        let submit_info = vk::SubmitInfo {
            wait_semaphores: &[&self.image_available_semaphore],
            wait_stages: &[vk::PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT],
            command_buffers: &[&self.command_buffer],
            signal_semaphores: &[&mut self.render_finished_semaphore],
        };

        self.queue
            .submit(&[submit_info], Some(&mut self.in_flight_fence))
            .expect("failed to submit draw command buffer");

        let present_info = vk::PresentInfo {
            wait_semaphores: &[&self.render_finished_semaphore],
            swapchains: &[&render_data.swapchain],
            image_indices: &[image_index],
        };

        let present_result = self.queue.present(present_info);

        match present_result {
            Ok(()) => {}
            Err(e) => warn!("failed to present: {:?}", e),
        }
    }

    fn resize(&mut self, resolution: (u32, u32)) {
        self.device.wait_idle().expect("failed to wait on device");

        let shaders = [
            vk::PipelineShaderStageCreateInfo {
                stage: vk::SHADER_STAGE_VERTEX,
                module: &self.shaders[self.last_batch.vertex_shader],
                entry_point: "main",
            },
            vk::PipelineShaderStageCreateInfo {
                stage: vk::SHADER_STAGE_FRAGMENT,
                module: &self.shaders[self.last_batch.fragment_shader],
                entry_point: "main",
            },
        ];

        self.render_info.extent = resolution;
        self.ubo.resolution = Vector::<f32, 2>::new([resolution.0 as _, resolution.1 as _]);

        let render_data = self.render_data.take().unwrap();

        let swapchain = render_data.swapchain;

        self.render_data = Some(VulkanRenderData::init(
            self.device.clone(),
            &self.physical_device,
            &self.surface,
            &shaders,
            Some(swapchain),
            &self.render_info,
        ));
    }
}

impl Drop for Vulkan {
    fn drop(&mut self) {
        self.device.wait_idle().expect("failed to wait on device");
    }
}
