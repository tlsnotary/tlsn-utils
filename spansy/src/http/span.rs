use std::{borrow::Cow, ops::Range};

use rangeset::set::RangeSet;

use crate::{
    ParseError, Store, View,
    http::{
        Body, BodyContent, Chunk, Code, Header, HeaderName, HeaderValue, Method, Reason, Request,
        RequestLine, Response, Status, Target,
    },
    json,
};

const MAX_HEADERS: usize = 128;

/// Parses an HTTP request.
pub fn parse_request<S: Store>(src: impl Into<View<S>>) -> Result<Request<S>, ParseError> {
    let view = src.into();
    let src = view.as_bytes();
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];

    let (method, path, head_end) = {
        let mut request = httparse::Request::new(&mut headers);

        let head_end = match request.parse(&src) {
            Ok(httparse::Status::Complete(head_end)) => head_end,
            Ok(httparse::Status::Partial) => {
                return Err(ParseError(format!("incomplete request: {:?}", &src[..])));
            }
            Err(err) => return Err(ParseError(err.to_string())),
        };

        let method = request
            .method
            .ok_or_else(|| ParseError("method missing from request".to_string()))?;

        let path = request
            .path
            .ok_or_else(|| ParseError("path missing from request".to_string()))?;

        (method, path, head_end)
    };

    let request_line_end = src
        .windows(2)
        .position(|w| w == b"\r\n")
        .expect("request line should be terminated with CRLF");
    let request_line_range = 0..request_line_end + 2;

    let headers: Vec<Header<S>> = headers
        .iter()
        .take_while(|h| *h != &httparse::EMPTY_HEADER)
        .map(|header| from_header(&view, &src, header))
        .collect();

    // httparse allocates a new buffer to store the method for performance reasons,
    // so we have to search for the span in the source. This is quick as the method
    // is at the front.
    let method_slice = src
        .windows(method.len())
        .find(|w| *w == method.as_bytes())
        .expect("method should be present");
    let method_range = get_range(&src, method_slice);

    let path_range = get_range(&src, path.as_bytes());

    let method = Method {
        view: view
            .select(method_range)
            .expect("method range should be valid")
            .try_into()
            .expect("method should be valid UTF-8"),
    };
    let target = Target {
        view: view
            .select(path_range)
            .expect("path range should be valid")
            .try_into()
            .expect("target should be valid UTF-8"),
    };
    let request_line = RequestLine {
        view: view
            .select(request_line_range)
            .expect("request line range should be valid")
            .try_into()
            .expect("request line should be valid UTF-8"),
        method,
        target,
    };

    let mut request = Request {
        view: view
            .select(0..head_end)
            .expect("head range should be valid"),
        request: request_line,
        headers,
        body: None,
    };

    let content_type: Cow<'_, [u8]> = request
        .headers_with_name("Content-Type")
        .next()
        .map(|header| header.value.as_bytes())
        .unwrap_or(Cow::Borrowed(&[]));

    match request_body_info(&request)? {
        BodyInfo::ContentLength(len) => {
            if len > 0 {
                let range = head_end..head_end + len;

                if range.end > src.len() {
                    return Err(ParseError(format!(
                        "body range {}..{} exceeds source {}",
                        range.start,
                        range.end,
                        src.len()
                    )));
                }

                request.body = Some(parse_body(&view, range.clone(), &content_type)?);
                request.view = view
                    .select(0..range.end)
                    .expect("body range should be valid");
            }
        }
        BodyInfo::Chunked => {
            let (body, consumed) = parse_chunked_body(&view, head_end, &content_type)?;
            request.body = Some(body);
            request.view = view
                .select(0..head_end + consumed)
                .expect("chunked range should be valid");
        }
        BodyInfo::None => {}
    }

    Ok(request)
}

