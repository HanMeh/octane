use std::alloc;
use std::fs;
use std::io::{self, BufRead};
use std::mem;
use std::ptr;
use std::slice;

pub struct Mesh {
    vertex_count: usize,
    index_count: usize,
    data: ptr::NonNull<u8>,
}

impl Mesh {
    pub fn from_obj(file: fs::File) -> Self {
        let buf_reader = io::BufReader::new(file);

        let mut vertices = vec![];
        let mut indices = vec![];

        //not a totally accurate obj reader. made to read a single cube
        for line in buf_reader.lines() {
            let line = line.expect("failed to read line");
            let segments = line.split(" ").collect::<Vec<_>>();

            match segments[0] {
                "v" => {
                    let vertex = [
                        segments[1].parse::<f32>().expect("failed to parse float"),
                        segments[2].parse::<f32>().expect("failed to parse float"),
                        segments[3].parse::<f32>().expect("failed to parse float"),
                    ];

                    vertices.push(vertex);
                }
                "f" => {
                    let parse_index = |id: &str| {
                        let y = id.split("/").collect::<Vec<_>>();

                        y[0].parse::<usize>().expect("failed to parse usize")
                    };

                    indices.push(parse_index(segments[1]));
                    indices.push(parse_index(segments[2]));
                    indices.push(parse_index(segments[3]));
                }
                _ => {}
            }
        }

        Mesh::create(&vertices, &indices)
    }

    pub fn create(vertices: &'_ [[f32; 3]], indices: &'_ [usize]) -> Self {
        let vertex_byte_len = vertices.len() * mem::size_of::<[f32; 3]>();
        let index_byte_len = indices.len() * mem::size_of::<usize>();
        let byte_len = vertex_byte_len + index_byte_len;

        let layout = alloc::Layout::array::<u8>(byte_len).expect("failed to create layout");

        let data = match ptr::NonNull::new(unsafe { alloc::alloc(layout) }) {
            Some(p) => p,
            None => alloc::handle_alloc_error(layout),
        };

        let (data_vertex, data_index) = unsafe {
            (
                data.as_ptr().cast::<_>(),
                data.as_ptr().add(vertex_byte_len).cast::<_>(),
            )
        };

        unsafe { ptr::copy(&vertices[0], data_vertex, vertices.len()) };
        unsafe { ptr::copy(&indices[0], data_index, indices.len()) };

        Self {
            vertex_count: vertices.len(),
            index_count: indices.len(),
            data,
        }
    }

    pub fn get(&self) -> (&'_ [[f32; 3]], &'_ [usize]) {
        let vertices = unsafe {
            slice::from_raw_parts(
                self.data
                    .as_ptr()
                    .cast::<[f32; 3]>()
                    .add(self.get_vertex_offset()),
                self.vertex_count,
            )
        };

        let indices = unsafe {
            slice::from_raw_parts(
                self.data
                    .as_ptr()
                    .cast::<usize>()
                    .add(self.get_index_offset()),
                self.index_count,
            )
        };

        (vertices, indices)
    }

    #[inline]
    fn get_vertex_offset(&self) -> usize {
        0
    }

    #[inline]
    fn get_index_offset(&self) -> usize {
        self.get_vertex_offset() + self.vertex_count * mem::size_of::<[f32; 3]>()
    }
}

impl Drop for Mesh {
    fn drop(&mut self) {}
}
