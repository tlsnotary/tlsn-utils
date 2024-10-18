use crate::ParseError;
use bytes::{Bytes, BytesMut};
use flate2::read::{GzDecoder, DeflateDecoder};
use std::io::Read;

// Parsing functions for Transfer-Encoding header types
/// Parse Transfer-Encoding: chunked body
pub fn parse_chunked_body(src: &Bytes, offset: usize) -> Result<(Bytes, usize), ParseError> {
    let mut body = BytesMut::new();
    let mut pos = offset;

    loop {
        let chunk_size_end = src[pos..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| ParseError("Invalid chunked encoding: missing chunk size CRLF".to_string()))?
            + pos;
        
        let chunk_size_str = std::str::from_utf8(&src[pos..chunk_size_end])
            .map_err(|_| ParseError("Invalid chunk size encoding".to_string()))?;
        let chunk_size = usize::from_str_radix(chunk_size_str.trim(), 16)
            .map_err(|_| ParseError("Invalid chunk size value".to_string()))?;
        
        pos = chunk_size_end + 2;
        
        if chunk_size == 0 {
            break;
        }
        
        let chunk_data_end = pos + chunk_size;
        if chunk_data_end > src.len() {
            return Err(ParseError("Chunk data exceeds source length".to_string()));
        }
        body.extend_from_slice(&src[pos..chunk_data_end]);
        
        pos = chunk_data_end + 2;
    }
    
    pos += 2;
    
    Ok((body.freeze(), pos))
}

/// Parse Transfer-Encoding: gzip body
pub fn parse_gzip_body(src: &Bytes) -> Result<Bytes, ParseError> {
    let mut decoder = GzDecoder::new(&src[..]);
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed).map_err(|e| ParseError(format!("Failed to decompress gzip body: {}", e)))?;
    Ok(Bytes::from(decompressed))
}

/// Parse Transfer-Encoding: deflate body
pub fn parse_deflate_body(src: &Bytes) -> Result<Bytes, ParseError> {
    let mut decoder = DeflateDecoder::new(&src[..]);
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed).map_err(|e| ParseError(format!("Failed to decompress deflate body: {}", e)))?;
    Ok(Bytes::from(decompressed))
}

/// Parse Transfer-Encoding: identity body
pub fn parse_identity_body(src: &Bytes) -> Result<Bytes, ParseError> {
    Ok(src.clone())
}