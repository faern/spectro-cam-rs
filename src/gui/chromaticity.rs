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
use colorimetry::cam::{CamTransforms, CieCam16, ViewConditions};
use colorimetry::illuminant::{D65, Illuminant};
use colorimetry::observer::Observer;
use colorimetry::rgb::{RgbSpace, WideRgb};
use colorimetry::traits::Filter;
use colorimetry::traits::Light;
use colorimetry::xyz::{Chromaticity, XYZ};
use eframe::{
    egui, egui_glow,
    glow::{self, HasContext},
};
use egui::ComboBox;
use egui::mutex::Mutex;
use nalgebra::Vector2;
use std::collections::HashMap;
use std::sync::Arc;

pub struct ChromaticityWindow {
    /// Behind an `Arc<Mutex<…>>` so we can pass it to [`egui::PaintCallback`] and paint later.
    chromaticity_diagram: Arc<Mutex<ChromaticityDiagram>>,
    observer: Observer,
    colorspace: RgbSpace,
    normalize_illuminance: bool,
}

impl ChromaticityWindow {
    pub fn new(gl: Arc<glow::Context>) -> Self {
        let observer = Observer::Cie1931;
        let colorspace = RgbSpace::SRGB;
        Self {
            chromaticity_diagram: Arc::new(Mutex::new(ChromaticityDiagram::new(
                gl, observer, colorspace,
            ))),
            observer,
            colorspace,
            normalize_illuminance: true,
        }
    }

    pub fn update(&mut self, ctx: &egui::Context, show: &mut bool, spectrum: &[SpectrumPoint]) {
        let spectrum = Illuminant::new(Self::parse_spectrum(spectrum));

        let mut xyz = spectrum.xyz(Some(self.observer));
        if self.normalize_illuminance {
            xyz = xyz.set_illuminance(100.0);
        }
        let [x, y, z] = xyz.values();
        let chromaticity = xyz.chromaticity();
        let cri = spectrum.cri().map(|cri| cri.ra()).unwrap_or(f64::NAN);
        let cfi = spectrum
            .cfi()
            .map(|cfi| cfi.general_color_fidelity_index())
            .unwrap_or(f64::NAN);
        let (kelvin, tint) = xyz
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

                ComboBox::from_label("Observer")
                    .selected_text(self.observer.to_string())
                    .show_ui(ui, |ui| {
                        let mut changed = false;
                        changed |= ui
                            .selectable_value(
                                &mut self.observer,
                                Observer::Cie1931,
                                Observer::Cie1931.to_string(),
                            )
                            .changed();
                        changed |= ui
                            .selectable_value(
                                &mut self.observer,
                                Observer::Cie1964,
                                Observer::Cie1964.to_string(),
                            )
                            .changed();
                        changed |= ui
                            .selectable_value(
                                &mut self.observer,
                                Observer::Cie2015,
                                Observer::Cie2015.to_string(),
                            )
                            .changed();
                        changed |= ui
                            .selectable_value(
                                &mut self.observer,
                                Observer::Cie2015_10,
                                Observer::Cie2015_10.to_string(),
                            )
                            .changed();

                        if changed {
                            self.chromaticity_diagram.lock().set_observer(self.observer);
                        }
                    });
                ComboBox::from_label("RGB color space")
                    .selected_text(self.colorspace.name())
                    .show_ui(ui, |ui| {
                        let mut changed = false;
                        changed |= ui
                            .selectable_value(
                                &mut self.colorspace,
                                RgbSpace::SRGB,
                                RgbSpace::SRGB.to_string(),
                            )
                            .changed();
                        changed |= ui
                            .selectable_value(
                                &mut self.colorspace,
                                RgbSpace::Adobe,
                                RgbSpace::Adobe.name(),
                            )
                            .changed();
                        changed |= ui
                            .selectable_value(
                                &mut self.colorspace,
                                RgbSpace::DisplayP3,
                                RgbSpace::DisplayP3.name(),
                            )
                            .changed();

                        if changed {
                            self.chromaticity_diagram
                                .lock()
                                .set_colorspace(self.colorspace);
                        }
                    });

                ui.separator();

                ui.horizontal(|ui| {
                    ui.label("Tristimulus values: ");
                    ui.label("X: ");
                    ui.monospace(format!("{:.3}", x));
                    ui.label("Y: ");
                    ui.monospace(format!("{:.3}", y));
                    ui.label("Z: ");
                    ui.monospace(format!("{:.3}", z));
                    ui.checkbox(&mut self.normalize_illuminance, "Normalize");
                });
                ui.horizontal(|ui| {
                    ui.label("Chromaticity coordinates: ");
                    ui.label("x: ");
                    ui.monospace(format!("{:.4}", chromaticity.x()));
                    ui.label("y: ");
                    ui.monospace(format!("{:.4}", chromaticity.y()));
                });
                ui.horizontal(|ui| {
                    ui.label("CRI: ");
                    ui.label("Ra: ");
                    ui.monospace(format!("{:.2}", cri));
                });
                ui.horizontal(|ui| {
                    ui.label("CFI: ");
                    ui.label("Rf: ");
                    ui.monospace(format!("{:.2}", cfi));
                });
                ui.horizontal(|ui| {
                    ui.label("CCT: ");
                    ui.label("Temp: ");
                    ui.monospace(format!("{:.0} K", kelvin));
                    ui.label("Tint: ");
                    ui.monospace(format!("{:.2}", tint));
                });
            });
    }

    fn custom_painting(&mut self, ui: &mut egui::Ui, chromaticity: Chromaticity) {
        // let (rect, _response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::drag());
        let (rect, _response) =
            ui.allocate_exact_size(egui::Vec2::splat(900.0), egui::Sense::drag());

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
    observer: Observer,
    colorspace: RgbSpace,
    gl: Arc<glow::Context>,
    program: ShaderProgram,
    observer_vertices: HashMap<(Observer, RgbSpace), ChromaticityDiagramObserverVertices>,
    cross_vertices: VertexArrayWithBuffer,
}

