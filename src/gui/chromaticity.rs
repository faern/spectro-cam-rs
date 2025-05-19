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

use crate::config::SpectrumPoint;

use super::opengl_helpers::{
    Vertex, VertexArrayWithBuffer, VertexIndexBuffer, create_index_buffer, create_program,
    create_vertex_buffer,
};
use colorimetry::illuminant::Illuminant;
use colorimetry::observer::Observer;
use colorimetry::rgbspace::RgbSpace;
use colorimetry::traits::Filter;
use colorimetry::widergb::WideRgb;
use colorimetry::xyz::{Chromaticity, XYZ};
use colorimetry::{data::illuminants::D65, traits::Light};
use eframe::{
    egui, egui_glow,
    glow::{self, HasContext},
};
use egui::mutex::Mutex;
use nalgebra::Vector2;
use std::sync::Arc;

pub struct ChromaticityWindow {
    /// Behind an `Arc<Mutex<…>>` so we can pass it to [`egui::PaintCallback`] and paint later.
    chromaticity_diagram: Arc<Mutex<ChromaticityDiagram>>,
    observer: Observer,
    colorspace: RgbSpace,
}

impl ChromaticityWindow {
    pub fn new(gl: Arc<glow::Context>) -> Self {
        let observer = Observer::Std1931;
        let colorspace = RgbSpace::SRGB;
        Self {
            chromaticity_diagram: Arc::new(Mutex::new(ChromaticityDiagram::new(
                gl, observer, colorspace,
            ))),
            observer,
            colorspace,
        }
    }

    pub fn update(&mut self, ctx: &egui::Context, show: &mut bool, spectrum: &[SpectrumPoint]) {
        let spectrum = Illuminant::new(Self::parse_spectrum(spectrum));
        // let spectrum = Illuminant::planckian(2700.0);
        let xyz = spectrum.xyz(Some(self.observer));
        let [x, y, z] = xyz.values();
        let chromaticity = xyz.chromaticity();
        let cri = spectrum.cri().map(|cri| cri.ra()).unwrap_or(f64::NAN);
        let (kelvin, tint) = spectrum
            .cct()
            .map(|cct| (cct.t(), cct.tint()))
            .unwrap_or((f64::NAN, f64::NAN));

        egui::Window::new("Chromaticity")
            .open(show)
            .show(ctx, |ui| {
                egui::Frame::canvas(ui.style()).show(ui, |ui| {
                    // ui.set_min_size(egui::Vec2::splat(300.0));
                    self.custom_painting(ui, chromaticity);
                });

                ui.columns_const(|[col_1, col_2]| {
                    col_1.label("Tristimulus values: ");
                    col_1.label("Chromaticity coordinates: ");
                    col_1.label("CRI: ");
                    col_1.label("CCT: ");
                    col_2.horizontal(|ui| {
                        ui.label("X: ");
                        ui.monospace(format!("{:.3}", x));
                        ui.label("Y: ");
                        ui.monospace(format!("{:.3}", y));
                        ui.label("Z: ");
                        ui.monospace(format!("{:.3}", z));
                    });
                    col_2.horizontal(|ui| {
                        ui.label("x: ");
                        ui.monospace(format!("{:.3}", chromaticity.x()));
                        ui.label("y: ");
                        ui.monospace(format!("{:.3}", chromaticity.y()));
                    });
                    col_2.horizontal(|ui| {
                        ui.label("Ra: ");
                        ui.monospace(format!("{:.3}", cri));
                    });
                    col_2.horizontal(|ui| {
                        ui.label("Temp: ");
                        ui.monospace(format!("{:.3}K", kelvin));
                        ui.label("Tint: ");
                        ui.monospace(format!("{:.3}", tint));
                    });
                });
            });
    }

