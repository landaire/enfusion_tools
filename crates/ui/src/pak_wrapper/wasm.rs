use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use eframe::wasm_bindgen::prelude::Closure;
use enfusion_pak::PakFile;
use enfusion_pak::PakParser;
use enfusion_pak::ParserStateMachine;
use enfusion_pak::Stream;
use enfusion_pak::async_pak_vfs::AsyncPrime;
use enfusion_pak::pak_vfs::Prime;
use enfusion_pak::winnow::stream::Offset;
use enfusion_pak::winnow::stream::Stream as _;
use futures::channel::oneshot;
use log::debug;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use web_sys::js_sys;

use crate::task::FileReference;
use crate::task::execute;

#[derive(Debug)]
#[allow(unused)]
pub struct WrappedPakFile {
    path: PathBuf,
    handle: FileReference,
    buffer: Mutex<HashMap<std::ops::Range<usize>, BufferWrapper>>,
    pak_file: PakFile,
}

impl AsRef<PakFile> for WrappedPakFile {
    fn as_ref(&self) -> &PakFile {
        &self.pak_file
    }
}

#[repr(transparent)]
#[derive(Clone, Debug)]
struct BufferWrapper(Arc<oval::Buffer>);

impl AsRef<[u8]> for BufferWrapper {
    fn as_ref(&self) -> &[u8] {
        self.0.data()
    }
}

impl Prime for WrappedPakFile {
    fn prime_file(&self, _file_range: std::ops::Range<usize>) -> impl AsRef<[u8]> {
        if false {
            panic!("PAK files cannot be primed in WASM")
        } else {
            BufferWrapper(Arc::new(oval::Buffer::with_capacity(0)))
        }
    }
}

#[async_trait]
impl AsyncPrime for WrappedPakFile {
    async fn prime_file(&self, file_range: std::ops::Range<usize>) -> impl AsRef<[u8]> {
        debug!("attempting to prime file");
        {
            let buffers = self.buffer.lock().unwrap();

            if let Some(entry) = buffers.get(&file_range) {
                return entry.clone();
            }
        }

        let file_size = file_range.end - file_range.start;

        let (tx, rx) = oneshot::channel();
        let handle = self.handle.clone();
        execute(async move {
            let data = read_file_slice(handle, file_range.start as u64, file_range.end as u64)
                .await
                .expect("failed to read buffer");

            let _ = tx.send(data);
        });

        if let Ok(data) = rx.await {
            let mut buffer = oval::Buffer::with_capacity(file_size);
            let mut data = data.as_slice();
            let mut buffer_slice = buffer.space();
            let read =
                std::io::copy(&mut data, &mut buffer_slice).expect("failed to copy to buffer");
            buffer.fill(read as usize);

            let mut buffers = self.buffer.lock().unwrap();
            // To prevent memory usage from ballooning, we will evict entries from cache if we're above a certain threshold
            let mut buffers_and_mem_usage =
                buffers.iter().map(|(k, v)| (k.clone(), v.0.capacity())).collect::<Vec<_>>();
            let mut mem_usage = buffers_and_mem_usage.iter().fold(0, |accum, (_, mem)| accum + mem);

            // Don't consume more than 20MiB
            const MEM_LIMIT: usize = 1024 * 1024 * 20;
            if mem_usage > MEM_LIMIT {
                // Start removing large items from memory
                buffers_and_mem_usage.sort_by_key(|(_, v)| *v);
                for (k, v) in buffers_and_mem_usage {
                    buffers.remove(&k);

                    mem_usage -= v;
                    if mem_usage < MEM_LIMIT {
                        break;
                    }
                }
            }

            let entry =
                buffers.entry(file_range.clone()).insert_entry(BufferWrapper(Arc::new(buffer)));

            entry.get().clone()
        } else {
            panic!("failed to receive data");
        }
    }
}

// Asynchronously read a slice from a JS File object
pub async fn read_file_slice(
    file_reference: FileReference,
    start: u64,
    end: u64,
) -> Result<Vec<u8>, JsValue> {
    // Stolen from RFD's file reading implementation
    let file = file_reference.clone();
    let promise = js_sys::Promise::new(&mut move |res, _rej| {
        // Create a slice of the file using the slice method
        let blob = file
            .0
            .inner()
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
            file_handle.clone(),
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
                    buffer: Default::default(),
                    pak_file,
                };
            }
            Ok(ParserStateMachine::Skip { from: _, count, parser: next_parser }) => {
                assert!(next_parser.bytes_parsed() > 0);

                buffer.consume(
                    (input.checkpoint().offset_from(&start) + count).min(buffer.available_space()),
                );
                parser = next_parser;
            }
            Ok(ParserStateMachine::Continue(next_parser)) => {
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
