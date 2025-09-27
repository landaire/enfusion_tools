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
use enfusion_pak::async_pak_vfs::AsyncReadAt;
use enfusion_pak::pak_vfs::Prime;
use enfusion_pak::vfs::VfsError;
use enfusion_pak::winnow::stream::Offset;
use enfusion_pak::winnow::stream::Stream as _;
use futures::channel::oneshot;
use log::debug;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use web_sys::js_sys;

use crate::task::execute;

#[repr(transparent)]
#[derive(Clone, Debug)]
pub struct FileReference(pub rfd::FileHandle);

unsafe impl Send for FileReference {}
unsafe impl Sync for FileReference {}

#[async_trait]
impl AsyncReadAt for FileReference {
    async fn read_at(
        &self,
        file_range: std::ops::Range<usize>,
    ) -> Result<impl AsRef<[u8]>, VfsError> {
        let (tx, rx) = oneshot::channel();
        let handle = self.clone();

        // Need to execute this task separately as it does not impl Send
        execute(async move {
            let data = read_file_slice(handle, file_range).await.expect("failed to read buffer");

            let _ = tx.send(data);
        });

        let data = rx.await.expect("failed to receive buffer");

        Ok(data)
    }
}

// Asynchronously read a slice from a JS File object
async fn read_file_slice(
    file_reference: FileReference,
    range: std::ops::Range<usize>,
) -> Result<Vec<u8>, ()> {
    let range = (range.start as u64)..(range.end as u64);
    let start = range.start;
    let end = range.end;

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