    fn custom_painting(&mut self, ui: &mut egui::Ui, chromaticity: Chromaticity) {
        // let (rect, _response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::drag());
        let (rect, _response) =
            ui.allocate_exact_size(egui::Vec2::splat(300.0), egui::Sense::drag());

        // Clone locals so we can move them into the paint callback:
        let chromaticity_diagram = self.chromaticity_diagram.clone();

        let callback = egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                chromaticity_diagram
                    .lock()
                    .paint(painter.gl(), chromaticity);
            })),
        };
        ui.painter().add(callback);
    }

    fn parse_spectrum(spectrum: &[SpectrumPoint]) -> colorimetry::spectrum::Spectrum {
        let (wavelengths, values): (Vec<f64>, Vec<f64>) = spectrum
            .iter()
            .map(|s| (f64::from(s.wavelength), f64::from(s.value)))
            .unzip();
        dbg!((wavelengths.len(), values.len()));
        if wavelengths.len() < 2 {
            return colorimetry::spectrum::Spectrum::default();
        }
        let spectrum =
            colorimetry::spectrum::Spectrum::linear_interpolate(&wavelengths, &values).unwrap();
        spectrum
    }
}

struct ShaderProgram {
    gl: Arc<glow::Context>,
    program: glow::Program,
    offset_uniform_location: glow::UniformLocation,
    position_attribute_index: u32,
    color_attribute_index: u32,
}

