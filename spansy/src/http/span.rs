use std::ops::Range;

use bytes::Bytes;

use crate::{
    helpers::get_span_range,
    http::{
        Body, BodyContent, Code, Header, HeaderName, HeaderValue, Method, Reason, Request,
        RequestLine, Response, Status, Target,
    },
    json, ParseError, Span,
};

const MAX_HEADERS: usize = 128;

/// Parses an HTTP request.
pub fn parse_request(src: &[u8]) -> Result<Request, ParseError> {
    parse_request_from_bytes(&Bytes::copy_from_slice(src), 0)
}

/// Parses an HTTP request from a `Bytes` buffer starting from the `offset`.
pub(crate) fn parse_request_from_bytes(src: &Bytes, offset: usize) -> Result<Request, ParseError> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];

    let (method, path, head_end) = {
        let mut request = httparse::Request::new(&mut headers);

        let head_end = match request.parse(&src[offset..]) {
            Ok(httparse::Status::Complete(head_end)) => head_end + offset,
            Ok(httparse::Status::Partial) => {
                return Err(ParseError(format!("incomplete request: {:?}", src)))
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

    let request_line_end = src[offset..]
        .windows(2)
        .position(|w| w == b"\r\n")
        .expect("request line is terminated with CRLF");
    let request_line_range = offset..offset + request_line_end + 2;

    let headers = headers
        .iter()
        .take_while(|h| *h != &httparse::EMPTY_HEADER)
        .map(|header| from_header(src, header))
        .collect();

    // httparse allocates a new buffer to store the method for performance reasons,
    // so we have to search for the span in the source. This is quick as the method
    // is at the front.
    let method = src[offset..]
        .windows(method.len())
        .find(|w| *w == method.as_bytes())
        .expect("method is present");

    let mut request = Request {
        span: Span::new_bytes(src.clone(), offset..head_end),
        request: RequestLine {
            span: Span::new_str(src.clone(), request_line_range),
            method: Method(Span::new_str(src.clone(), get_span_range(src, method))),
            target: Target(Span::new_from_str(src.clone(), path)),
        },
        headers,
        body: None,
    };

    let body_len = request_body_len(&request)?;

    if body_len > 0 {
        let range = head_end..head_end + body_len;

        if range.end > src.len() {
            return Err(ParseError(format!(
                "body range {}..{} exceeds source {}",
                range.start,
                range.end,
                src.len()
            )));
        }

        let content_type = request
            .headers_with_name("Content-Type")
            .next()
            .map(|header| header.value.as_bytes())
            .unwrap_or_default();

        request.body = Some(parse_body(src, range.clone(), content_type)?);
        request.span = Span::new_bytes(src.clone(), offset..range.end);
    }

    Ok(request)
}

/// Parses an HTTP response.
pub fn parse_response(src: &[u8]) -> Result<Response, ParseError> {
    parse_response_from_bytes(&Bytes::copy_from_slice(src), 0)
}

/// Parses an HTTP response from a `Bytes` buffer starting from the `offset`.
pub(crate) fn parse_response_from_bytes(
    src: &Bytes,
    offset: usize,
) -> Result<Response, ParseError> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];

    let (reason, code, head_end) = {
        let mut response = httparse::Response::new(&mut headers);

        let head_end = match response.parse(&src[offset..]) {
            Ok(httparse::Status::Complete(head_end)) => head_end + offset,
            Ok(httparse::Status::Partial) => {
                return Err(ParseError(format!("incomplete response: {:?}", src)))
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

    let status_line_end = src[offset..]
        .windows(2)
        .position(|w| w == b"\r\n")
        .expect("status line is terminated with CRLF");
    let status_line_range = offset..offset + status_line_end + 2;

    let headers = headers
        .iter()
        .take_while(|h| *h != &httparse::EMPTY_HEADER)
        .map(|header| from_header(src, header))
        .collect();

    // httparse doesn't preserve the response code span, so we find it.
    let code = src[offset..]
        .windows(3)
        .find(|w| *w == code.as_bytes())
        .expect("code is present");

    let mut response = Response {
        span: Span::new_bytes(src.clone(), offset..head_end),
        status: Status {
            span: Span::new_str(src.clone(), status_line_range),
            code: Code(Span::new_str(src.clone(), get_span_range(src, code))),
            reason: Reason(Span::new_from_str(src.clone(), reason)),
        },
        headers,
        body: None,
    };

    let body_len = response_body_len(&response)?;

    if body_len > 0 {
        let range = head_end..head_end + body_len;

        if range.end > src.len() {
            return Err(ParseError(format!(
                "body range {}..{} exceeds source {}",
                range.start,
                range.end,
                src.len()
            )));
        }

        let content_type = response
            .headers_with_name("Content-Type")
            .next()
            .map(|header| header.value.as_bytes())
            .unwrap_or_default();

        response.body = Some(parse_body(src, range.clone(), content_type)?);
        response.span = Span::new_bytes(src.clone(), offset..range.end);
    }

    Ok(response)
}

/// Converts a `httparse::Header` to a `Header`.
fn from_header(src: &Bytes, header: &httparse::Header) -> Header {
    let name_range = get_span_range(src, header.name.as_bytes());
    let value_range = get_span_range(src, header.value);

    let crlf_idx = src[value_range.end..]
        .windows(2)
        .position(|b| b == b"\r\n")
        .expect("CRLF is present in a valid header");

    // Capture the entire header including trailing whitespace and the CRLF.
    let header_range = name_range.start..value_range.end + crlf_idx + 2;

    Header {
        span: Span::new_bytes(src.clone(), header_range),
        name: HeaderName(Span::new_str(src.clone(), name_range)),
        value: HeaderValue(Span::new_bytes(src.clone(), value_range)),
    }
}

/// Calculates the length of the request body according to RFC 9112, section 6.
fn request_body_len(request: &Request) -> Result<usize, ParseError> {
    // The presence of a message body in a request is signaled by a Content-Length
    // or Transfer-Encoding header field.

    // If a message is received with both a Transfer-Encoding and a Content-Length header field,
    // the Transfer-Encoding overrides the Content-Length
    if request
        .headers_with_name("Transfer-Encoding")
        .next()
        .is_some()
    {
        Err(ParseError(
            "Transfer-Encoding not supported yet".to_string(),
        ))
    } else if let Some(h) = request.headers_with_name("Content-Length").next() {
        // If a valid Content-Length header field is present without Transfer-Encoding, its decimal value
        // defines the expected message body length in octets.
        std::str::from_utf8(h.value.0.as_bytes())?
            .parse::<usize>()
            .map_err(|err| ParseError(format!("failed to parse Content-Length value: {err}")))
    } else {
        // If this is a request message and none of the above are true, then the message body length is zero
        Ok(0)
    }
}

/// Calculates the length of the response body according to RFC 9112, section 6.
fn response_body_len(response: &Response) -> Result<usize, ParseError> {
    // Any response to a HEAD request and any response with a 1xx (Informational), 204 (No Content), or 304 (Not Modified)
    // status code is always terminated by the first empty line after the header fields, regardless of the header fields
    // present in the message, and thus cannot contain a message body or trailer section.
    match response
        .status
        .code
        .as_str()
        .parse::<usize>()
        .expect("code is valid utf-8")
    {
        100..=199 | 204 | 304 => return Ok(0),
        _ => {}
    }

    if response
        .headers_with_name("Transfer-Encoding")
        .next()
        .is_some()
    {
        Err(ParseError(
            "Transfer-Encoding not supported yet".to_string(),
        ))
    } else if let Some(h) = response.headers_with_name("Content-Length").next() {
        // If a valid Content-Length header field is present without Transfer-Encoding, its decimal value
        // defines the expected message body length in octets.
        std::str::from_utf8(h.value.0.as_bytes())?
            .parse::<usize>()
            .map_err(|err| ParseError(format!("failed to parse Content-Length value: {err}")))
    } else {
        // If this is a response message and none of the above are true, then there is no way to
        // determine the length of the message body except by reading it until the connection is closed.

        // We currently consider this an error because we have no outer context information.
        Err(ParseError(
            "A response with a body must contain either a Content-Length or Transfer-Encoding header".to_string(),
        ))
    }
}

/// Parses a request or response message body.
///
/// # Arguments
///
/// * `src` - The source bytes.
/// * `range` - The range of the message body in the source bytes.
/// * `content_type` - The value of the Content-Type header.
fn parse_body(src: &Bytes, range: Range<usize>, content_type: &[u8]) -> Result<Body, ParseError> {
    let span = Span::new_bytes(src.clone(), range.clone());
    let content = if content_type.get(..16) == Some(b"application/json".as_slice()) {
        let mut value = json::parse(span.data.clone())?;
        value.offset(range.start);

        BodyContent::Json(value)
    } else {
        BodyContent::Unknown(span.clone())
    };

    Ok(Body { span, content })
}

#[cfg(test)]
mod tests {
    use crate::Spanned;

    use super::*;

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

    const TEST_REQUEST2: &[u8] = b"\
                        GET /info.html HTTP/1.1\r\n\
                        Host: tlsnotary.org\r\n\
                        User-Agent: client\r\n\
                        Content-Length: 4\r\n\r\n\
                        ping";

    const TEST_RESPONSE2: &[u8] = b"\
                        HTTP/1.1 200 OK\r\n\
                        Server: server\r\n\
                        Content-Length: 4\r\n\
                        Content-Type: text/plain\r\n\
                        Connection: keep-alive\r\n\r\n\
                        pong";

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

        assert_eq!(req.span(), TEST_REQUEST);
        assert_eq!(req.request.method.as_str(), "GET");
        assert_eq!(
            req.headers_with_name("Host").next().unwrap().value.span(),
            b"developer.mozilla.org".as_slice()
        );
        assert_eq!(
            req.headers_with_name("User-Agent")
                .next()
                .unwrap()
                .value
                .span(),
            b"Mozilla/5.0 (Macintosh; Intel Mac OS X 10.9; rv:50.0) Gecko/20100101 Firefox/50.0"
                .as_slice()
        );
        assert_eq!(req.body.unwrap().span(), b"Hello World!".as_slice());
    }

    #[test]
    fn test_parse_header_trailing_whitespace() {
        let req = parse_request(b"GET / HTTP/1.1\r\nHost: example.com \r\n\r\n").unwrap();
        let header = req.headers_with_name("Host").next().unwrap();

        assert_eq!(header.span.as_bytes(), b"Host: example.com \r\n".as_slice());
    }

    #[test]
    fn test_parse_response() {
        let res = parse_response(TEST_RESPONSE).unwrap();

        assert_eq!(res.span(), TEST_RESPONSE);
        assert_eq!(res.status.code.as_str(), "200");
        assert_eq!(res.status.reason.as_str(), "OK");
        assert_eq!(
            res.headers_with_name("Server").next().unwrap().value.span(),
            b"Apache/2.2.14 (Win32)".as_slice()
        );
        assert_eq!(
            res.headers_with_name("Connection")
                .next()
                .unwrap()
                .value
                .span(),
            b"Closed".as_slice()
        );
        assert_eq!(
            res.body.unwrap().span(),
            b"<html>\n<body>\n<h1>Hello, World!</h1>\n</body>\n</html>".as_slice()
        );
    }

    // Make sure the first request is not parsed.
    #[test]
    fn test_parse_request_from_bytes() {
        let mut request = Vec::new();
        request.extend(TEST_REQUEST2);
        request.extend(TEST_REQUEST);
        let request = Bytes::copy_from_slice(&request);
        let req = parse_request_from_bytes(&request, TEST_REQUEST2.len()).unwrap();

        assert_eq!(req.span(), TEST_REQUEST);
        assert_eq!(req.request.method.as_str(), "GET");
        assert_eq!(
            req.headers_with_name("Host").next().unwrap().value.span(),
            b"developer.mozilla.org".as_slice()
        );
        assert_eq!(
            req.headers_with_name("User-Agent")
                .next()
                .unwrap()
                .value
                .span(),
            b"Mozilla/5.0 (Macintosh; Intel Mac OS X 10.9; rv:50.0) Gecko/20100101 Firefox/50.0"
                .as_slice()
        );
        assert_eq!(req.body.unwrap().span(), b"Hello World!".as_slice());
    }

    // Make sure the first response is not parsed.
    #[test]
    fn test_parse_response_from_bytes() {
        let mut response = Vec::new();
        response.extend(TEST_RESPONSE2);
        response.extend(TEST_RESPONSE);
        let response = Bytes::copy_from_slice(&response);
        let res = parse_response_from_bytes(&response, TEST_RESPONSE2.len()).unwrap();

        assert_eq!(res.span(), TEST_RESPONSE);
        assert_eq!(res.status.code.as_str(), "200");
        assert_eq!(res.status.reason.as_str(), "OK");
        assert_eq!(
            res.headers_with_name("Server").next().unwrap().value.span(),
            b"Apache/2.2.14 (Win32)".as_slice()
        );
        assert_eq!(
            res.headers_with_name("Connection")
                .next()
                .unwrap()
                .value
                .span(),
            b"Closed".as_slice()
        );
        assert_eq!(
            res.body.unwrap().span(),
            b"<html>\n<body>\n<h1>Hello, World!</h1>\n</body>\n</html>".as_slice()
        );
    }

    #[test]
    fn test_parse_request_json() {
        let req = parse_request(TEST_REQUEST_JSON).unwrap();

        let BodyContent::Json(value) = req.body.unwrap().content else {
            panic!("body is not json");
        };

        assert_eq!(value.span(), "{\"foo\": \"bar\"}");
    }

    #[test]
    fn test_parse_response_json() {
        let res = parse_response(TEST_RESPONSE_JSON).unwrap();

        let BodyContent::Json(value) = res.body.unwrap().content else {
            panic!("body is not json");
        };

        assert_eq!(value.span(), "{\"foo\": \"bar\"}");
    }
}