/// Parses an HTTP response.
pub fn parse_response<S: Store>(src: impl Into<View<S>>) -> Result<Response<S>, ParseError> {
    let view = src.into();
    let src = view.as_bytes();
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];

    let (reason, code, head_end) = {
        let mut response = httparse::Response::new(&mut headers);

        let head_end = match response.parse(&src) {
            Ok(httparse::Status::Complete(head_end)) => head_end,
            Ok(httparse::Status::Partial) => {
                return Err(ParseError(format!("incomplete response: {:?}", &src[..])));
            }
            Err(err) => return Err(ParseError(err.to_string())),
        };

        let code = response
            .code
            .ok_or_else(|| ParseError("code missing from response".to_string()))
            .map(|c| c.to_string())?;

        let reason = response
            .reason
            .ok_or_else(|| ParseError("reason missing from response".to_string()))?;

        (reason, code, head_end)
    };

    let status_line_end = src
        .windows(2)
        .position(|w| w == b"\r\n")
        .expect("status line should be terminated with CRLF");
    let status_line_range = 0..status_line_end + 2;

    let headers: Vec<Header<S>> = headers
        .iter()
        .take_while(|h| *h != &httparse::EMPTY_HEADER)
        .map(|header| from_header(&view, &src, header))
        .collect();

    // httparse doesn't preserve the response code span, so we find it.
    let code_slice = src
        .windows(3)
        .find(|w| *w == code.as_bytes())
        .expect("code should be present");
    let code_range = get_range(&src, code_slice);

    let reason_range = get_range(&src, reason.as_bytes());

    let code = Code {
        view: view
            .select(code_range)
            .expect("code range should be valid")
            .try_into()
            .expect("code should be valid UTF-8"),
    };
    let reason = Reason {
        view: view
            .select(reason_range)
            .expect("reason range should be valid")
            .try_into()
            .expect("reason should be valid UTF-8"),
    };
    let status = Status {
        view: view
            .select(status_line_range)
            .expect("status line range should be valid")
            .try_into()
            .expect("status line should be valid UTF-8"),
        code,
        reason,
    };

    let mut response = Response {
        view: view
            .select(0..head_end)
            .expect("head range should be valid"),
        status,
        headers,
        body: None,
    };

    let content_type: Cow<'_, [u8]> = response
        .headers_with_name("Content-Type")
        .next()
        .map(|header| header.value.as_bytes())
        .unwrap_or(Cow::Borrowed(&[]));

    match response_body_info(&response)? {
        BodyInfo::ContentLength(len) => {
            if len > 0 {
                let range = head_end..head_end + len;

                if range.end > src.len() {
                    return Err(ParseError(format!(
                        "body range {}..{} exceeds source {}",
                        range.start,
                        range.end,
                        src.len()
                    )));
                }

                response.body = Some(parse_body(&view, range.clone(), &content_type)?);
                response.view = view
                    .select(0..range.end)
                    .expect("body range should be valid");
            }
        }
        BodyInfo::Chunked => {
            let (body, consumed) = parse_chunked_body(&view, head_end, &content_type)?;
            response.body = Some(body);
            response.view = view
                .select(0..head_end + consumed)
                .expect("chunked range should be valid");
        }
        BodyInfo::None => {}
    }

    Ok(response)
}

/// Helper to get the range of a slice within the source.
fn get_range(src: &[u8], slice: &[u8]) -> Range<usize> {
    let start = slice.as_ptr() as usize - src.as_ptr() as usize;
    start..start + slice.len()
}

