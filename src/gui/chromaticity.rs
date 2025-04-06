use eframe::{
    egui, egui_glow,
    glow::{self, HasContext},
};
use egui::mutex::Mutex;
use std::sync::Arc;

pub struct ChromaticityWindow {
    /// Behind an `Arc<Mutex<…>>` so we can pass it to [`egui::PaintCallback`] and paint later.
    rotating_triangle: Arc<Mutex<RotatingTriangle>>,
    angle: f32,
}

impl ChromaticityWindow {
    pub fn new(gl: Arc<glow::Context>) -> Self {
        Self {
            rotating_triangle: Arc::new(Mutex::new(RotatingTriangle::new(gl))),
            angle: 0.0,
        }
    }

    pub fn update(&mut self, ctx: &egui::Context, show: &mut bool) {
        egui::Window::new("Chromaticity")
            .open(show)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label("The triangle is being painted using ");
                    ui.hyperlink_to("glow", "https://github.com/grovesNL/glow");
                    ui.label(" (OpenGL).");
                });

                egui::Frame::canvas(ui.style()).show(ui, |ui| {
                    self.custom_painting(ui);
                });
                ui.label("Drag to rotate!");
            });
    }

    fn custom_painting(&mut self, ui: &mut egui::Ui) {
        let (rect, response) =
            ui.allocate_exact_size(egui::Vec2::splat(300.0), egui::Sense::drag());

        self.angle += response.drag_motion().x * 0.01;

        // Clone locals so we can move them into the paint callback:
        let angle = self.angle;
        let rotating_triangle = self.rotating_triangle.clone();

        let callback = egui::PaintCallback {
            rect,
            callback: std::sync::Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                rotating_triangle.lock().paint(painter.gl(), angle);
            })),
        };
        ui.painter().add(callback);
    }
}

// struct ChromaticityDiagram {
//     gl: Arc<glow::Context>,
//     program: glow::NativeProgram,
//     vertex_array: glow::VertexArray,
// }

// impl ChromaticityDiagram {
//     fn new(gl: Arc<glow::Context>) -> Self {
//         let vertex_shader_source = "
//             #version 150

//             uniform mat4 matrix;
//             in vec2 position;
//             in vec3 color;
//             out vec3 vColor;

//             void main() {
//                 gl_Position = vec4(position, 0.0, 1.0) * matrix;
//                 vColor = color;
//             }
//         ";
//         let fragment_shader_source = "
//             #version 150

//             in vec3 vColor;
//             out vec4 f_color;

//             void main() {
//                 f_color = vec4(vColor, 1.0);
//             }
//         ";

//         let program =
//             unsafe { create_program(gl.as_ref(), vertex_shader_source, fragment_shader_source) };

//         let outline_vertex_buffer = VertexBuffer::new(
//             &context,
//             &Self::compute_chromaticity_diagram_outline_vertices(),
//         )
//         .unwrap();

//         let vertex_array = unsafe {
//             gl.create_vertex_array()
//                 .expect("Cannot create vertex array")
//         };

//         Self {
//             gl,
//             program,
//             vertex_array,
//         }
//     }

//     // fn compute_chromaticity_diagram_outline_vertices() -> Vec<Vertex> {
//     //     let cmf = &colorspace::cmf::CIE_1931_2_DEGREE;

//     //     let cmf_x = InterpolatorLinear::new(&cmf.x_bar);
//     //     let cmf_y = InterpolatorLinear::new(&cmf.y_bar);
//     //     let cmf_z = InterpolatorLinear::new(&cmf.z_bar);

//     //     let mut vertices = Vec::new();
//     //     for wavelength in 380..=700 {
//     //         let xyz = XYZf64 {
//     //             x: cmf_x.evaluate(wavelength as f64),
//     //             y: cmf_y.evaluate(wavelength as f64),
//     //             z: cmf_z.evaluate(wavelength as f64),
//     //         };
//     //         let xyy = XYYf64::from_xyz(xyz);

//     //         vertices.push(Vertex {
//     //             position: [xyy.x as f32, xyy.y as f32],
//     //             color: [0.0, 0.0, 0.0],
//     //         });
//     //     }
//     //     vertices
//     // }
// }

