//! HTTP request and response parsing with span tracking.
//!
//! # Example
//!
//! ```
//! use spansy::http::{parse_request, parse_response, BodyContent};
//!
//! // Parse a request
//! let req = parse_request(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n").unwrap();
//! assert_eq!(req.request.method, "GET");
//!
//! // Parse a response with JSON body
//! let res = parse_response(
//!     b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 12\r\n\r\n{\"ok\": true}"
//! ).unwrap();
//!
//! let BodyContent::Json(json) = res.body.unwrap().content else { panic!() };
//! assert_eq!(json.get("ok").unwrap(), "true");
//! ```

mod span;
mod types;

use bytes::Bytes;

pub use span::{parse_request, parse_response};
pub use types::{
    Body, BodyContent, Chunk, Code, Header, HeaderName, HeaderValue, Method, Reason, Request,
    RequestLine, Response, Status, Target,
};

use crate::{ParseError, Store, View};

/// An iterator yielding parsed HTTP requests.
#[derive(Debug)]
pub struct Requests<S: Store = Bytes> {
    view: View<S>,
    /// The current position in the source string.
    pos: usize,
}

impl<S: Store> Requests<S> {
    /// Returns a new `Requests` iterator from a View.
    pub fn new(view: View<S>) -> Self {
        Self { view, pos: 0 }
    }
}

impl Requests<Bytes> {
    /// Returns a new `Requests` iterator from a byte slice.
    pub fn new_from_slice(src: &[u8]) -> Self {
        Self {
            view: View::new(Bytes::copy_from_slice(src)),
            pos: 0,
        }
    }
}

impl<S: Store> Iterator for Requests<S> {
    type Item = Result<Request<S>, ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        let len = self.view.len();
        if self.pos >= len {
            None
        } else {
            let subview = self.view.select(self.pos..len)?;
            Some(parse_request(subview).inspect(|req| {
                self.pos += req.view.len();
            }))
        }
    }
}

/// An iterator yielding parsed HTTP responses.
#[derive(Debug)]
pub struct Responses<S: Store = Bytes> {
    view: View<S>,
    /// The current position in the source string.
    pos: usize,
}

impl<S: Store> Responses<S> {
    /// Returns a new `Responses` iterator from a View.
    pub fn new(view: View<S>) -> Self {
        Self { view, pos: 0 }
    }
}

impl Responses<Bytes> {
    /// Returns a new `Responses` iterator from a byte slice.
    pub fn new_from_slice(src: &[u8]) -> Self {
        Self {
            view: View::new(Bytes::copy_from_slice(src)),
            pos: 0,
        }
    }
}

impl<S: Store> Iterator for Responses<S> {
    type Item = Result<Response<S>, ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        let len = self.view.len();
        if self.pos >= len {
            None
        } else {
            let subview = self.view.select(self.pos..len)?;
            Some(parse_response(subview).inspect(|resp| {
                self.pos += resp.view.len();
            }))
        }
    }
}

#[cfg(test)]
mod tests {
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

        assert_eq!(reqs[0].request.method, "GET");
        assert!(reqs[0].body.is_none());
        assert_eq!(
            reqs[0]
                .headers_with_name("host")
                .next()
                .unwrap()
                .value
                .as_bytes()
                .as_ref(),
            b"localhost"
        );

        assert_eq!(reqs[1].request.method, "POST");
        assert_eq!(
            reqs[1]
                .headers_with_name("host")
                .next()
                .unwrap()
                .value
                .as_bytes()
                .as_ref(),
            b"localhost"
        );
        assert_eq!(
            reqs[1]
                .headers_with_name("content-length")
                .next()
                .unwrap()
                .value
                .as_bytes()
                .as_ref(),
            b"14"
        );
        assert_eq!(
            reqs[1].body.as_ref().unwrap(),
            b"Hello, world!\n".as_slice()
        );
    }

    #[test]
    fn test_parse_responses() {
        let resps = Responses::new_from_slice(MULTIPLE_RESPONSES)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(resps.len(), 3);

        assert_eq!(resps[0].status.code, "200");
        assert_eq!(
            resps[0]
                .headers_with_name("content-length")
                .next()
                .unwrap()
                .value
                .as_bytes()
                .as_ref(),
            b"0"
        );
        assert!(resps[0].body.is_none());

        assert_eq!(resps[1].status.code, "200");
        assert_eq!(
            resps[1]
                .headers_with_name("content-length")
                .next()
                .unwrap()
                .value
                .as_bytes()
                .as_ref(),
            b"14"
        );
        assert_eq!(
            resps[1].body.as_ref().unwrap(),
            b"Hello, world!\n".as_slice()
        );

        assert_eq!(resps[2].status.code, "204");
        assert_eq!(
            resps[2]
                .headers_with_name("content-length")
                .next()
                .unwrap()
                .value
                .as_bytes()
                .as_ref(),
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
        assert_eq!(headers.first().unwrap().value, b"localhost");

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
        assert_eq!(headers.first().unwrap().value, b"14");
    }

    #[test]
    fn test_requests_non_contiguous() {
        use rangeset::set::RangeSet;

        // Two requests split across non-contiguous ranges
        // "GET / HTTP/1.1\r\nHost: a\r\n\r\n" + filler + "GET /x HTTP/1.1\r\nHost:
        // b\r\n\r\n"
        let req1 = b"GET / HTTP/1.1\r\nHost: a\r\n\r\n";
        let req2 = b"GET /x HTTP/1.1\r\nHost: b\r\n\r\n";
        let filler = b"----FILLER----";

        let mut data = Vec::new();
        data.extend_from_slice(req1);
        data.extend_from_slice(filler);
        data.extend_from_slice(req2);

        let view = View::new(data.as_slice());
        let indices = RangeSet::from([0..req1.len(), req1.len() + filler.len()..data.len()]);
        let non_contig = view.subview(indices);

        // Verify view is non-contiguous
        assert_eq!(non_contig.indices().len_ranges(), 2);

        let reqs: Vec<_> = Requests::new(non_contig)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].request.target, "/");
        assert_eq!(reqs[1].request.target, "/x");
    }
}