struct ChromaticityDiagramObserverVertices {
    outline_vertex_buffer: VertexArrayWithBuffer,
    chromaticity_diagram_vertex_buffer: VertexArrayWithBuffer,
    chromaticity_diagram_index_buffer: VertexIndexBuffer,
    planckian_locus_vertex_buffer: VertexArrayWithBuffer,
    rgb_gamut_vertex_array: VertexArrayWithBuffer,
    ciecam_hue_line_vertex_buffers: Vec<VertexArrayWithBuffer>,
}

impl ChromaticityDiagram {
    fn new(gl: Arc<glow::Context>, observer: Observer, colorspace: RgbSpace) -> Self {
        let program = ShaderProgram::new(gl.clone());

        let cross_vertices = create_vertex_buffer(
            gl.clone(),
            &Self::cross_vertices_at(0.0, 0.0),
            program.position_attribute_index,
            program.color_attribute_index,
        );

        Self {
            observer,
            colorspace,
            gl,
            program,
            observer_vertices: HashMap::new(),
            cross_vertices,
        }
    }

    fn set_observer(&mut self, observer: Observer) {
        self.observer = observer;
    }

    fn set_colorspace(&mut self, colorspace: RgbSpace) {
        self.colorspace = colorspace;
    }

    fn compute_observer_vertices(
        gl: Arc<glow::Context>,
        program: &ShaderProgram,
        observer: Observer,
        colorspace: RgbSpace,
    ) -> ChromaticityDiagramObserverVertices {
        let outline_vertices = Self::compute_chromaticity_diagram_outline_vertices(observer);

        let outline_vertex_buffer = create_vertex_buffer(
            gl.clone(),
            &outline_vertices,
            program.position_attribute_index,
            program.color_attribute_index,
        );

        let chromaticity_diagram_outline_positions =
            Self::chromaticity_diagram_outline_positions(observer);
        let center = D65.xyz(Some(observer)).chromaticity().to_vector();
        let (chromaticity_diagram_vertices, chromaticity_diagram_indices) =
            Self::compute_gl_triangle_strip_from_ring(
                &chromaticity_diagram_outline_positions,
                center,
                observer,
                colorspace,
            );

        let chromaticity_diagram_vertex_buffer = create_vertex_buffer(
            gl.clone(),
            &chromaticity_diagram_vertices,
            program.position_attribute_index,
            program.color_attribute_index,
        );
        let chromaticity_diagram_index_buffer =
            create_index_buffer(gl.clone(), &chromaticity_diagram_indices);

        let planckian_locus_vertex_buffer = create_vertex_buffer(
            gl.clone(),
            &Self::compute_planckian_locus_vertices(observer),
            program.position_attribute_index,
            program.color_attribute_index,
        );

        let rgb_gamut_vertex_array = create_vertex_buffer(
            gl.clone(),
            &Self::rgb_gamut_vertices(colorspace, observer),
            program.position_attribute_index,
            program.color_attribute_index,
        );

        let mut ciecam_hue_line_vertex_buffers = Vec::new();
        // for edge_pos in chromaticity_diagram_outline_positions.iter().step_by(10) {
        //     let chromaticity = Chromaticity::new(edge_pos.x, edge_pos.y);
        //     let xyz = XYZ::from_chromaticity(chromaticity, None, Some(observer)).unwrap();
        //     ciecam_hue_line_vertex_buffers.push(create_vertex_buffer(
        //         gl.clone(),
        //         &Self::compute_ciecam_hue_line(xyz, observer),
        //         program.position_attribute_index,
        //         program.color_attribute_index,
        //     ));
        // }
        // for wavelength in [390, 470, 480, 485, 495, 505, 520, 540, 570, 590, 610, 699] {
        //     let xyz = observer
        //         .xyz_at_wavelength(wavelength)
        //         .unwrap()
        //         .set_illuminance(100.0);
        //     // let constrained_xyz = XYZ::from_chromaticity(xyz.chromaticity(), None, Some(observer)).unwrap();
        //     // log::debug!("Luminance1 {}, luminance2: {}", xyz.y(), constrained_xyz.y());
        //     ciecam_hue_line_vertex_buffers.push(create_vertex_buffer(
        //         gl.clone(),
        //         &Self::compute_ciecam_hue_line(xyz, observer),
        //         program.position_attribute_index,
        //         program.color_attribute_index,
        //     ));
        // }
        for hue in (1..=360).step_by(20) {
            ciecam_hue_line_vertex_buffers.push(create_vertex_buffer(
                gl.clone(),
                &Self::compute_ciecam_hue_line_for_hue(hue as f64, observer),
                program.position_attribute_index,
                program.color_attribute_index,
            ));
        }

        ChromaticityDiagramObserverVertices {
            outline_vertex_buffer,
            chromaticity_diagram_vertex_buffer,
            chromaticity_diagram_index_buffer,
            planckian_locus_vertex_buffer,
            rgb_gamut_vertex_array,
            ciecam_hue_line_vertex_buffers,
        }
    }

