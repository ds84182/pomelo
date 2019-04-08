use std::io::Cursor;

use byteorder::{ByteOrder, LittleEndian, ReadBytesExt, WriteBytesExt};

use crate::Handle;

pub struct IPCContext<'a> {
    data: &'a mut [u8],
}

impl<'a> IPCContext<'a> {
    pub fn new(data: &'a mut [u8]) -> IPCContext<'a> {
        IPCContext { data }
    }

    pub fn header(&self) -> IPCHeaderCode {
        IPCRequest::new(self.data).header
    }

    pub fn request(&'a self) -> IPCRequest<'a> {
        IPCRequest::new(self.data)
    }

    pub fn response(self, result: u32) -> IPCResponse<'a> {
        let header = self.header();
        IPCResponse::new(header, result, self.data)
    }
}

pub struct IPCRequest<'a> {
    header: IPCHeaderCode,
    data: Cursor<&'a [u8]>,
    normal_parameter_count: u8,
    translate_parameter_words: u8,
}

impl<'a> IPCRequest<'a> {
    pub fn new(data: &'a [u8]) -> IPCRequest<'a> {
        let mut data = Cursor::new(data);
        let header = IPCHeaderCode::from(data.read_u32::<LittleEndian>().unwrap());
        IPCRequest {
            header,
            data,
            normal_parameter_count: header.normal_parameter_count,
            translate_parameter_words: header.translate_parameter_size,
        }
    }

    pub fn read_normal(&mut self) -> u32 {
        assert!(self.normal_parameter_count > 0);
        self.normal_parameter_count -= 1;
        self.data.read_u32::<LittleEndian>().unwrap()
    }

    fn read_translate_word(&mut self) -> u32 {
        assert!(self.translate_parameter_words > 0);
        self.translate_parameter_words -= 1;
        self.data.read_u32::<LittleEndian>().unwrap()
    }

    fn read_translate_handle(&mut self) -> Option<Handle> {
        Handle::from_raw(self.read_translate_word())
    }

    pub fn read_translate(&mut self) -> IPCTranslateParameter {
        assert!(self.normal_parameter_count == 0);
        let descriptor = self.read_translate_word();
        match descriptor & 7 {
            0 => {
                let moved = (descriptor & 16) != 0;
                let calling_process = (descriptor & 32) != 0;
                let count = (descriptor & 0xFC000000) >> 26;
                let handles = if count == 0 {
                    let mut handle = self.read_translate_handle();
                    if calling_process {
                        handle = Handle::from_raw(0xFFFF8001);
                    }
                    IPCHandles::Single(handle)
                } else {
                    let mut vec = Vec::with_capacity((count + 1) as usize);
                    for _ in 0..(count + 1) {
                        let mut handle = self.read_translate_handle();
                        if calling_process {
                            handle = Handle::from_raw(0xFFFF8001);
                        }
                        vec.push(handle);
                    }
                    IPCHandles::Multiple(vec)
                };
                IPCTranslateParameter::Handle { moved, calling_process, handles }
            },
            1 => {
                let index = ((descriptor & 0x3C00) >> 10) as u8;
                let size = (descriptor & 0xFFFFC000) >> 14;
                let ptr = self.read_translate_word();
                IPCTranslateParameter::StaticBuffer { index, size, ptr }
            },
            n => {
                let permission = (n & 3) as u8;
                let size = (descriptor & 0xFFFFFFF0) >> 4;
                let ptr = self.read_translate_word();
                IPCTranslateParameter::BufferMapping { permission, size, ptr }
            }
        }
    }
}

pub struct IPCResponse<'a> {
    command_id: u16,
    normal_parameter_count: u8,
    translate_parameter_size: u8,
    data: Cursor<&'a mut [u8]>,
}

