use crate::ParseError;
use bytes::{Bytes, BytesMut};
use flate2::read::{GzDecoder, DeflateDecoder};
use std::io::Read;

// Parsing functions for Transfer-Encoding header types
// Parse Transfer-Encoding: chunked body
fn parse_chunked_body(src: &Bytes, offset: usize) -> Result<(Bytes, usize), ParseError> {
    let mut body = BytesMut::new();
    let mut pos = offset;

    loop {
        // Find the end of the chunk size line
        let chunk_size_end = src[pos..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| ParseError("Invalid chunked encoding: missing chunk size CRLF".to_string()))?
            + pos;
        
        // Parse the chunk size
        let chunk_size_str = std::str::from_utf8(&src[pos..chunk_size_end])
            .map_err(|_| ParseError("Invalid chunk size encoding".to_string()))?;
        let chunk_size = usize::from_str_radix(chunk_size_str.trim(), 16)
            .map_err(|_| ParseError("Invalid chunk size value".to_string()))?;
        
        // Move past the chunk size line
        pos = chunk_size_end + 2;
        
        // If chunk size is zero, this is the last chunk
        if chunk_size == 0 {
            break;
        }
        
        // Extract the chunk data
        let chunk_data_end = pos + chunk_size;
        if chunk_data_end > src.len() {
            return Err(ParseError("Chunk data exceeds source length".to_string()));
        }
        body.extend_from_slice(&src[pos..chunk_data_end]);
        
        // Move past the chunk data and the trailing CRLF
        pos = chunk_data_end + 2;
    }
    
    // Move past the final CRLF after the last chunk
    pos += 2;
    
    Ok((body.freeze(), pos))
}

fn parse_gzip_body(src: &Bytes) -> Result<Bytes, ParseError> {
    let mut decoder = GzDecoder::new(&src[..]);
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed).map_err(|e| ParseError(format!("Failed to decompress gzip body: {}", e)))?;
    Ok(Bytes::from(decompressed))
}

fn parse_deflate_body(src: &Bytes) -> Result<Bytes, ParseError> {
    let mut decoder = DeflateDecoder::new(&src[..]);
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed).map_err(|e| ParseError(format!("Failed to decompress deflate body: {}", e)))?;
    Ok(Bytes::from(decompressed))
}

fn parse_idenity_body() {}