    fn paint(&mut self, gl: &glow::Context, chromaticity: Chromaticity) {
        use glow::HasContext as _;

        let ChromaticityDiagramObserverVertices {
            outline_vertex_buffer,
            chromaticity_diagram_vertex_buffer,
            chromaticity_diagram_index_buffer,
            planckian_locus_vertex_buffer,
            rgb_gamut_vertex_array,
            ciecam_hue_line_vertex_buffers,
        } = self
            .observer_vertices
            .entry((self.observer, self.colorspace))
            .or_insert_with(|| {
                Self::compute_observer_vertices(
                    self.gl.clone(),
                    &self.program,
                    self.observer,
                    self.colorspace,
                )
            });
        unsafe {
            gl.use_program(Some(self.program.program));

            self.program.set_offset(gl, 0.0, 0.0);

            // Draw the chromaticity diagram "background"
            gl.bind_vertex_array(Some(chromaticity_diagram_vertex_buffer.vertex_array()));
            gl.bind_buffer(
                glow::ELEMENT_ARRAY_BUFFER,
                Some(chromaticity_diagram_index_buffer.index_buffer()),
            );
            gl.draw_elements(
                glow::TRIANGLE_STRIP,
                chromaticity_diagram_index_buffer.len(),
                glow::UNSIGNED_INT,
                0,
            );

            // // Draw the outline of the chromaticity diagram
            // gl.bind_vertex_array(Some(outline_vertex_buffer.vertex_array()));
            // gl.draw_arrays(glow::LINE_LOOP, 0, outline_vertex_buffer.len());

            // gl.bind_vertex_array(Some(planckian_locus_vertex_buffer.vertex_array()));
            // gl.line_width(1.0);
            // gl.draw_arrays(glow::LINE_STRIP, 0, planckian_locus_vertex_buffer.len());

            // Draw the gamut triangle of the selected RGB space
            gl.bind_vertex_array(Some(rgb_gamut_vertex_array.vertex_array()));
            gl.line_width(1.5);
            gl.draw_arrays(glow::LINE_LOOP, 0, 3);

            for ciecam_hue_line_vertex_buffer in ciecam_hue_line_vertex_buffers {
                gl.bind_vertex_array(Some(ciecam_hue_line_vertex_buffer.vertex_array()));
                gl.line_width(2.0);
                gl.draw_arrays(glow::LINE_STRIP, 0, ciecam_hue_line_vertex_buffer.len());
            }

            // // Draw a cross at the measured spectrum position
            // gl.line_width(2.0);
            // self.program
            //     .set_offset(gl, chromaticity.x() as f32, chromaticity.y() as f32);
            // gl.bind_vertex_array(Some(self.cross_vertices.vertex_array()));
            // gl.line_width(2.0);
            // gl.draw_arrays(glow::LINES, 0, 8);
        }
    }

