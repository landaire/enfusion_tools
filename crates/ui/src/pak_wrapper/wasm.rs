use std::{
    io::{Read, Seek},
    path::PathBuf,
};

use eframe::wasm_bindgen::prelude::Closure;
use enfusion_pak::{
    PakFile, PakParser, ParserStateMachine, Stream,
    winnow::{
        error::{ErrMode, Needed},
        stream::{Offset, Stream as _},
    },
};
use log::debug;
use oval::Buffer;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{File, FileReader, js_sys};

use crate::task::FileReference;

const BUFFER_SIZE_BYTES: usize = 1024 * 1024 * 20;

#[derive(Debug)]
pub struct WrappedPakFile {
    path: PathBuf,
    handle: FileReference,
    buffer: oval::Buffer,
    pak_file: PakFile,
    pos: usize,
}

impl AsRef<PakFile> for WrappedPakFile {
    fn as_ref(&self) -> &PakFile {
        &self.pak_file
    }
}

impl AsRef<[u8]> for WrappedPakFile {
    fn as_ref(&self) -> &[u8] {
        &self.buffer.data()
    }
}

// Asynchronously read a slice from a JS File object
pub async fn read_file_slice(file: &File, start: u64, end: u64) -> Result<Vec<u8>, JsValue> {
    log::debug!("Performing a read from {start:#X} to {end:#X}");

    // Stolen from RFD's file reading implementation
    let promise = js_sys::Promise::new(&mut move |res, _rej| {
        // Create a slice of the file using the slice method
        let blob = file
            .slice_with_f64_and_f64(start as f64, end as f64)
            .expect("failed to create file blob");

        let file_reader = web_sys::FileReader::new().unwrap();

        let fr = file_reader.clone();
        let closure = Closure::wrap(Box::new(move || {
            res.call1(&JsValue::undefined(), &fr.result().unwrap()).unwrap();
        }) as Box<dyn FnMut()>);

        file_reader.set_onload(Some(closure.as_ref().unchecked_ref()));

        closure.forget();

        file_reader.read_as_array_buffer(&blob).unwrap();
    });
    let future = wasm_bindgen_futures::JsFuture::from(promise);

    let res = future.await.unwrap();

    let buffer: js_sys::Uint8Array = js_sys::Uint8Array::new(&res);
    let mut vec = vec![0; buffer.length() as usize];
    buffer.copy_to(&mut vec[..]);

    debug!("First 4 bytes: {:#X?}", &vec[0..4]);

    Ok(vec)
}

pub async fn parse_pak_file(file_handle: FileReference) -> WrappedPakFile {
    let mut parser = PakParser::new();

    // 16k buffer size
    let mut buffer = oval::Buffer::with_capacity(4096 * 16);

    loop {
        // Populate the buffer with the first 16k
        //
        // TODO: fix this so we only load the minimum amount of data
        let data = read_file_slice(
            file_handle.0.inner(),
            parser.bytes_parsed() as u64,
            (parser.bytes_parsed() + buffer.capacity()) as u64,
        )
        .await
        .expect("failed to read buffer");

        buffer.reset();

        let mut data_slice = data.as_slice();
        let mut buffer_slice = buffer.space();

        let read = std::io::copy(&mut data_slice, &mut buffer_slice)
            .expect("failed to copy from vec to oval buffer");
        buffer.fill(read as usize);

        let mut input = Stream::new(buffer.data());
        let start = input.checkpoint();
        // For mmap the parser should never raise an error or require state transitions
        match parser.parse(&mut input) {
            Ok(ParserStateMachine::Done(pak_file)) => {
                debug!("Parser is done");
                return WrappedPakFile {
                    path: file_handle.0.file_name().into(),
                    handle: file_handle,
                    buffer: Buffer::with_capacity(16 * 4096),
                    pak_file,
                    pos: 0,
                };
            }
            Ok(ParserStateMachine::Skip { from: _, count, parser: next_parser }) => {
                debug!("Parser requested a skip");
                assert!(next_parser.bytes_parsed() > 0);

                buffer.consume(
                    (input.checkpoint().offset_from(&start) + count).min(buffer.available_space()),
                );
                parser = next_parser;
            }
            Ok(ParserStateMachine::Continue(next_parser)) => {
                debug!("Parser requested a continue");
                buffer.consume(input.checkpoint().offset_from(&start));
                parser = next_parser;
            }
            Ok(ParserStateMachine::Loop(_)) => {
                unreachable!("This should never occur");
            }
            Err(e) => {
                panic!("error reading pak file: {:?}", e)
            }
        }
    }
}