struct Vertex {
    position: [f32; 2],
}

unsafe fn create_vertex_buffer(
    gl: &glow::Context,
    vertices: &[Vertex],
) -> (glow::NativeBuffer, glow::NativeVertexArray) {
    let vertices_u8: &[u8] = unsafe {
        core::slice::from_raw_parts(
            vertices.as_ptr() as *const u8,
            vertices.len() * core::mem::size_of::<Vertex>(),
        )
    };

    // We construct a buffer and upload the data
    unsafe {
        let vbo = gl.create_buffer().unwrap();
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, vertices_u8, glow::STATIC_DRAW);

        // We now construct a vertex array to describe the format of the input buffer
        let vao = gl.create_vertex_array().unwrap();
        gl.bind_vertex_array(Some(vao));
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
        (vbo, vao)
    }
}

unsafe fn create_program(
    gl: &glow::Context,
    vertex_shader_source: &str,
    fragment_shader_source: &str,
) -> glow::NativeProgram {
    unsafe {
        let program = gl.create_program().expect("Cannot create shader program");

        let shader_sources = [
            (glow::VERTEX_SHADER, vertex_shader_source),
            (glow::FRAGMENT_SHADER, fragment_shader_source),
        ];

        let mut shaders = Vec::with_capacity(shader_sources.len());

        for (shader_type, shader_source) in shader_sources.iter() {
            let shader = gl
                .create_shader(*shader_type)
                .expect("Cannot create shader");
            gl.shader_source(shader, shader_source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                panic!("{}", gl.get_shader_info_log(shader));
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }

        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            panic!("{}", gl.get_program_info_log(program));
        }

        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }

        program
    }
}

struct RotatingTriangle {
    gl: Arc<glow::Context>,
    program: glow::Program,
    vertex_buffer: glow::Buffer,
    vertex_array: glow::VertexArray,
}

impl RotatingTriangle {
    fn new(gl: Arc<glow::Context>) -> Self {
        let (vertex_shader_source, fragment_shader_source) = (
            r#"
                #version 330

                in vec2 in_position;
                const vec4 colors[3] = vec4[3](
                    vec4(1.0, 0.0, 0.0, 1.0),
                    vec4(0.0, 1.0, 0.0, 1.0),
                    vec4(0.0, 0.0, 1.0, 1.0)
                );
                out vec4 v_color;
                uniform float u_angle;

                void main() {
                    v_color = colors[gl_VertexID];
                    gl_Position = vec4(in_position, 0.0, 1.0);
                    gl_Position.x *= cos(u_angle);
                }
            "#,
            r#"
                #version 330
                precision mediump float;
                in vec4 v_color;
                out vec4 out_color;
                void main() {
                    out_color = v_color;
                }
            "#,
        );

        let program = unsafe { create_program(&gl, vertex_shader_source, fragment_shader_source) };

        let vertices = [
            Vertex {
                position: [0.0, 1.0],
            },
            Vertex {
                position: [-1.0, -1.0],
            },
            Vertex {
                position: [1.0, -1.0],
            },
        ];
        // let vertex_array = unsafe { gl.create_vertex_array() }.expect("Cannot create vertex array");
        let (vertex_buffer, vertex_array) = unsafe { create_vertex_buffer(&gl, &vertices) };

        Self {
            gl,
            program,
            vertex_buffer,
            vertex_array,
        }
    }

    fn paint(&self, gl: &glow::Context, angle: f32) {
        use glow::HasContext as _;
        unsafe {
            gl.use_program(Some(self.program));
            gl.uniform_1_f32(
                gl.get_uniform_location(self.program, "u_angle").as_ref(),
                angle,
            );
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vertex_buffer));
            gl.bind_vertex_array(Some(self.vertex_array));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }
    }
}

impl Drop for RotatingTriangle {
    fn drop(&mut self) {
        use glow::HasContext as _;
        unsafe {
            self.gl.delete_program(self.program);
            self.gl.delete_vertex_array(self.vertex_array);
        };
    }
}
