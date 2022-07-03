mod window;

mod term {
    pub const RESET: &str = "\x1b[1;0m";
    pub const BOLDMAGENTA: &str = "\x1b[1;35m";
    pub const BOLDCYAN: &str = "\x1b[1;36m";
    pub const BOLDYELLOW: &str = "\x1b[1;33m";
    pub const BOLDRED: &str = "\x1b[1;31m";
}

use crate::window::{Event as WindowEvent, Keycode, Window};

use common::mesh::Mesh;
use common::render::{self, Renderer};

use math::prelude::{Matrix, Vector};

use std::collections::HashMap;
use std::fs;
use std::mem;
use std::path::Path;

use log::{error, info, trace, warn};

struct Logger;

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Trace
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            println!(
                "{}{}{}: {}",
                match record.level() {
                    log::Level::Trace => term::BOLDMAGENTA,
                    log::Level::Info => term::BOLDCYAN,
                    log::Level::Warn => term::BOLDYELLOW,
                    log::Level::Error => term::BOLDRED,
                    _ => term::RESET,
                },
                record.level().as_str().to_lowercase(),
                term::RESET,
                record.args()
            );
        }
    }
    fn flush(&self) {}
}

static LOGGER: Logger = Logger;

//TODO identify why release segfaults
fn main() {
    println!("Hello, world!");

    log::set_max_level(log::LevelFilter::Info);
    log::set_logger(&LOGGER).expect("failed to set logger");

    let mut window = Window::new();

    window.rename("Octane");
    window.show();

    let mut projection = Matrix::<f32, 4, 4>::identity();
    let mut camera = Matrix::<f32, 4, 4>::identity();
    let mut model = Matrix::<f32, 4, 4>::identity();

    //create matrices
    {
        let fov = 45.0_f32 * 2.0 * std::f32::consts::PI / 360.0;

        let focal_length = 1.0 / (fov / 2.0).tan();

        let aspect_ratio = (960) as f32 / (540) as f32;

        let near = 0.01;
        let far = 1000.0;

        projection[0][0] = focal_length / aspect_ratio;
        projection[1][1] = -focal_length;
        projection[2][2] = far / (near - far);
        projection[2][3] = -1.0;
        projection[3][2] = (near * far) / (near - far);
    }

    let mut vulkan = render::Vulkan::init(&window);

    vulkan.ubo.proj = projection;
    vulkan.ubo.view = camera.inverse();
    vulkan.ubo.model = model;

    let vertex_shader = "/home/brynn/dev/octane/assets/default.vs.spirv";
    let fragment_shader = "/home/brynn/dev/octane/assets/default.fs.spirv";

    let cube_obj =
        fs::File::open("/home/brynn/dev/octane/assets/cube.obj").expect("failed to open obj");

    let mut cube = Mesh::from_obj(cube_obj);

    let batch = render::Batch {
        vertex_shader: &vertex_shader,
        fragment_shader: &fragment_shader,
    };

    let entries = [render::Entry { mesh: &cube }];

    let startup = std::time::Instant::now();
    let mut last = startup;

    let mut keys = HashMap::new();

    let mut x_rot = 0.0;
    let mut y_rot = 0.0;
    let mut position = Vector::<f32, 4>::new([0.0, 0.0, 10.0, 1.0]);
    let mut should_capture = false;

    'main: loop {
        let current = std::time::Instant::now();
        let delta_time = current.duration_since(last).as_secs_f32();
        last = current;

        window.rename(format!("Octane {}", 1.0 / delta_time).as_str());
        if should_capture {
            window.capture();
        }
        window.show_cursor(!should_capture);

        //TODO must be done soon.. tired of this convoluted movement code.
        //simplify movement code
        let sens = 1000.0;

        while let Some(event) = window.next_event() {
            match event {
                WindowEvent::KeyPress { keycode } => {
                    keys.insert(keycode, current);
                }
                WindowEvent::KeyRelease { keycode } => {
                    keys.remove(&keycode);
                }
                WindowEvent::PointerMotion { x, y } => {
                    if should_capture {
                        x_rot -= (x as f32 - window.resolution().0 as f32 / 2.0) / sens;
                        y_rot -= (y as f32 - window.resolution().1 as f32 / 2.0) / sens;
                    }
                }
                WindowEvent::CloseRequested => {
                    break 'main;
                }
                WindowEvent::Resized { resolution } => {
                    vulkan.resize(resolution);
                }
            }
        }

        let movement_speed = 5.612;

        let mut camera = Matrix::<f32, 4, 4>::identity();

        let mut x_r = Matrix::<f32, 4, 4>::identity();
        let mut y_r = Matrix::<f32, 4, 4>::identity();

        x_r[0][0] = x_rot.cos();
        x_r[2][0] = x_rot.sin();
        x_r[0][2] = -x_rot.sin();
        x_r[2][2] = x_rot.cos();

        y_rot = y_rot.clamp(
            -std::f32::consts::PI / 2.0 + 0.1,
            std::f32::consts::PI / 2.0 - 0.1,
        );

        y_r[1][1] = y_rot.cos();
        y_r[2][1] = -y_rot.sin();
        y_r[1][2] = y_rot.sin();
        y_r[2][2] = y_rot.cos();

        camera = camera * y_r;
        camera = camera * x_r;

        let mut m = Matrix::<f32, 4, 4>::identity();

        for (key, &time) in &keys {
            match key {
                Keycode::W => {
                    m[3][2] += -1.0;
                }
                Keycode::A => {
                    m[3][0] += -1.0;
                }
                Keycode::S => {
                    m[3][2] += 1.0;
                }
                Keycode::D => {
                    m[3][0] += 1.0;
                }
                Keycode::Space => {
                    position[1] += movement_speed * delta_time;
                }
                Keycode::LeftShift => {
                    position[1] -= movement_speed * delta_time;
                }
                Keycode::Escape => {
                    if time == current {
                        should_capture = !should_capture;
                    }
                }
            }
        }

        let l = m * y_r;
        let l = l * x_r;
        let mut p = Vector::<f32, 4>::new(l[3]);
        p[1] = 0.0;
        p[3] = 0.0;
        let p = if p.magnitude() > 0.0 {
            p.normalize()
        } else {
            p
        };
        position[0] += p[0] * movement_speed * delta_time;
        position[2] += p[2] * movement_speed * delta_time;

        camera[3][0] = position[0];
        camera[3][1] = position[1];
        camera[3][2] = position[2];

        let follow = 2.0 * 1.0 as f32;
        let angle = std::time::Instant::now()
            .duration_since(startup)
            .as_secs_f32()
            % (2.0 * std::f32::consts::PI);

        vulkan.ubo.model[0][0] = angle.cos();
        vulkan.ubo.model[2][0] = angle.sin();
        vulkan.ubo.model[0][2] = -angle.sin();
        vulkan.ubo.model[2][2] = angle.cos();

        vulkan.ubo.view = camera.inverse();

        vulkan.draw_batch(batch.clone(), &entries);
    }

    //TODO figure out surface dependency on window
    //window is dropped before surface which causes segfault
    //explicit drop fixes this but it is not ideal

    drop(vulkan);
    drop(window);
    //vk shutdown happens during implicit Drop.
    //Rc ensures shutdown happens in right order.
}