    /// Returns three vertices representing the RGB gamut of the given RGB space
    /// under the given observer.
    fn rgb_gamut_vertices(rgb_space: RgbSpace, observer: Observer) -> [Vertex; 3] {
        let [red, green, blue] = rgb_space.data().primaries_as_colorants().map(|colorant| {
            observer
                .xyz_from_spectrum(&colorant.spectrum())
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
        const CROSS_SIZE: f32 = 0.015;
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

    /// Computes the vertices of the Planckian locus line.
    ///
    /// Returns `None` if the observer is not CIE 1931, since the colorimetry library does not
    /// support this currently.
    fn compute_planckian_locus_vertices(observer: Observer) -> Vec<Vertex> {
        let temp_to_vertex = |temp: f64| {
            let chromaticity = observer.xyz_planckian_locus(temp).chromaticity();
            Vertex {
                position: [chromaticity.x() as f32, chromaticity.y() as f32],
                color: [0.0, 0.0, 0.0],
            }
        };

        let mut vertices = Vec::new();
        let mut temp = 1000.0;
        // High resolution at low temperatures
        while temp < 10_000.0 {
            vertices.push(temp_to_vertex(temp));
            temp = temp.powf(1.01);
        }
        // Low resolution at high temperatures
        while temp < 1_000_000.0 {
            vertices.push(temp_to_vertex(temp));
            temp = temp.powf(1.1);
        }
        vertices
    }

    fn compute_chromaticity_diagram_outline_vertices(observer: Observer) -> Vec<Vertex> {
        let planckian_locus_wavelength_range = observer.spectral_locus_wavelength_range();

        let mut vertices = Vec::new();
        for wavelength in planckian_locus_wavelength_range {
            // Compute tristimulus values for the monochromatic spectrum
            let xyz = observer.xyz_at_wavelength(wavelength).unwrap();
            // Compute the chromaticity coordinates
            let [x, y] = xyz.chromaticity().to_array();

            vertices.push(Vertex {
                position: [x as f32, y as f32],
                color: [1.0, 1.0, 1.0],
            });
        }
        vertices
    }

    fn compute_ciecam_hue_line(xyz: XYZ, observer: Observer) -> Vec<Vertex> {
        let xyzn = observer.xyz_d65();
        let viewconditions = ViewConditions::default();

        let xyz_to_vertex = |xyz: XYZ| {
            let chromaticity = xyz.chromaticity();
            Vertex {
                position: [chromaticity.x() as f32, chromaticity.y() as f32],
                color: [0.5, 0.5, 0.5],
            }
        };

        let mut vertices = vec![];
        // Start with the original XYZ value
        vertices.push(xyz_to_vertex(xyz));

        // The lightness and hue stay fixed. We only lower the chroma.
        let [lightness, mut chroma, hue] =
            dbg!(CieCam16::from_xyz(xyz, xyzn, viewconditions).unwrap().jch());
        loop {
            let cam = CieCam16::new([lightness, chroma, hue], xyzn, viewconditions);
            let xyz = cam.xyz(None, None).unwrap();
            vertices.push(xyz_to_vertex(xyz));

            // When there are no negative RGB values, we stop.
            if xyz.rgb(None).values().iter().all(|&v| v >= 0.0) {
                break;
            }
            chroma *= 0.95;
        }
        vertices
    }

    fn compute_ciecam_hue_line_for_hue(hue: f64, observer: Observer) -> Vec<Vertex> {
        let xyzn = observer.xyz_d65();
        let viewconditions = ViewConditions::default();

        let xyz_to_vertex = |xyz: XYZ| {
            let chromaticity = xyz.chromaticity();
            Vertex {
                position: [chromaticity.x() as f32, chromaticity.y() as f32],
                color: [0.9, 0.9, 0.9],
            }
        };
        let is_valid_chromaticity_coordinate = |xyz: XYZ| {
            let [x, y] = xyz.chromaticity().to_array();
            (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y) && x + y <= 1.0
        };

        let mut vertices = vec![];
        let lightness = 100.0;
        let mut chroma = 0.0;
        loop {
            log::debug!("Hue: {hue}, Chroma: {}", chroma);
            let cam = CieCam16::new([lightness, chroma, hue], xyzn, viewconditions);
            let xyz = cam.xyz(None, None).unwrap();
            vertices.push(xyz_to_vertex(xyz));

            if !is_valid_chromaticity_coordinate(xyz) {
                break;
            }
            chroma = (chroma * 1.1).max(0.1);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        vertices
    }

    fn chromaticity_diagram_outline_positions(observer: Observer) -> Vec<Vector2<f64>> {
        const BOTTOM_EDGE_RESOLUTION: u16 = 70;

        let spectral_locus_wavelength_range = observer.spectral_locus_wavelength_range();

        let mut outer_edge_vertexes = Vec::new();
        let mut last_position = Vector2::new(0.0, 0.0);
        for wavelength in spectral_locus_wavelength_range.clone() {
            // Compute tristimulus values for the monochromatic spectrum
            let xyz = observer.xyz_at_wavelength(wavelength).unwrap();
            // Compute the chromaticity coordinates
            let chromaticity = xyz.chromaticity();
            let position = chromaticity.to_vector();

            if (position - last_position).norm() > 1E-2
                || wavelength == *spectral_locus_wavelength_range.end()
            {
                outer_edge_vertexes.push(position);
                last_position = position;
            }
        }
        let bottom_edge_start = *outer_edge_vertexes.last().unwrap();
        let bottom_edge_end = *outer_edge_vertexes.first().unwrap();
        let bottom_edge_diff = bottom_edge_end - bottom_edge_start;
        for i in 1..BOTTOM_EDGE_RESOLUTION {
            let ratio = i as f64 / BOTTOM_EDGE_RESOLUTION as f64;
            let bottom_edge_vector = bottom_edge_start + bottom_edge_diff * ratio;
            outer_edge_vertexes.push(bottom_edge_vector);
        }
        log::debug!(
            "Chromaticity diagram outer edge vertexes: {}",
            outer_edge_vertexes.len()
        );
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

        let wide_rgb: WideRgb = xyz.rgb(Some(colorspace));
        let in_gamut_rgb = wide_rgb.compress().values();

        in_gamut_rgb.map(|v| v as f32)
    }

    // /// Converts xy chromaticity coordinates to OpenGL compatible RGB values.
    // fn xy_to_rgb(chromaticity: Vector2<f64>, observer: Observer, colorspace: RgbSpace) -> [f32; 3] {
    //     let chromaticity = Chromaticity::new(chromaticity.x, chromaticity.y);

    //     let mut xyz = XYZ::from_chromaticity(chromaticity, None, Some(observer)).unwrap();
    //     let xyzn = observer.xyz_d65();
    //     let viewconditions = ViewConditions::default();

    //     let cam = CieCam16::from_xyz(xyz, xyzn, viewconditions).unwrap();
    //     let [lightness, mut chroma, hue] = cam.jch();
    //     let mut chroma_upper_bound = chroma;
    //     let mut chroma_lower_bound = 0.0;

    //     let wide_rgb: WideRgb = xyz.rgb(Some(colorspace));
    //     if wide_rgb.values().iter().all(|&v| v >= 0.0) {
    //         return wide_rgb.compress().values().map(|v| v as f32);
    //     }
    //     let wide_rgb = loop {
    //         chroma = (chroma_upper_bound + chroma_lower_bound) / 2.0;
    //         log::debug!("Trying chroma {chroma} for with J {lightness} and H {hue}");
    //         let cam = CieCam16::new([lightness, chroma, hue], xyzn, viewconditions);
    //         xyz = cam.xyz(None, None).unwrap();

    //         let wide_rgb: WideRgb = xyz.rgb(Some(colorspace));
    //         if wide_rgb.values().iter().all(|&v| v >= 0.0) {
    //             if chroma_upper_bound - chroma_lower_bound < 1E-1 {
    //                 break wide_rgb;
    //             }
    //             chroma_lower_bound = chroma;
    //             log::debug!("Bringing lower chroma bound up to {chroma_lower_bound}");
    //         } else {
    //             chroma_upper_bound = chroma;
    //             log::debug!("Bringing upper chroma bound down to {chroma_upper_bound}");
    //         }
    //         // sleep(Duration::from_millis(100));
    //     };

    //     wide_rgb.compress().values().map(|v| v as f32)
    // }
}

impl Drop for ChromaticityDiagram {
    fn drop(&mut self) {}
}
