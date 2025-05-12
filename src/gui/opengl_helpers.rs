use core::mem;
use eframe::glow::{self, HasContext};
use std::sync::Arc;

const NUM_POSITION_COMPONENTS: usize = 2;
const NUM_COLOR_COMPONENTS: usize = 3;
type Position = [f32; NUM_POSITION_COMPONENTS];
type Color = [f32; NUM_COLOR_COMPONENTS];

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct Vertex {
    pub position: Position,
    pub color: Color,
}

pub struct VertexArrayWithBuffer {
    gl: Arc<glow::Context>,
    vertex_array: glow::NativeVertexArray,
    vertex_buffer: glow::NativeBuffer,
    len: i32,
}

impl VertexArrayWithBuffer {
    pub fn vertex_array(&self) -> glow::NativeVertexArray {
        self.vertex_array
    }

    /// Returns number of elements in the vertex buffer
    pub fn len(&self) -> i32 {
        self.len
    }
}

impl Drop for VertexArrayWithBuffer {
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_vertex_array(self.vertex_array);
            self.gl.delete_buffer(self.vertex_buffer);
        }
    }
}

pub struct VertexIndexBuffer {
    gl: Arc<glow::Context>,
    index_buffer: glow::NativeBuffer,
    len: i32,
}

impl VertexIndexBuffer {
    pub fn index_buffer(&self) -> glow::NativeBuffer {
        self.index_buffer
    }

    /// Returns number of elements in the index buffer
    pub fn len(&self) -> i32 {
        self.len
    }
}

impl Drop for VertexIndexBuffer {
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_buffer(self.index_buffer);
        }
    }
}

/// Returns a vertex buffer object (VBO) containing the data in the passed in `vertices`,
/// along with a vertex array object (VAO) representing the data layout and contents of the VBO.
///
/// This first allocates a vertex buffer object (VBO) and uploads the vertex data to the GPU.
/// Then it creates a vertex array object (VAO) and binds the VBO to it with VAO attribute
/// `position_attribute_index` pointing to the position data and VAO attribute
/// `color_attribute_index` pointing to the color data.
pub fn create_vertex_buffer(
    gl: Arc<glow::Context>,
    vertices: &[Vertex],
    position_attribute_index: u32,
    color_attribute_index: u32,
) -> VertexArrayWithBuffer {
    const BYTES_PER_VERTEX: usize = mem::size_of::<Vertex>();

    // assert the NUM_* constants match the layout of the Vertex struct
    debug_assert_eq!(
        BYTES_PER_VERTEX,
        mem::size_of::<f32>() * (NUM_POSITION_COMPONENTS + NUM_COLOR_COMPONENTS)
    );

    // SAFETY: The passed in pointer is valid for the passed in length
    // Every byte is properly initialized, there is no padding between consecutive elements.
    // The alignment is correct, since the alignment of `u8` is 1.
    // The data is guaranteed to be valid and not mutated for the lifetime of this slice.
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
            NUM_POSITION_COMPONENTS as i32,
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
            NUM_COLOR_COMPONENTS as i32,
            glow::FLOAT,
            false,
            BYTES_PER_VERTEX as i32,
            core::mem::offset_of!(Vertex, color) as i32,
        );
        // Activate the color attribute. By default all attributes are disabled
        gl.enable_vertex_attrib_array(color_attribute_index);

        VertexArrayWithBuffer {
            gl,
            vertex_array,
            vertex_buffer,
            len: i32::try_from(vertices.len()).expect("Too many vertices"),
        }
    }
}

/// Creates an index buffer object (IBO) from the given indices.
pub fn create_index_buffer(gl: Arc<glow::Context>, indices: &[u32]) -> VertexIndexBuffer {
    const BYTES_PER_INDEX: usize = core::mem::size_of::<u32>();

    // SAFETY: The passed in pointer is valid for the passed in length
    // Every byte is properly initialized, there is no padding between consecutive elements.
    // The alignment is correct, since the alignment of `u8` is 1.
    // The data is guaranteed to be valid and not mutated for the lifetime of this slice.
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
        VertexIndexBuffer {
            gl,
            index_buffer,
            len: i32::try_from(indices.len()).expect("Too many indices"),
        }
    }
}

/// Creates a shader program from the given vertex and fragment shader source code.
///
/// # Panics
///
/// This function panics if the shader compilation or program linking fails.
pub fn create_program(
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
