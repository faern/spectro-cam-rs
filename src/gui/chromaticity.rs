//! This module implements drawing an egui window containing a [Chromaticity diagram]
//! using OpenGL. Since egui supports glow natively that's the OpenGL bindings we'll use.
//!
//! To understand OpenGL concepts such as "Vertex Array Objects" (VAOs) and
//! "Vertex Buffer Objects" (VBOs) etc, this tutorial is very helpful:
//! https://antongerdelan.net/opengl/hellotriangle.html
//!
//! [Chromaticity diagram]: https://en.wikipedia.org/wiki/CIE_1931_color_space#Chromaticity_diagram
//!
//!

use colorimetry::{data::illuminants::D65, observer::Observer, xyz::XYZ};
use eframe::{
    egui, egui_glow,
    glow::{self, HasContext},
};
use egui::mutex::Mutex;
use nalgebra::Vector2;
use std::sync::Arc;

pub struct ChromaticityWindow {
    /// Behind an `Arc<Mutex<…>>` so we can pass it to [`egui::PaintCallback`] and paint later.
    rotating_triangle: Arc<Mutex<ChromaticityDiagram>>,
}

impl ChromaticityWindow {
    pub fn new(gl: Arc<glow::Context>) -> Self {
        Self {
            rotating_triangle: Arc::new(Mutex::new(ChromaticityDiagram::new(gl))),
        }
    }

    pub fn update(&mut self, ctx: &egui::Context, show: &mut bool) {
        egui::Window::new("Chromaticity")
            .open(show)
            .show(ctx, |ui| {
                egui::Frame::canvas(ui.style()).show(ui, |ui| {
                    ui.set_min_size(egui::Vec2::splat(300.0));
                    self.custom_painting(ui);
                });
            });
    }

    fn custom_painting(&mut self, ui: &mut egui::Ui) {
        let (rect, _response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::drag());

        // Clone locals so we can move them into the paint callback:
        let rotating_triangle = self.rotating_triangle.clone();

        let callback = egui::PaintCallback {
            rect,
            callback: std::sync::Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                rotating_triangle.lock().paint(painter.gl());
            })),
        };
        ui.painter().add(callback);
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 3],
}

/// Returns a vertex array object (VAO) representing the given vertices.
///
/// This first allocates a vertex buffer object (VBO) and uploads the vertex data to the GPU.
/// Then it creates a vertex array object (VAO) and binds the VBO to it with VAO attribute
/// zero pointing to the position data and VAO attribute one pointing to the color data.
unsafe fn create_vertex_buffer(
    gl: &glow::Context,
    vertices: &[Vertex],
    position_attribute_index: u32,
    color_attribute_index: u32,
) -> glow::NativeVertexArray {
    const BYTES_PER_VERTEX: usize = core::mem::size_of::<Vertex>();

    // These need to match the layout of the Vertex struct (number of floats in each field)
    const NUM_POSITION_COMPONENTS: i32 = 2;
    const NUM_COLOR_COMPONENTS: i32 = 3;

    // We need a raw byte slice over the vertex data for uploading to the GPU
    let vertices_u8: &[u8] = unsafe {
        core::slice::from_raw_parts(
            vertices.as_ptr() as *const u8,
            vertices.len() * BYTES_PER_VERTEX,
        )
    };

    unsafe {
        // We construct a buffer and upload the data
        let vertex_buffer = gl.create_buffer().unwrap();
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, vertices_u8, glow::STATIC_DRAW);

        // We now construct a vertex array to describe the format of the buffer
        let vertex_array = gl.create_vertex_array().unwrap();
        gl.bind_vertex_array(Some(vertex_array));

        // Point attribute zero to the position data in the buffer
        gl.vertex_attrib_pointer_f32(
            position_attribute_index,
            NUM_POSITION_COMPONENTS,
            glow::FLOAT,
            false,
            BYTES_PER_VERTEX as i32,
            core::mem::offset_of!(Vertex, position) as i32,
        );
        // Activate the position attribute. By default all attributes are disabled
        gl.enable_vertex_attrib_array(position_attribute_index);

        // Point attribute one to the color data in the buffer
        gl.vertex_attrib_pointer_f32(
            color_attribute_index,
            NUM_COLOR_COMPONENTS,
            glow::FLOAT,
            false,
            BYTES_PER_VERTEX as i32,
            core::mem::offset_of!(Vertex, color) as i32,
        );
        // Activate the color attribute. By default all attributes are disabled
        gl.enable_vertex_attrib_array(color_attribute_index);
        vertex_array
    }
}

