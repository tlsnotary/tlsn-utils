//! HTTP span parsing.

mod span;
mod types;

use bytes::Bytes;

pub use span::{parse_request, parse_response};
pub use types::{
    Body, Code, Header, HeaderName, HeaderValue, Method, Reason, Request, RequestLine, Response,
    Status, Target,
};

use crate::ParseError;

use self::span::{parse_request_from_bytes, parse_response_from_bytes};
/// An iterator yielding parsed HTTP requests.
#[derive(Debug)]
pub struct Requests {
    src: Bytes,
    /// The current position in the source string.
    pos: usize,
}

impl Requests {
    /// Returns a new `Requests` iterator.
    pub fn new(src: Bytes) -> Self {
        Self { src, pos: 0 }
    }

    /// Returns a new `Requests` iterator.
    pub fn new_from_slice(src: &[u8]) -> Self {
        Self {
            src: Bytes::copy_from_slice(src),
            pos: 0,
        }
    }
}

impl Iterator for Requests {
    type Item = Result<Request, ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.src.len() {
            None
        } else {
            Some(parse_request_from_bytes(&self.src, self.pos).map(|req| {
                self.pos += req.span.len();
                req
            }))
        }
    }
}

/// An iterator yielding parsed HTTP responses.
#[derive(Debug)]
pub struct Responses {
    src: Bytes,
    /// The current position in the source string.
    pos: usize,
}

impl Responses {
    /// Returns a new `Responses` iterator.
    pub fn new(src: Bytes) -> Self {
        Self { src, pos: 0 }
    }

    /// Returns a new `Responses` iterator.
    pub fn new_from_slice(src: &[u8]) -> Self {
        Self {
            src: Bytes::copy_from_slice(src),
            pos: 0,
        }
    }
}

impl Iterator for Responses {
    type Item = Result<Response, ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.src.len() {
            None
        } else {
            Some(parse_response_from_bytes(&self.src, self.pos).map(|resp| {
                self.pos += resp.span.len();
                resp
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Spanned;

    use super::*;

    const MULTIPLE_REQUESTS: &[u8] = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n\
        POST /hello HTTP/1.1\r\nHost: localhost\r\nContent-Length: 14\r\n\r\n\
        Hello, world!\n";

    const MULTIPLE_RESPONSES: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n\
        HTTP/1.1 200 OK\r\nContent-Length: 14\r\n\r\nHello, world!\n\
        HTTP/1.1 204 OK\r\nContent-Length: 0\r\n\r\n";

    #[test]
    fn test_parse_requests() {
        let reqs = Requests::new_from_slice(MULTIPLE_REQUESTS)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(reqs.len(), 2);

        assert_eq!(reqs[0].request.method.as_str(), "GET");
        assert!(reqs[0].body.is_none());
        assert_eq!(
            reqs[0]
                .headers_with_name("host")
                .next()
                .unwrap()
                .value
                .as_bytes(),
            b"localhost"
        );

        assert_eq!(reqs[1].request.method.as_str(), "POST");
        assert_eq!(
            reqs[1]
                .headers_with_name("host")
                .next()
                .unwrap()
                .value
                .as_bytes(),
            b"localhost"
        );
        assert_eq!(
            reqs[1]
                .headers_with_name("content-length")
                .next()
                .unwrap()
                .value
                .as_bytes(),
            b"14"
        );
        assert_eq!(
            reqs[1].body.as_ref().unwrap().span(),
            b"Hello, world!\n".as_slice()
        );
    }

    #[test]
    fn test_parse_responses() {
        let resps = Responses::new_from_slice(MULTIPLE_RESPONSES)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(resps.len(), 3);

        assert_eq!(resps[0].status.code.as_str(), "200");
        assert_eq!(
            resps[0]
                .headers_with_name("content-length")
                .next()
                .unwrap()
                .value
                .as_bytes(),
            b"0"
        );
        assert!(resps[0].body.is_none());

        assert_eq!(resps[1].status.code.as_str(), "200");
        assert_eq!(
            resps[1]
                .headers_with_name("content-length")
                .next()
                .unwrap()
                .value
                .as_bytes(),
            b"14"
        );
        assert_eq!(
            resps[1].body.as_ref().unwrap().span(),
            b"Hello, world!\n".as_slice()
        );

        assert_eq!(resps[2].status.code.as_str(), "204");
        assert_eq!(
            resps[2]
                .headers_with_name("content-length")
                .next()
                .unwrap()
                .value
                .as_bytes(),
            b"0"
        );
        assert!(resps[2].body.is_none());
    }

    #[test]
    fn test_parse_request_duplicate_headers() {
        let req_bytes = b"GET / HTTP/1.1\r\nHost: localhost\r\nAccept: application/json\r\n\
        Accept: application/xml\r\n\r\n";
        let reqs = Requests::new_from_slice(req_bytes)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(reqs.len(), 1);
        let req = reqs.first().unwrap();

        let headers: Vec<_> = req.headers_with_name("host").collect();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers.first().unwrap().value.as_bytes(), b"localhost");

        let headers: Vec<_> = req.headers_with_name("accept").collect();
        assert_eq!(headers.len(), 2);
        assert_eq!(
            headers
                .iter()
                .map(|h| h.value.as_bytes())
                .collect::<Vec<_>>(),
            vec!["application/json".as_bytes(), "application/xml".as_bytes()],
        );
    }

    #[test]
    fn test_parse_response_duplicate_headers() {
        let resp_bytes = b"HTTP/1.1 200 OK\r\nSet-Cookie: lang=en; Path=/\r\n\
        Set-Cookie: fang=fen; Path=/\r\nContent-Length: 14\r\n\r\n{\"foo\": \"bar\"}";
        let resps = Responses::new_from_slice(resp_bytes)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(resps.len(), 1);
        let resp = resps.first().unwrap();

        let headers: Vec<_> = resp.headers_with_name("set-cookie").collect();
        assert_eq!(headers.len(), 2);
        assert_eq!(
            headers
                .iter()
                .map(|h| h.value.as_bytes())
                .collect::<Vec<_>>(),
            vec!["lang=en; Path=/".as_bytes(), "fang=fen; Path=/".as_bytes()],
        );

        let headers: Vec<_> = resp.headers_with_name("content-length").collect();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers.first().unwrap().value.as_bytes(), b"14");
    }
}