impl<'a> IPCResponse<'a> {
    pub fn new(header: IPCHeaderCode, result: u32, data: &'a mut [u8]) -> IPCResponse<'a> {
        let mut response = IPCResponse {
            command_id: header.command_id,
            normal_parameter_count: 0,
            translate_parameter_size: 0,
            data: Cursor::new(data),
        };

        response.write(0);
        response.write(result);

        response
    }

    fn write(&mut self, word: u32) {
        self.data.write_u32::<LittleEndian>(word).unwrap();
    }

    pub fn write_normal(&mut self, param: u32) {
        assert!(self.translate_parameter_size == 0);
        self.normal_parameter_count += 1;
        self.write(param);
    }

    pub fn write_translate(&mut self, param: &IPCTranslateParameter) {
        self.translate_parameter_size += 1;
        match param {
            IPCTranslateParameter::Handle { moved, calling_process, ref handles } => {
                self.write(
                    0 |
                    (if *moved { 16 } else { 0 }) |
                    (if *calling_process { 32 } else { 0 }) |
                    ((handles.as_slice().len() - 1) as u32) << 26
                );
                for handle in handles.as_slice() {
                    self.write(handle.as_ref().map(|h| h.raw()).unwrap_or(0));
                    self.translate_parameter_size += 1;
                }
            },
            _ => unimplemented!("Unimplemented write_translate({:?})", param)
        }
    }
}

impl<'a> Drop for IPCResponse<'a> {
    fn drop(&mut self) {
        use std::{io, io::Seek};
        self.data.seek(io::SeekFrom::Start(0)).unwrap();
        self.write(
            (self.command_id as u32) << 16 |
            (self.normal_parameter_count as u32) << 6 |
            self.translate_parameter_size as u32
        )
    }
}

pub struct IPCReader<'a> {
    header: IPCHeaderCode,
    result: u32,
    data: Cursor<&'a [u8]>,
    normal_parameter_count: u8,
    translate_parameter_words: u8,
}