/// Converts a `httparse::Header` to a `Header`.
fn from_header<S: Store>(view: &View<S>, src: &[u8], header: &httparse::Header) -> Header<S> {
    let name_range = get_range(src, header.name.as_bytes());
    let value_range = get_range(src, header.value);

    let crlf_idx = src[value_range.end..]
        .windows(2)
        .position(|b| b == b"\r\n")
        .expect("CRLF should be present in a valid header");

    // Capture the entire header including trailing whitespace and the CRLF.
    let header_range = name_range.start..value_range.end + crlf_idx + 2;

    let name = HeaderName {
        view: view
            .select(name_range)
            .expect("header name range should be valid")
            .try_into()
            .expect("header name should be valid UTF-8"),
    };
    let value = HeaderValue {
        view: view
            .select(value_range)
            .expect("header value range should be valid"),
    };
    Header {
        view: view
            .select(header_range)
            .expect("header range should be valid"),
        name,
        value,
    }
}

/// Information about how a body's length is determined.
enum BodyInfo {
    /// Body length specified by Content-Length header.
    ContentLength(usize),
    /// Body uses chunked transfer encoding.
    Chunked,
    /// No body present.
    None,
}

/// Determines body information for a request according to RFC 9112, section 6.
fn request_body_info<S: Store>(request: &Request<S>) -> Result<BodyInfo, ParseError> {
    // The presence of a message body in a request is signaled by a Content-Length
    // or Transfer-Encoding header field.

    // If a message is received with both a Transfer-Encoding and a Content-Length
    // header field, the Transfer-Encoding overrides the Content-Length
    if let Some(h) = request.headers_with_name("Transfer-Encoding").next() {
        if h.value.as_bytes().eq_ignore_ascii_case(b"chunked") {
            return Ok(BodyInfo::Chunked);
        }
        return Err(ParseError(format!(
            "unsupported Transfer-Encoding: {:?}",
            String::from_utf8_lossy(&h.value.as_bytes())
        )));
    }

    if let Some(h) = request.headers_with_name("Content-Length").next() {
        // If a valid Content-Length header field is present without Transfer-Encoding,
        // its decimal value defines the expected message body length in octets.
        let len = std::str::from_utf8(&h.value.as_bytes())?
            .parse::<usize>()
            .map_err(|err| ParseError(format!("failed to parse Content-Length value: {err}")))?;
        return Ok(BodyInfo::ContentLength(len));
    }

    // If this is a request message and none of the above are true, then the message
    // body length is zero
    Ok(BodyInfo::None)
}

/// Determines body information for a response according to RFC 9112, section 6.
fn response_body_info<S: Store>(response: &Response<S>) -> Result<BodyInfo, ParseError> {
    // Any response to a HEAD request and any response with a 1xx (Informational),
    // 204 (No Content), or 304 (Not Modified) status code is always terminated
    // by the first empty line after the header fields, regardless of the header
    // fields present in the message, and thus cannot contain a message body or
    // trailer section.
    match response
        .status
        .code
        .as_str()
        .parse::<usize>()
        .expect("code should be valid")
    {
        100..=199 | 204 | 304 => return Ok(BodyInfo::None),
        _ => {}
    }

    if let Some(h) = response.headers_with_name("Transfer-Encoding").next() {
        if h.value.as_bytes().eq_ignore_ascii_case(b"chunked") {
            return Ok(BodyInfo::Chunked);
        }
        return Err(ParseError(format!(
            "unsupported Transfer-Encoding: {:?}",
            String::from_utf8_lossy(&h.value.as_bytes())
        )));
    }

    if let Some(h) = response.headers_with_name("Content-Length").next() {
        // If a valid Content-Length header field is present without Transfer-Encoding,
        // its decimal value defines the expected message body length in octets.
        let len = std::str::from_utf8(&h.value.as_bytes())?
            .parse::<usize>()
            .map_err(|err| ParseError(format!("failed to parse Content-Length value: {err}")))?;
        return Ok(BodyInfo::ContentLength(len));
    }

    // If this is a response message and none of the above are true, then there is
    // no way to determine the length of the message body except by reading
    // it until the connection is closed.

    // We currently consider this an error because we have no outer context
    // information.
    Err(ParseError(
        "A response with a body must contain either a Content-Length or Transfer-Encoding header"
            .to_string(),
    ))
}