fn create_index_buffer(gl: &glow::Context, indices: &[u32]) -> glow::NativeBuffer {
    const BYTES_PER_INDEX: usize = core::mem::size_of::<u32>();

    let indices_u8: &[u8] = unsafe {
        core::slice::from_raw_parts(
            indices.as_ptr() as *const u8,
            indices.len() * BYTES_PER_INDEX,
        )
    };
    unsafe {
        let index_buffer = gl.create_buffer().unwrap();
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index_buffer));
        gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, indices_u8, glow::STATIC_DRAW);
        index_buffer
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

struct ChromaticityDiagram {
    gl: Arc<glow::Context>,
    program: glow::Program,
    outline_vertex_array: glow::VertexArray,
    chromaticity_diagram_vertex_array: glow::VertexArray,
    chromaticity_diagram_index_buffer: glow::NativeBuffer,
    chromaticity_diagram_num_elements: usize,
}

impl ChromaticityDiagram {
    fn new(gl: Arc<glow::Context>) -> Self {
        let (vertex_shader_source, fragment_shader_source) = (
            r#"
                #version 330

                // uniform float u_angle;

                in vec2 position;
                in vec3 color;

                out vec4 v_color;

                void main() {
                    gl_Position = vec4(position, 0.0, 1.0);
                    gl_Position.x = gl_Position.x * 2.2 - 0.8;
                    gl_Position.y = gl_Position.y * 2.2 - 0.9;
                    v_color = vec4(color, 1.0);
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

        // Get the attribute indexes for our two shader input parameters.
        // These are needed to bind the corresponding vertex buffers to the matching
        // vertex array attributes below.
        let position_attribute_index =
            unsafe { gl.get_attrib_location(program, "position") }.unwrap();
        let color_attribute_index = unsafe { gl.get_attrib_location(program, "color") }.unwrap();

        let observer = colorimetry::observer::Observer::Std1931;
        let outline_vertices = Self::compute_chromaticity_diagram_outline_vertices(observer);

        let outline_vertex_array = unsafe {
            create_vertex_buffer(
                &gl,
                &outline_vertices,
                position_attribute_index,
                color_attribute_index,
            )
        };

        let chromaticity_diagram_outline_positions = Self::chromaticity_diagram_outline_positions();
        let [center_x, center_y] = D65.xyz(Some(observer)).chromaticity();
        let center = Vector2::new(center_x, center_y);
        let (chromaticity_diagram_vertices, chromaticity_diagram_indices) =
            Self::compute_gl_triangle_strip_from_ring(
                &chromaticity_diagram_outline_positions,
                center,
                observer,
            );

        println!(
            "NUMBER OF VERTICE INDICES: {}",
            chromaticity_diagram_indices.len()
        );

        let chromaticity_diagram_vertex_array = unsafe {
            create_vertex_buffer(
                &gl,
                &chromaticity_diagram_vertices,
                position_attribute_index,
                color_attribute_index,
            )
        };
        let chromaticity_diagram_index_buffer =
            create_index_buffer(&gl, &chromaticity_diagram_indices);

        Self {
            gl,
            program,
            outline_vertex_array,
            chromaticity_diagram_vertex_array,
            chromaticity_diagram_index_buffer,
            chromaticity_diagram_num_elements: chromaticity_diagram_indices.len(),
        }
    }

    fn paint(&self, gl: &glow::Context) {
        use glow::HasContext as _;
        unsafe {
            gl.clear_color(0.1, 0.1, 0.1, 1.0);
            gl.clear(glow::DEPTH_BUFFER_BIT | glow::COLOR_BUFFER_BIT);

            gl.use_program(Some(self.program));
            // gl.bind_vertex_array(Some(self.outline_vertex_array));
            // gl.draw_arrays(glow::LINE_LOOP, 0, 320);

            gl.bind_vertex_array(Some(self.chromaticity_diagram_vertex_array));
            gl.bind_buffer(
                glow::ELEMENT_ARRAY_BUFFER,
                Some(self.chromaticity_diagram_index_buffer),
            );
            gl.draw_elements(
                glow::TRIANGLE_STRIP,
                self.chromaticity_diagram_num_elements as i32,
                glow::UNSIGNED_INT,
                0,
            );
        }
    }

    fn compute_chromaticity_diagram_outline_vertices(observer: Observer) -> Vec<Vertex> {
        let planckian_locus_min_wavelength = observer.data().spectral_locus_nm_min();
        let planckian_locus_max_wavelength = observer.data().spectral_locus_nm_max();

        let mut vertices = Vec::new();
        for wavelength in planckian_locus_min_wavelength..=planckian_locus_max_wavelength {
            // Compute tristimulus values for the monochromatic spectrum
            let xyz = observer.data().spectral_locus_by_nm(wavelength).unwrap();
            // Compute the chromaticity coordinates
            let [x, y] = xyz.chromaticity();

            vertices.push(Vertex {
                position: [x as f32, y as f32],
                color: [1.0, 1.0, 1.0],
            });
        }
        vertices
    }

    fn chromaticity_diagram_outline_positions() -> Vec<Vector2<f64>> {
        const BOTTOM_EDGE_RESOLUTION: u16 = 100;

        let observer = colorimetry::observer::Observer::Std1931;
        let planckian_locus_min_wavelength = observer.data().spectral_locus_nm_min();
        let planckian_locus_max_wavelength = observer.data().spectral_locus_nm_max();

        let mut outer_edge_vertexes = Vec::new();
        for wavelength in planckian_locus_min_wavelength..=planckian_locus_max_wavelength {
            // Compute tristimulus values for the monochromatic spectrum
            let xyz = observer.data().spectral_locus_by_nm(wavelength).unwrap();
            // Compute the chromaticity coordinates
            let [x, y] = xyz.chromaticity();

            outer_edge_vertexes.push(Vector2::new(x, y));
        }
        let bottom_edge_start = *outer_edge_vertexes.last().unwrap();
        let bottom_edge_end = *outer_edge_vertexes.first().unwrap();
        let bottom_edge_diff = bottom_edge_end - bottom_edge_start;
        for i in 1..BOTTOM_EDGE_RESOLUTION {
            let ratio = i as f64 / BOTTOM_EDGE_RESOLUTION as f64;
            let bottom_edge_vector = bottom_edge_start + bottom_edge_diff * ratio;
            outer_edge_vertexes.push(bottom_edge_vector);
        }
        outer_edge_vertexes
    }

    fn compute_gl_triangle_strip_from_ring(
        outer_ring: &[Vector2<f64>],
        center: Vector2<f64>,
        observer: Observer,
    ) -> (Vec<Vertex>, Vec<u32>) {
        const STEPS_TO_CENTER: u32 = 50;

        let items_per_ring = u32::try_from(outer_ring.len()).unwrap();
        // The index in the `vertices` vector that the last vertex (the center point)
        // will have. It's added last, after the loop below.
        let index_of_center_vertex = STEPS_TO_CENTER * items_per_ring;

        let mut vertices = Vec::new();
        // The list of indices into `vertices` to draw the triangle strip from.
        // Starts out with with index of the last vertice in the outermost ring (
        // will be added at the last step the first time the inner for loop runs)
        let mut indices = vec![items_per_ring - 1];
        for ring_i in 0..STEPS_TO_CENTER {
            let center_ratio = ring_i as f64 / STEPS_TO_CENTER as f64;
            for (i, ring_vertex) in outer_ring.iter().copied().enumerate() {
                let vertex = ring_vertex + (center - ring_vertex) * center_ratio;

                vertices.push(Self::chromaticity_to_vertex(vertex, observer));

                let i = u32::try_from(i).unwrap();
                // Draw a triangle to the vertice we just pushed above
                indices.push(ring_i * items_per_ring + i);
                // Draw a triangle to the corresponding vertice in the next ring, or the
                // center point if we are on the last ring
                indices.push(if ring_i < STEPS_TO_CENTER - 1 {
                    (ring_i + 1) * items_per_ring + i
                } else {
                    index_of_center_vertex
                });
            }
        }
        vertices.push(Self::chromaticity_to_vertex(center, observer));

        (vertices, indices)
    }

    fn chromaticity_to_vertex(chromaticity: Vector2<f64>, observer: Observer) -> Vertex {
        let color = Self::xy_to_rgb(chromaticity, observer);
        Vertex {
            position: [chromaticity.x as f32, chromaticity.y as f32],
            color,
        }
    }

    /// Converts xy chromaticity coordinates to RGB values.
    fn xy_to_rgb(chromaticity: Vector2<f64>, observer: Observer) -> [f32; 3] {
        let colorspace = colorimetry::rgbspace::RgbSpace::SRGB;
        let xyz = XYZ::try_from_chromaticity(chromaticity.x, chromaticity.y, None, Some(observer))
            .unwrap();
        let rgb = xyz.rgb(Some(colorspace));
        // let rgb_vec_f64: Vector3<f64> = *rgb.as_ref();
        // let rgb_vec_f32 = rgb_vec_f64.cast::<f32>();
        // let rgb_array = <Vector3<f32> as AsRef<[f32; 3]>>::as_ref(&rgb_vec_f32);
        rgb.values().map(|v| v as f32)
    }
}

impl Drop for ChromaticityDiagram {
    fn drop(&mut self) {
        use glow::HasContext as _;
        unsafe {
            self.gl.delete_program(self.program);
            self.gl.delete_vertex_array(self.outline_vertex_array);
            self.gl
                .delete_vertex_array(self.chromaticity_diagram_vertex_array);
            self.gl
                .delete_buffer(self.chromaticity_diagram_index_buffer);
        };
    }
}