impl<'a> IPCReader<'a> {
    // Reads an IPC request
    pub fn new_request(data: &'a [u8]) -> IPCReader<'a> {
        let mut data = Cursor::new(data);
        let header = IPCHeaderCode::from(data.read_u32::<LittleEndian>().unwrap());
        IPCReader {
            header,
            result: 0,
            data,
            normal_parameter_count: header.normal_parameter_count,
            translate_parameter_words: header.translate_parameter_size,
        }
    }

    // Reads an IPC response
    pub fn new_response(data: &'a [u8]) -> IPCReader<'a> {
        let mut data = Cursor::new(data);
        let header = IPCHeaderCode::from(data.read_u32::<LittleEndian>().unwrap());
        let result = data.read_u32::<LittleEndian>().unwrap();
        IPCReader {
            header,
            result,
            data,
            normal_parameter_count: header.normal_parameter_count,
            translate_parameter_words: header.translate_parameter_size,
        }
    }

    pub fn header(&self) -> IPCHeaderCode {
        self.header
    }

    pub fn result(&self) -> u32 {
        self.result
    }

    pub fn read_normal(&mut self) -> u32 {
        assert!(self.normal_parameter_count > 0);
        self.normal_parameter_count -= 1;
        self.data.read_u32::<LittleEndian>().unwrap()
    }

    fn read_translate_word(&mut self) -> u32 {
        assert!(self.translate_parameter_words > 0);
        self.translate_parameter_words -= 1;
        self.data.read_u32::<LittleEndian>().unwrap()
    }

    fn read_translate_handle(&mut self) -> Option<Handle> {
        Handle::from_raw(self.read_translate_word())
    }

    pub fn has_more_translate_params(&self) -> bool {
        self.translate_parameter_words > 0
    }

    pub fn read_translate(&mut self) -> IPCTranslateParameter {
        assert!(self.normal_parameter_count == 0);
        let descriptor = self.read_translate_word();
        match descriptor & 7 {
            0 => {
                let moved = (descriptor & 16) != 0;
                let calling_process = (descriptor & 32) != 0;
                let count = (descriptor & 0xFC000000) >> 26;
                let handles = if count == 0 {
                    let mut handle = self.read_translate_handle();
                    if calling_process {
                        handle = Handle::from_raw(0xFFFF8001);
                    }
                    IPCHandles::Single(handle)
                } else {
                    let mut vec = Vec::with_capacity((count + 1) as usize);
                    for _ in 0..(count + 1) {
                        let mut handle = self.read_translate_handle();
                        if calling_process {
                            handle = Handle::from_raw(0xFFFF8001);
                        }
                        vec.push(handle);
                    }
                    IPCHandles::Multiple(vec)
                };
                IPCTranslateParameter::Handle { moved, calling_process, handles }
            },
            1 => {
                let index = ((descriptor & 0x3C00) >> 10) as u8;
                let size = (descriptor & 0xFFFFC000) >> 14;
                let ptr = self.read_translate_word();
                IPCTranslateParameter::StaticBuffer { index, size, ptr }
            },
            n => {
                let permission = ((n & 0b0110) as u8) >> 1;
                let size = (descriptor & 0xFFFFFFF0) >> 4;
                let ptr = self.read_translate_word();
                IPCTranslateParameter::BufferMapping { permission, size, ptr }
            }
        }
    }
}

pub struct IPCWriter<'a> {
    command_id: u16,
    normal_parameter_count: u8,
    translate_parameter_size: u8,
    data: Cursor<&'a mut [u8]>,
}

impl<'a> IPCWriter<'a> {
    pub fn new_request(command_id: u16, data: &'a mut [u8]) -> IPCWriter<'a> {
        let mut response = IPCWriter {
            command_id,
            normal_parameter_count: 0,
            translate_parameter_size: 0,
            data: Cursor::new(data),
        };

        response.write(0);

        response
    }

    pub fn new_response(command_id: u16, result: u32, data: &'a mut [u8]) -> IPCWriter<'a> {
        let mut response = IPCWriter {
            command_id,
            normal_parameter_count: 0,
            translate_parameter_size: 0,
            data: Cursor::new(data),
        };

        response.write(0);
        response.write(result);

        response
    }

    fn write(&mut self, word: u32) {
        self.data.write_u32::<LittleEndian>(word).unwrap();
    }

    pub fn write_normal(&mut self, param: u32) {
        assert!(self.translate_parameter_size == 0);
        self.normal_parameter_count += 1;
        self.write(param);
    }

    pub fn write_translate(&mut self, param: &IPCTranslateParameter) {
        self.translate_parameter_size += 1;
        match param {
            IPCTranslateParameter::Handle { moved, calling_process, ref handles } => {
                self.write(
                    0 |
                    (if *moved { 16 } else { 0 }) |
                    (if *calling_process { 32 } else { 0 }) |
                    ((handles.as_slice().len() - 1) as u32) << 26
                );
                for handle in handles.as_slice() {
                    self.write(handle.as_ref().map(|h| h.raw()).unwrap_or(0));
                    self.translate_parameter_size += 1;
                }
            },
            _ => unimplemented!("Unimplemented write_translate({:?})", param)
        }
    }
}

impl<'a> Drop for IPCWriter<'a> {
    fn drop(&mut self) {
        use std::{io, io::Seek};
        self.data.seek(io::SeekFrom::Start(0)).unwrap();
        self.write(
            (self.command_id as u32) << 16 |
            (self.normal_parameter_count as u32) << 6 |
            self.translate_parameter_size as u32
        )
    }
}

#[derive(Debug)]
pub enum IPCHandles {
    Single(Option<Handle>),
    Multiple(Vec<Option<Handle>>)
}

impl IPCHandles {
    fn as_slice(&self) -> &[Option<Handle>] {
        use std::slice;
        match self {
            IPCHandles::Single(handle) => slice::from_ref(&handle),
            IPCHandles::Multiple(handles) => &handles,
        }
    }
}

#[derive(Debug)]
pub enum IPCTranslateParameter {
    Handle {
        moved: bool,
        calling_process: bool,
        handles: IPCHandles,
    },
    StaticBuffer {
        index: u8,
        size: u32,
        ptr: u32,
    },
    BufferMapping {
        permission: u8,
        size: u32,
        ptr: u32,
    }
}

#[derive(Debug, Copy, Clone)]
pub struct IPCHeaderCode {
    pub command_id: u16,
    pub normal_parameter_count: u8,
    pub translate_parameter_size: u8,
}

impl From<u32> for IPCHeaderCode {
    fn from(value: u32) -> IPCHeaderCode {
        IPCHeaderCode {
            command_id: ((value & 0xFFFF0000) >> 16) as u16,
            normal_parameter_count: ((value & 0xFC0) >> 6) as u8,
            translate_parameter_size: (value & 0x3F) as u8,
        }
    }
}
