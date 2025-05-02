use std::fmt::Debug;

use winnow::stream::{Location, Offset, SliceLen, Stream, StreamIsPartial};

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SkippableStream<I> {
    inner: I,
}

impl<I> std::ops::Deref for SkippableStream<I> {
    type Target = I;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<I> Location for SkippableStream<I>
where
    I: Location,
{
    fn previous_token_end(&self) -> usize {
        self.inner.previous_token_end()
    }

    fn current_token_start(&self) -> usize {
        self.inner.current_token_start()
    }
}

impl<I> SliceLen for SkippableStream<I>
where
    I: SliceLen,
{
    fn slice_len(&self) -> usize {
        self.inner.slice_len()
    }
}

impl<I: Stream> Stream for SkippableStream<I> {
    type Token = <I as Stream>::Token;

    type Slice = <I as Stream>::Slice;

    type IterOffsets = <I as Stream>::IterOffsets;

    type Checkpoint = <I as Stream>::Checkpoint;

    #[inline(always)]
    fn iter_offsets(&self) -> Self::IterOffsets {
        self.inner.iter_offsets()
    }

    #[inline(always)]
    fn eof_offset(&self) -> usize {
        self.inner.eof_offset()
    }

    #[inline(always)]
    fn next_token(&mut self) -> Option<Self::Token> {
        self.inner.next_token()
    }

    #[inline(always)]
    fn peek_token(&self) -> Option<Self::Token> {
        self.inner.peek_token()
    }

    #[inline(always)]
    fn offset_for<P>(&self, predicate: P) -> Option<usize>
    where
        P: Fn(Self::Token) -> bool,
    {
        self.inner.offset_for(predicate)
    }

    #[inline(always)]
    fn offset_at(&self, tokens: usize) -> Result<usize, winnow::error::Needed> {
        self.inner.offset_at(tokens)
    }

    #[inline(always)]
    fn next_slice(&mut self, offset: usize) -> Self::Slice {
        self.inner.next_slice(offset)
    }

    fn peek_slice(&self, offset: usize) -> Self::Slice {
        self.inner.peek_slice(offset)
    }

    fn checkpoint(&self) -> Self::Checkpoint {
        self.inner.checkpoint()
    }

    fn reset(&mut self, checkpoint: &Self::Checkpoint) {
        self.inner.reset(checkpoint);
    }

    fn raw(&self) -> &dyn Debug {
        &self.inner
    }

    unsafe fn next_slice_unchecked(&mut self, offset: usize) -> Self::Slice {
        // SAFETY: the inner takes care of invariants
        unsafe { self.inner.next_slice_unchecked(offset) }
    }

    unsafe fn peek_slice_unchecked(&self, offset: usize) -> Self::Slice {
        // SAFETY: the inner takes care of invariants
        unsafe { self.inner.peek_slice_unchecked(offset) }
    }

    fn finish(&mut self) -> Self::Slice {
        self.inner.finish()
    }
}

impl<I> StreamIsPartial for SkippableStream<I>
where
    I: StreamIsPartial,
{
    type PartialState = <I as StreamIsPartial>::PartialState;

    #[inline]
    fn complete(&mut self) -> Self::PartialState {
        self.inner.complete()
    }

    #[inline]
    fn restore_partial(&mut self, state: Self::PartialState) {
        self.inner.restore_partial(state);
    }

    #[inline(always)]
    fn is_partial_supported() -> bool {
        true
    }

    #[inline(always)]
    fn is_partial(&self) -> bool {
        self.inner.is_partial()
    }
}

impl<I> Offset<<SkippableStream<I> as Stream>::Checkpoint> for SkippableStream<I>
where
    I: Stream,
{
    #[inline(always)]
    fn offset_from(&self, other: &<SkippableStream<I> as Stream>::Checkpoint) -> usize {
        self.checkpoint().offset_from(other)
    }
}