impl ShaderProgram {
    fn new(gl: Arc<glow::Context>) -> Self {
        let (vertex_shader_source, fragment_shader_source) = (
            r#"
                #version 330

                uniform vec2 offset;

                in vec2 position;
                in vec3 color;

                out vec4 v_color;

                void main() {
                    gl_Position = vec4(position + offset, 0.0, 1.0);
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

        let program = create_program(&gl, vertex_shader_source, fragment_shader_source);

        // Get the attribute indexes for our shader input parameters.
        // These are needed to bind the corresponding vertex buffers to the matching
        // vertex array attributes below.
        let offset_uniform_location =
            dbg!(unsafe { gl.get_uniform_location(program, "offset") }.unwrap());
        let position_attribute_index =
            dbg!(unsafe { gl.get_attrib_location(program, "position") }.unwrap());
        let color_attribute_index =
            dbg!(unsafe { gl.get_attrib_location(program, "color") }.unwrap());

        Self {
            gl,
            program,
            offset_uniform_location,
            position_attribute_index,
            color_attribute_index,
        }
    }

    fn set_offset(&self, gl: &glow::Context, x: f32, y: f32) {
        use glow::HasContext as _;
        unsafe {
            gl.uniform_2_f32(Some(&self.offset_uniform_location), x, y);
        }
    }
}

impl Drop for ShaderProgram {
    fn drop(&mut self) {
        use glow::HasContext as _;
        unsafe {
            self.gl.delete_program(self.program);
        }
    }
}

struct ChromaticityDiagram {
    gl: Arc<glow::Context>,
    program: ShaderProgram,
    outline_vertex_buffer: VertexArrayWithBuffer,
    chromaticity_diagram_vertex_buffer: VertexArrayWithBuffer,
    chromaticity_diagram_index_buffer: VertexIndexBuffer,
    rgb_gamut_vertex_array: VertexArrayWithBuffer,
    cross_vertices: VertexArrayWithBuffer,
}

impl ChromaticityDiagram {
    fn new(gl: Arc<glow::Context>, observer: Observer, colorspace: RgbSpace) -> Self {
        let program = ShaderProgram::new(gl.clone());

        let outline_vertices = Self::compute_chromaticity_diagram_outline_vertices(observer);

        let outline_vertex_buffer = create_vertex_buffer(
            gl.clone(),
            &outline_vertices,
            program.position_attribute_index,
            program.color_attribute_index,
        );

        let chromaticity_diagram_outline_positions =
            Self::chromaticity_diagram_outline_positions(observer);
        let center = D65.xyz(Some(observer)).chromaticity().to_vec();
        let (chromaticity_diagram_vertices, chromaticity_diagram_indices) =
            Self::compute_gl_triangle_strip_from_ring(
                &chromaticity_diagram_outline_positions,
                center,
                observer,
                colorspace,
            );

        println!(
            "NUMBER OF VERTICE INDICES: {}",
            chromaticity_diagram_indices.len()
        );

        let chromaticity_diagram_vertex_buffer = create_vertex_buffer(
            gl.clone(),
            &chromaticity_diagram_vertices,
            program.position_attribute_index,
            program.color_attribute_index,
        );
        let chromaticity_diagram_index_buffer =
            create_index_buffer(gl.clone(), &chromaticity_diagram_indices);

        let rgb_gamut_vertex_array = create_vertex_buffer(
            gl.clone(),
            &Self::rgb_gamut_vertices(colorspace, observer),
            program.position_attribute_index,
            program.color_attribute_index,
        );

        let cross_vertices = create_vertex_buffer(
            gl.clone(),
            &Self::cross_vertices_at(0.0, 0.0),
            program.position_attribute_index,
            program.color_attribute_index,
        );

        Self {
            gl,
            program,
            outline_vertex_buffer,
            chromaticity_diagram_vertex_buffer,
            chromaticity_diagram_index_buffer,
            rgb_gamut_vertex_array,
            cross_vertices,
        }
    }

    fn paint(&self, gl: &glow::Context, chromaticity: Chromaticity) {
        use glow::HasContext as _;
        unsafe {
            gl.use_program(Some(self.program.program));

            self.program.set_offset(gl, 0.0, 0.0);

            // Draw the chromaticity diagram "background"
            gl.bind_vertex_array(Some(self.chromaticity_diagram_vertex_buffer.vertex_array()));
            gl.bind_buffer(
                glow::ELEMENT_ARRAY_BUFFER,
                Some(self.chromaticity_diagram_index_buffer.index_buffer()),
            );
            gl.draw_elements(
                glow::TRIANGLE_STRIP,
                self.chromaticity_diagram_index_buffer.len(),
                glow::UNSIGNED_INT,
                0,
            );

            // Draw the outline of the chromaticity diagram
            gl.bind_vertex_array(Some(self.outline_vertex_buffer.vertex_array()));
            gl.draw_arrays(glow::LINE_LOOP, 0, self.outline_vertex_buffer.len());

            // Draw the gamut triangle of the selected RGB space
            gl.bind_vertex_array(Some(self.rgb_gamut_vertex_array.vertex_array()));
            gl.line_width(1.5);
            gl.draw_arrays(glow::LINE_LOOP, 0, 3);

            // Draw a cross at the measured spectrum position
            self.program
                .set_offset(gl, chromaticity.x() as f32, chromaticity.y() as f32);
            gl.bind_vertex_array(Some(self.cross_vertices.vertex_array()));
            gl.line_width(2.0);
            gl.draw_arrays(glow::LINES, 0, 8);
        }
    }

    /// Returns three vertices representing the RGB gamut of the given RGB space
    /// under the given observer.
    fn rgb_gamut_vertices(rgb_space: RgbSpace, observer: Observer) -> [Vertex; 3] {
        let [red, green, blue] = rgb_space.data().primaries_as_colorants().map(|colorant| {
            observer
                .data()
                .xyz_from_spectrum(&colorant.spectrum(), None)
                .chromaticity()
        });
        [
            Vertex {
                position: red.to_array().map(|v| v as f32),
                color: [0.0, 0.0, 0.0],
            },
            Vertex {
                position: green.to_array().map(|v| v as f32),
                color: [0.0, 0.0, 0.0],
            },
            Vertex {
                position: blue.to_array().map(|v| v as f32),
                color: [0.0, 0.0, 0.0],
            },
        ]
    }

    fn _light_to_vertices_cross(light: impl Light, observer: Observer) -> [Vertex; 8] {
        let [x, y] = light
            .xyzn(observer, None)
            .chromaticity()
            .to_array()
            .map(|v| v as f32);
        Self::cross_vertices_at(x, y)
    }

    fn cross_vertices_at(x: f32, y: f32) -> [Vertex; 8] {
        const CROSS_OFFEST: f32 = 0.002;
        const CROSS_SIZE: f32 = 0.010;
        [
            // Left
            Vertex {
                position: [x - CROSS_OFFEST, y],
                color: [0.0, 0.0, 0.0],
            },
            Vertex {
                position: [x - CROSS_SIZE, y],
                color: [0.0, 0.0, 0.0],
            },
            // Right
            Vertex {
                position: [x + CROSS_OFFEST, y],
                color: [0.0, 0.0, 0.0],
            },
            Vertex {
                position: [x + CROSS_SIZE, y],
                color: [0.0, 0.0, 0.0],
            },
            // Top
            Vertex {
                position: [x, y + CROSS_OFFEST],
                color: [0.0, 0.0, 0.0],
            },
            Vertex {
                position: [x, y + CROSS_SIZE],
                color: [0.0, 0.0, 0.0],
            },
            // Bottom
            Vertex {
                position: [x, y - CROSS_OFFEST],
                color: [0.0, 0.0, 0.0],
            },
            Vertex {
                position: [x, y - CROSS_SIZE],
                color: [0.0, 0.0, 0.0],
            },
        ]
    }

    fn compute_chromaticity_diagram_outline_vertices(observer: Observer) -> Vec<Vertex> {
        let planckian_locus_wavelength_range = observer.data().spectral_locus_wavelength_range();

        let mut vertices = Vec::new();
        for wavelength in planckian_locus_wavelength_range {
            // Compute tristimulus values for the monochromatic spectrum
            let xyz = observer.data().xyz_at_wavelength(wavelength).unwrap();
            // Compute the chromaticity coordinates
            let [x, y] = xyz.chromaticity().to_array();

            vertices.push(Vertex {
                position: [x as f32, y as f32],
                color: [1.0, 1.0, 1.0],
            });
        }
        vertices
    }

    fn chromaticity_diagram_outline_positions(observer: Observer) -> Vec<Vector2<f64>> {
        const BOTTOM_EDGE_RESOLUTION: u16 = 70;

        let spectral_locus_wavelength_range = observer.data().spectral_locus_wavelength_range();

        let mut outer_edge_vertexes = Vec::new();
        for wavelength in spectral_locus_wavelength_range {
            // Compute tristimulus values for the monochromatic spectrum
            let xyz = observer.data().xyz_at_wavelength(wavelength).unwrap();
            // Compute the chromaticity coordinates
            let chromaticity = xyz.chromaticity();

            outer_edge_vertexes.push(chromaticity.to_vec());
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
        colorspace: RgbSpace,
    ) -> (Vec<Vertex>, Vec<u32>) {
        const STEPS_TO_CENTER: u32 = 40;

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

                vertices.push(Self::chromaticity_to_vertex(vertex, observer, colorspace));

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
        vertices.push(Self::chromaticity_to_vertex(center, observer, colorspace));

        (vertices, indices)
    }

    /// Converts a chromaticity coordinate to a vertex with the position and color matching
    /// the chromaticity coordinate.
    fn chromaticity_to_vertex(
        chromaticity: Vector2<f64>,
        observer: Observer,
        colorspace: RgbSpace,
    ) -> Vertex {
        Vertex {
            position: [chromaticity.x as f32, chromaticity.y as f32],
            color: Self::xy_to_rgb(chromaticity, observer, colorspace),
        }
    }

    /// Converts xy chromaticity coordinates to OpenGL compatible RGB values.
    fn xy_to_rgb(chromaticity: Vector2<f64>, observer: Observer, colorspace: RgbSpace) -> [f32; 3] {
        let chromaticity = Chromaticity::new(chromaticity.x, chromaticity.y);

        let xyz = XYZ::from_chromaticity(chromaticity, None, Some(observer)).unwrap();

        let wide_rgb = xyz.rgb(Some(colorspace));
        let rgb = Self::constrain_rgb_to_gamut(wide_rgb);
        let rgb2 = wide_rgb.compress().values();
        if rgb != rgb2 {
            println!(
                "RGB {wide_rgb:?} compresses to:\n\tmy compression: {rgb:?}\n\tlibrary compress(): {rgb2:?}"
            );
        }

        rgb2.map(|v| v as f32)
    }

    /// Desaturates and scales down the luminance of the given wide (unconstrained) RGB value
    /// until all channel values are in the range [0, 1].
    fn constrain_rgb_to_gamut(rgb: WideRgb) -> [f64; 3] {
        let [r, g, b] = rgb.values();
        // Amount of white needed to add to get all channels positive
        let w = -r.min(g).min(b).min(0.0);

        // Positive channel values
        let [pr, pg, pb] = [r + w, g + w, b + w];

        // The maximum channel value. Used to scale all channels linearly to the range [0, 1]
        let max = pr.max(pg).max(pb).max(1.0);

        [pr / max, pg / max, pb / max]
    }
}

impl Drop for ChromaticityDiagram {
    fn drop(&mut self) {}
}
