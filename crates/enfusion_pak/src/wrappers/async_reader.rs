use std::collections::HashMap;
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use crate::PakFile;
use crate::PakParser;
use crate::ParserStateMachine;
use crate::Stream;
use crate::async_pak_vfs::AsyncPrime;
use crate::async_pak_vfs::AsyncReadAt;
use crate::pak_vfs::Prime;
use crate::winnow::stream::Offset;
use crate::winnow::stream::Stream as _;
use async_trait::async_trait;
use log::debug;
use vfs::VfsError;

/// An async wrapper around a PakFile and its data source which caches reads
#[allow(unused)]
pub struct CachingAsyncPakFileWrapper<T> {
    path: PathBuf,
    handle: T,
    buffer: Mutex<HashMap<std::ops::Range<usize>, BufferWrapper>>,
    pak_file: PakFile,
}

impl<T> CachingAsyncPakFileWrapper<T> {
    pub fn new(path: PathBuf, handle: T, pak_file: PakFile) -> Self {
        Self { path, handle, buffer: Default::default(), pak_file }
    }
}

impl<T> Debug for CachingAsyncPakFileWrapper<T>
where
    T: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachingAsyncPakFileWrapper")
            .field("path", &self.path)
            .field("handle", &self.handle)
            .field("buffer", &self.buffer)
            .field("pak_file", &self.pak_file)
            .finish()
    }
}

impl<T> AsRef<PakFile> for CachingAsyncPakFileWrapper<T> {
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

impl<T> Prime for CachingAsyncPakFileWrapper<T> {
    fn prime_file(
        &self,
        _file_range: std::ops::Range<usize>,
    ) -> Result<impl AsRef<[u8]>, VfsError> {
        Ok(BufferWrapper(Arc::new(oval::Buffer::with_capacity(0))))
    }
}

#[async_trait]
impl<T> AsyncPrime for CachingAsyncPakFileWrapper<T>
where
    T: AsyncReadAt + Clone + Send + Sync + 'static,
{
    async fn prime_file(
        &self,
        file_range: std::ops::Range<usize>,
    ) -> Result<impl AsRef<[u8]>, VfsError> {
        debug!("attempting to prime file");
        {
            let buffers = self.buffer.lock().unwrap();

            if let Some(entry) = buffers.get(&file_range) {
                return Ok(entry.clone());
            }
        }

        let data = self.handle.read_at(file_range.clone()).await?;

        let file_size = file_range.end - file_range.start;
        let mut buffer = oval::Buffer::with_capacity(file_size);
        let mut data: &[u8] = data.as_ref();
        let mut buffer_slice = buffer.space();
        let read = std::io::copy(&mut data, &mut buffer_slice).expect("failed to copy to buffer");
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

        let entry = buffers.entry(file_range.clone()).insert_entry(BufferWrapper(Arc::new(buffer)));

        Ok(entry.get().clone())
    }
}

pub async fn parse_pak_file<T>(
    path: PathBuf,
    file_handle: T,
) -> Result<CachingAsyncPakFileWrapper<T>, VfsError>
where
    T: AsyncReadAt + Clone + Send + Sync + 'static,
{
    let mut parser = PakParser::new();

    // 64k buffer size
    let mut buffer = oval::Buffer::with_capacity(1024 * 64);

    loop {
        // Populate the buffer with the first 16k
        //
        // TODO: fix this so we only load the minimum amount of data
        let read_range = parser.bytes_parsed()..(parser.bytes_parsed() + buffer.capacity());
        let read_handle = file_handle.clone();
        let data = read_handle.read_at(read_range).await?;

        buffer.reset();

        let mut data_slice: &[u8] = data.as_ref();
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
                return Ok(CachingAsyncPakFileWrapper {
                    path,
                    handle: file_handle,
                    buffer: Default::default(),
                    pak_file,
                });
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
                panic!("error reading pak file: {e:?}")
            }
        }
    }
}