/// Parses a request or response message body.
fn parse_body<S: Store>(
    view: &View<S>,
    range: Range<usize>,
    content_type: &[u8],
) -> Result<Body<S>, ParseError> {
    let body_view = view
        .select(range.clone())
        .expect("body range should be valid");

    let content = if content_type.get(..16) == Some(b"application/json".as_slice()) {
        // Parse JSON directly from the body view - indices are automatically correct
        let value = json::parse(body_view.clone())?;
        BodyContent::Json(value)
    } else {
        BodyContent::Unknown
    };

    Ok(Body {
        view: body_view,
        content,
        chunks: None,
        trailers: Vec::new(),
    })
}

/// Parses a chunked transfer-encoded body.
///
/// Returns the parsed body and the total number of bytes consumed.
fn parse_chunked_body<S: Store>(
    view: &View<S>,
    start: usize,
    content_type: &[u8],
) -> Result<(Body<S>, usize), ParseError> {
    let full_data = view.as_bytes();
    let src = &full_data[start..];
    let mut pos = 0;
    let mut chunks = Vec::new();
    let mut chunk_ranges: Vec<Range<usize>> = Vec::new();

    loop {
        // Find CRLF after chunk size
        let size_end = src[pos..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| ParseError("missing CRLF after chunk size".to_string()))?;

        // Parse chunk size (hex digits, ignoring chunk extensions)
        let size_line = &src[pos..pos + size_end];
        let size_str = std::str::from_utf8(size_line)
            .map_err(|e| ParseError(format!("invalid chunk size encoding: {e}")))?;
        // Chunk extensions start with ';', take only the size part
        let size_part = size_str.split(';').next().unwrap_or(size_str).trim();
        let chunk_size = usize::from_str_radix(size_part, 16)
            .map_err(|e| ParseError(format!("invalid chunk size: {e}")))?;

        pos += size_end + 2; // skip size line and CRLF

        if chunk_size == 0 {
            // Last chunk - now parse trailers
            break;
        }

        // Record chunk data range (relative to view start)
        let data_start = start + pos;
        let data_end = data_start + chunk_size;

        chunks.push(Chunk {
            view: view
                .select(data_start..data_end)
                .expect("chunk range should be valid"),
        });
        chunk_ranges.push(data_start..data_end);

        pos += chunk_size;

        // Skip trailing CRLF after chunk data
        if src.get(pos..pos + 2) != Some(b"\r\n") {
            return Err(ParseError("missing CRLF after chunk data".to_string()));
        }
        pos += 2;
    }

    // Parse trailers (headers after last chunk)
    let (trailers, trailers_len) = parse_trailers(view, start + pos)?;
    pos += trailers_len;

    // Parse body content from assembled data
    let content =
        if !chunks.is_empty() && content_type.get(..16) == Some(b"application/json".as_slice()) {
            // Create a view of just the chunk data (non-contiguous) using absolute indices
            // from chunks
            let chunk_indices: RangeSet<usize> = chunks
                .iter()
                .flat_map(|c| c.view.indices().iter())
                .collect();
            let body_data_view = view.subview(chunk_indices);

            let value = json::parse(body_data_view)?;
            BodyContent::Json(value)
        } else {
            BodyContent::Unknown
        };

    // Body view covers the full raw chunked section (including metadata)
    let body = Body {
        view: view
            .select(start..start + pos)
            .expect("body range should be valid"),
        content,
        chunks: Some(chunks),
        trailers,
    };

    Ok((body, pos))
}

