// Copyright (c) 2019 Parity Technologies (UK) Ltd.
// Modifications Copyright (c) 2026 TLSNotary
//
// Licensed under the Apache License, Version 2.0 or MIT license, at your
// option.
//
// A copy of the Apache License, Version 2.0 is included in the software as
// LICENSE-APACHE and a copy of the MIT license is included in the software
// as LICENSE-MIT. You may also obtain a copy of the Apache License, Version 2.0
// at https://www.apache.org/licenses/LICENSE-2.0 and a copy of the MIT license
// at https://opensource.org/licenses/MIT.

use std::{collections::VecDeque, io};

/// An element in the buffer - either data or a FIN marker.
#[derive(Debug)]
pub(crate) enum ChunkOrFin {
    Chunk(Chunk),
    Fin,
}

/// A sequence of [`ChunkOrFin`] values.
///
/// [`Chunks::len`] considers all [`Chunk`] elements and computes the total
/// result, i.e. the length of all bytes, by summing up the lengths of all
/// [`Chunk`] elements. FIN markers don't contribute to length.
#[derive(Debug)]
pub(crate) struct Chunks {
    seq: VecDeque<ChunkOrFin>,
    len: usize,
}

impl Chunks {
    /// A new empty chunk list.
    pub(crate) fn new() -> Self {
        Chunks {
            seq: VecDeque::new(),
            len: 0,
        }
    }

    /// The total length of bytes yet-to-be-read in all `Chunk`s.
    pub(crate) fn len(&self) -> usize {
        let front_offset = self
            .seq
            .front()
            .and_then(|e| match e {
                ChunkOrFin::Chunk(c) => Some(c.offset()),
                ChunkOrFin::Fin => None,
            })
            .unwrap_or(0);
        self.len - front_offset
    }

    /// Returns true if there is no data in the buffer.
    ///
    /// Note: A buffer with only FIN markers is considered empty.
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Add another chunk of bytes to the end.
    pub(crate) fn push(&mut self, x: Vec<u8>) {
        self.len += x.len();
        if !x.is_empty() {
            self.seq.push_back(ChunkOrFin::Chunk(Chunk {
                cursor: io::Cursor::new(x),
            }))
        }
    }

    /// Add a FIN marker to the end.
    pub(crate) fn push_fin(&mut self) {
        self.seq.push_back(ChunkOrFin::Fin);
    }

    /// Remove and return the first element.
    pub(crate) fn pop(&mut self) -> Option<ChunkOrFin> {
        let elem = self.seq.pop_front();
        if let Some(ChunkOrFin::Chunk(ref c)) = elem {
            self.len -= c.len() + c.offset();
        }
        elem
    }

    /// Get a reference to the first element.
    pub(crate) fn front(&self) -> Option<&ChunkOrFin> {
        self.seq.front()
    }

    /// Get a mutable reference to the first chunk, if it is a chunk.
    ///
    /// Returns None if buffer is empty or front is a FIN marker.
    pub(crate) fn front_chunk_mut(&mut self) -> Option<&mut Chunk> {
        match self.seq.front_mut() {
            Some(ChunkOrFin::Chunk(c)) => Some(c),
            _ => None,
        }
    }
}

/// A `Chunk` wraps a `std::io::Cursor<Vec<u8>>`.
///
/// It provides a byte-slice view and a way to advance the cursor so the
/// vector can be consumed in steps.
#[derive(Debug)]
pub(crate) struct Chunk {
    cursor: io::Cursor<Vec<u8>>,
}

impl Chunk {
    /// Is this chunk empty?
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The remaining number of bytes in this `Chunk`.
    pub(crate) fn len(&self) -> usize {
        self.cursor.get_ref().len() - self.offset()
    }

    /// The sum of bytes that the cursor has been `advance`d over.
    pub(crate) fn offset(&self) -> usize {
        self.cursor.position() as usize
    }

    /// Move the cursor position by `amount` bytes.
    ///
    /// The `AsRef<[u8]>` impl of `Chunk` provides a byte-slice view
    /// from the current position to the end.
    pub(crate) fn advance(&mut self, amount: usize) {
        assert!({
            // the new position must not exceed the vector's length
            let pos = self.offset().checked_add(amount);
            let max = self.cursor.get_ref().len();
            pos.is_some() && pos <= Some(max)
        });

        self.cursor
            .set_position(self.cursor.position() + amount as u64);
    }
}

impl AsRef<[u8]> for Chunk {
    fn as_ref(&self) -> &[u8] {
        &self.cursor.get_ref()[self.offset()..]
    }
}