/// Parses trailer headers after a chunked body.
///
/// Returns the parsed headers and total bytes consumed.
fn parse_trailers<S: Store>(
    view: &View<S>,
    start: usize,
) -> Result<(Vec<Header<S>>, usize), ParseError> {
    let full_data = view.as_bytes();
    let src = &full_data[start..];
    let mut trailers = Vec::new();
    let mut pos = 0;

    // Parse headers until we hit an empty line (CRLF)
    loop {
        if src.get(pos..pos + 2) == Some(b"\r\n") {
            pos += 2;
            break;
        }

        // Find the end of this header line
        let line_end = src[pos..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| ParseError("missing CRLF in trailer".to_string()))?;

        let line = &src[pos..pos + line_end];

        // Find the colon separator
        let colon_pos = line
            .iter()
            .position(|&b| b == b':')
            .ok_or_else(|| ParseError("invalid trailer header: missing colon".to_string()))?;

        let mut value_start = colon_pos + 1;

        // Skip optional whitespace after colon
        while value_start < line.len() && (line[value_start] == b' ' || line[value_start] == b'\t')
        {
            value_start += 1;
        }

        // Compute ranges relative to the full view
        let base = start + pos;
        let name_range = base..base + colon_pos;
        let value_range = base + value_start..base + line_end;
        let header_range = base..base + line_end + 2;

        let name = HeaderName {
            view: view
                .select(name_range)
                .expect("trailer name range should be valid")
                .try_into()
                .expect("trailer name should be valid UTF-8"),
        };
        let value = HeaderValue {
            view: view
                .select(value_range)
                .expect("trailer value range should be valid"),
        };
        trailers.push(Header {
            view: view
                .select(header_range)
                .expect("trailer range should be valid"),
            name,
            value,
        });

        pos += line_end + 2;
    }

    Ok((trailers, pos))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Span;

    const TEST_REQUEST: &[u8] = b"\
                        GET /home.html HTTP/1.1\r\n\
                        Host: developer.mozilla.org\r\n\
                        User-Agent: Mozilla/5.0 (Macintosh; Intel Mac OS X 10.9; rv:50.0) Gecko/20100101 Firefox/50.0\r\n\
                        Accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.\r\n\
                        Accept-Language: en-US,en;q=0.\r\n\
                        Accept-Encoding: gzip, deflate, b\r\n\
                        Referer: https://developer.mozilla.org/testpage.htm\r\n\
                        Connection: keep-alive\r\n\
                        Content-Length: 12\r\n\
                        Cache-Control: max-age=0\r\n\r\n\
                        Hello World!";

    const TEST_RESPONSE: &[u8] = b"\
                        HTTP/1.1 200 OK\r\n\
                        Date: Mon, 27 Jul 2009 12:28:53 GMT\r\n\
                        Server: Apache/2.2.14 (Win32)\r\n\
                        Last-Modified: Wed, 22 Jul 2009 19:15:56 GMT\r\n\
                        Content-Length: 52\r\n\
                        Content-Type: text/html\r\n\
                        Connection: Closed\r\n\r\n\
                        <html>\n\
                        <body>\n\
                        <h1>Hello, World!</h1>\n\
                        </body>\n\
                        </html>";

    const TEST_REQUEST_JSON: &[u8] = b"\
                        POST / HTTP/1.1\r\n\
                        Host: localhost\r\n\
                        Content-Type: application/json\r\n\
                        Content-Length: 14\r\n\r\n\
                        {\"foo\": \"bar\"}";

    const TEST_RESPONSE_JSON: &[u8] = b"\
                        HTTP/1.1 200 OK\r\n\
                        Content-Type: application/json\r\n\
                        Content-Length: 14\r\n\r\n\
                        {\"foo\": \"bar\"}";

    #[test]
    fn test_parse_request() {
        let req = parse_request(TEST_REQUEST).unwrap();

        assert_eq!(req.data(), TEST_REQUEST);
        assert_eq!(req.request.method.as_str(), "GET");
        assert_eq!(
            req.headers_with_name("Host")
                .next()
                .unwrap()
                .value
                .as_bytes(),
            b"developer.mozilla.org".as_slice()
        );
        assert_eq!(
            req.headers_with_name("User-Agent")
                .next()
                .unwrap()
                .value
                .as_bytes(),
            b"Mozilla/5.0 (Macintosh; Intel Mac OS X 10.9; rv:50.0) Gecko/20100101 Firefox/50.0"
                .as_slice()
        );
        assert_eq!(req.body.unwrap(), b"Hello World!".as_slice());
    }

    #[test]
    fn test_parse_request_borrowed() {
        let src: &[u8] = TEST_REQUEST;
        let req = parse_request(src).unwrap();

        assert_eq!(req, TEST_REQUEST);
        assert_eq!(req.request.method.as_str().as_ref(), "GET");
        assert!(req.view.is_contiguous());
    }

    #[test]
    fn test_parse_header_trailing_whitespace() {
        let req = parse_request(b"GET / HTTP/1.1\r\nHost: example.com \r\n\r\n").unwrap();
        let header = req.headers_with_name("Host").next().unwrap();

        assert_eq!(header, b"Host: example.com \r\n".as_slice());
    }

    #[test]
    fn test_parse_response() {
        let res = parse_response(TEST_RESPONSE).unwrap();

        assert_eq!(res, TEST_RESPONSE);
        assert_eq!(res.status.code.as_str().as_ref(), "200");
        assert_eq!(res.status.reason.as_str().as_ref(), "OK");
        assert_eq!(
            res.headers_with_name("Server")
                .next()
                .unwrap()
                .value
                .as_bytes()
                .as_ref(),
            b"Apache/2.2.14 (Win32)".as_slice()
        );
        assert_eq!(
            res.headers_with_name("Connection")
                .next()
                .unwrap()
                .value
                .as_bytes()
                .as_ref(),
            b"Closed".as_slice()
        );
        assert_eq!(
            res.body.unwrap(),
            b"<html>\n<body>\n<h1>Hello, World!</h1>\n</body>\n</html>".as_slice()
        );
    }

    #[test]
    fn test_parse_request_json() {
        let req = parse_request(TEST_REQUEST_JSON).unwrap();

        let BodyContent::Json(value) = req.body.unwrap().content else {
            panic!("body should be json");
        };

        assert_eq!(value, "{\"foo\": \"bar\"}");
    }

    #[test]
    fn test_parse_response_json() {
        let res = parse_response(TEST_RESPONSE_JSON).unwrap();

        let BodyContent::Json(value) = res.body.unwrap().content else {
            panic!("body should be json");
        };

        assert_eq!(value, "{\"foo\": \"bar\"}");
    }

    #[test]
    fn test_chunked_request() {
        let src = b"POST / HTTP/1.1\r\n\
            Transfer-Encoding: chunked\r\n\
            Content-Type: text/plain\r\n\r\n\
            5\r\nhello\r\n\
            6\r\n world\r\n\
            0\r\n\r\n";

        let req = parse_request(src).unwrap();
        let body = req.body.unwrap();

        assert_eq!(body.content_data().as_ref(), b"hello world".as_slice());
        assert_eq!(body.chunks.as_ref().unwrap().len(), 2);
        assert!(body.trailers.is_empty());
    }

    #[test]
    fn test_chunked_response() {
        let src = b"HTTP/1.1 200 OK\r\n\
            Transfer-Encoding: chunked\r\n\
            Content-Type: text/plain\r\n\r\n\
            7\r\nMozilla\r\n\
            9\r\nDeveloper\r\n\
            0\r\n\r\n";

        let res = parse_response(src).unwrap();
        let body = res.body.unwrap();

        assert_eq!(body.content_data().as_ref(), b"MozillaDeveloper".as_slice());
        assert_eq!(body.chunks.as_ref().unwrap().len(), 2);
        assert!(body.trailers.is_empty());
    }

    #[test]
    fn test_chunked_response_json() {
        let src = b"HTTP/1.1 200 OK\r\n\
            Transfer-Encoding: chunked\r\n\
            Content-Type: application/json\r\n\r\n\
            e\r\n{\"foo\": \"bar\"}\r\n\
            0\r\n\r\n";

        let res = parse_response(src).unwrap();
        let body = res.body.unwrap();

        let BodyContent::Json(value) = body.content else {
            panic!("body should be json");
        };

        assert_eq!(value, "{\"foo\": \"bar\"}");
    }

    #[test]
    fn test_chunked_response_json_split_inside_tokens() {
        let src = b"HTTP/1.1 200 OK\r\n\
            Transfer-Encoding: chunked\r\n\
            Content-Type: application/json\r\n\r\n\
            c\r\n{\"foo\": \"abc\r\n\
            12\r\ndef\", \"baz\": 12345\r\n\
            5\r\n6789}\r\n\
            0\r\n\r\n";

        let res = parse_response(src).unwrap();
        let body = res.body.unwrap();

        let BodyContent::Json(value) = body.content else {
            panic!("body should be json");
        };

        assert_eq!(value, "{\"foo\": \"abcdef\", \"baz\": 123456789}");
    }

    #[test]
    fn test_chunked_with_trailers() {
        let src = b"HTTP/1.1 200 OK\r\n\
            Transfer-Encoding: chunked\r\n\r\n\
            5\r\nhello\r\n\
            0\r\n\
            X-Checksum: abc123\r\n\
            \r\n";

        let res = parse_response(src).unwrap();
        let body = res.body.unwrap();

        assert_eq!(body.content_data().as_ref(), b"hello".as_slice());
        assert_eq!(body.trailers.len(), 1);
        assert_eq!(body.trailers[0].name.as_str().as_ref(), "X-Checksum");
        assert_eq!(
            body.trailers[0].value.as_bytes().as_ref(),
            b"abc123".as_slice()
        );
    }

    #[test]
    fn test_complete_coverage_no_body() {
        let src = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let req = parse_request(src).unwrap();
        assert_eq!(req.indices(), &RangeSet::from(0..src.len()));
    }

    #[test]
    fn test_complete_coverage_with_body() {
        let src = b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello";
        let req = parse_request(src).unwrap();
        assert_eq!(req.indices(), &RangeSet::from(0..src.len()));
    }

    #[test]
    fn test_complete_coverage_chunked() {
        let src = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        let res = parse_response(src).unwrap();
        assert_eq!(res.indices(), &RangeSet::from(0..src.len()));
    }

    #[test]
    fn test_chunked_content_data() {
        let src = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let res = parse_response(src).unwrap();
        let body = res.body.unwrap();

        // data() returns raw bytes including chunk metadata
        assert!(body.data().len() > b"hello world".len());

        // content_data() returns assembled chunk content
        assert_eq!(body.content_data().as_ref(), b"hello world".as_slice());
    }

    #[test]
    fn test_invalid_chunked_response() {
        // Missing CRLF after chunk data
        let src = b"HTTP/1.1 200 OK\r\n\
            Transfer-Encoding: chunked\r\n\r\n\
            5\r\nhelloX\r\n\
            0\r\n\r\n";
        let err = parse_response(src).unwrap_err();
        assert!(err.0.contains("missing CRLF after chunk data"), "{}", err.0);

        // Invalid hex in chunk size
        let src = b"HTTP/1.1 200 OK\r\n\
            Transfer-Encoding: chunked\r\n\r\n\
            zz\r\nhello\r\n\
            0\r\n\r\n";
        let err = parse_response(src).unwrap_err();
        assert!(err.0.contains("invalid chunk size"), "{}", err.0);

        // Missing terminating chunk
        let src = b"HTTP/1.1 200 OK\r\n\
            Transfer-Encoding: chunked\r\n\r\n\
            5\r\nhello\r\n";
        let err = parse_response(src).unwrap_err();
        assert!(err.0.contains("missing CRLF after chunk size"), "{}", err.0);
    }
}
