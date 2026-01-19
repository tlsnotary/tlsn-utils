use std::borrow::Cow;

use bytes::Bytes;
use rangeset::{
    iter::{IntoRangeIterator, RangeIterator},
    ops::Set,
    set::{RangeIter, RangeSet},
};

use crate::{Span, Store, View, json::Document};

/// An HTTP header name.
#[derive(Debug, Clone)]
pub struct HeaderName<S: Store = Bytes> {
    pub(crate) view: View<S, str>,
}

impl<S: Store> HeaderName<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S, str> {
        &self.view
    }

    /// Returns the indices within the store.
    pub fn indices(&self) -> &RangeSet<usize> {
        self.view.indices()
    }

    /// Returns the header name as a string.
    pub fn as_str(&self) -> Cow<'_, str> {
        self.view.as_str()
    }
}

impl<S: Store> PartialEq for HeaderName<S> {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl<S: Store> Eq for HeaderName<S> {}

/// An HTTP header value.
#[derive(Debug, Clone)]
pub struct HeaderValue<S: Store = Bytes> {
    pub(crate) view: View<S>,
}

impl<S: Store> HeaderValue<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S> {
        &self.view
    }

    /// Returns the indices within the store.
    pub fn indices(&self) -> &RangeSet<usize> {
        self.view.indices()
    }

    /// Returns the header value as bytes.
    pub fn as_bytes(&self) -> Cow<'_, [u8]> {
        self.view.as_bytes()
    }
}

impl<S: Store> PartialEq for HeaderValue<S> {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl<S: Store> Eq for HeaderValue<S> {}

/// An HTTP header, including optional whitespace and the trailing CRLF.
#[derive(Debug, Clone)]
pub struct Header<S: Store = Bytes> {
    pub(crate) view: View<S>,
    /// The header name.
    pub name: HeaderName<S>,
    /// The header value.
    pub value: HeaderValue<S>,
}

impl<S: Store> Header<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S> {
        &self.view
    }

    /// Returns the indices within the store.
    pub fn indices(&self) -> &RangeSet<usize> {
        self.view.indices()
    }

    /// Returns the header data as bytes.
    pub fn data(&self) -> Cow<'_, [u8]> {
        self.view.as_bytes()
    }

    /// Returns a view of the header excluding the value.
    ///
    /// The view will include any optional whitespace and the CRLF.
    pub fn without_value(&self) -> View<S> {
        let indices = self.view.indices().difference(self.value.view.indices());
        self.view.subview(indices.into_set())
    }
}

impl<S: Store> PartialEq for Header<S> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.value == other.value
    }
}

impl<S: Store> Eq for Header<S> {}

/// An HTTP request method.
#[derive(Debug, Clone)]
pub struct Method<S: Store = Bytes> {
    pub(crate) view: View<S, str>,
}

impl<S: Store> Method<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S, str> {
        &self.view
    }

    /// Returns the indices within the store.
    pub fn indices(&self) -> &RangeSet<usize> {
        self.view.indices()
    }

    /// Returns the method as a string.
    pub fn as_str(&self) -> Cow<'_, str> {
        self.view.as_str()
    }
}

impl<S: Store> PartialEq for Method<S> {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl<S: Store> Eq for Method<S> {}

/// An HTTP request target.
#[derive(Debug, Clone)]
pub struct Target<S: Store = Bytes> {
    pub(crate) view: View<S, str>,
}

impl<S: Store> Target<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S, str> {
        &self.view
    }

    /// Returns the indices within the store.
    pub fn indices(&self) -> &RangeSet<usize> {
        self.view.indices()
    }

    /// Returns the target as a string.
    pub fn as_str(&self) -> Cow<'_, str> {
        self.view.as_str()
    }
}

impl<S: Store> PartialEq for Target<S> {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl<S: Store> Eq for Target<S> {}

/// An HTTP request line, including the trailing CRLF.
#[derive(Debug, Clone)]
pub struct RequestLine<S: Store = Bytes> {
    pub(crate) view: View<S, str>,
    /// The request method.
    pub method: Method<S>,
    /// The request target.
    pub target: Target<S>,
}

impl<S: Store> RequestLine<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S, str> {
        &self.view
    }

    /// Returns the indices within the store.
    pub fn indices(&self) -> &RangeSet<usize> {
        self.view.indices()
    }

    /// Returns the request line as a string.
    pub fn as_str(&self) -> Cow<'_, str> {
        self.view.as_str()
    }

    /// Returns the indices of the request line excluding the request target.
    pub fn without_target(&self) -> RangeSet<usize> {
        self.view
            .indices()
            .difference(self.target.indices())
            .into_set()
    }
}

impl<S: Store> PartialEq for RequestLine<S> {
    fn eq(&self, other: &Self) -> bool {
        self.method == other.method && self.target == other.target
    }
}

impl<S: Store> Eq for RequestLine<S> {}

/// An HTTP request with span tracking.
///
/// # Example
///
/// ```
/// use spansy::http::parse_request;
///
/// let req = parse_request(b"GET /path HTTP/1.1\r\nHost: example.com\r\n\r\n").unwrap();
///
/// assert_eq!(req.request.method, "GET");
/// assert_eq!(req.request.target, "/path");
///
/// let host = req.headers_with_name("Host").next().unwrap();
/// assert_eq!(&*host.value.as_bytes(), b"example.com");
/// ```
#[derive(Debug, Clone)]
pub struct Request<S: Store = Bytes> {
    pub(crate) view: View<S>,
    /// The request line.
    pub request: RequestLine<S>,
    /// Request headers.
    pub headers: Vec<Header<S>>,
    /// Request body.
    pub body: Option<Body<S>>,
}

impl<S: Store> Request<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S> {
        &self.view
    }

    /// Returns the indices within the store.
    pub fn indices(&self) -> &RangeSet<usize> {
        self.view.indices()
    }

    /// Returns the request data as bytes.
    pub fn data(&self) -> Cow<'_, [u8]> {
        self.view.as_bytes()
    }

    /// Sets the body of the request.
    pub fn set_body(&mut self, body: Option<Body<S>>) {
        self.body = body;
    }

    /// Returns an iterator of request headers with the given name
    /// (case-insensitive).
    ///
    /// This method returns an iterator because it is valid for HTTP records to
    /// contain duplicate header names.
    pub fn headers_with_name<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Header<S>> {
        self.headers
            .iter()
            .filter(move |h| h.name.as_str().eq_ignore_ascii_case(name))
    }

    /// Returns the indices of the request excluding the target, headers and
    /// body.
    pub fn without_data(&self) -> RangeSet<usize> {
        let mut indices: RangeSet<usize> = self
            .view
            .indices()
            .difference(self.request.target.indices())
            .into_set();

        for header in &self.headers {
            indices = indices.difference(header.indices()).into_set();
        }
        if let Some(body) = &self.body {
            indices = indices.difference(body.indices()).into_set();
        }
        indices
    }
}

impl<S: Store> PartialEq for Request<S> {
    fn eq(&self, other: &Self) -> bool {
        self.request == other.request && self.headers == other.headers && self.body == other.body
    }
}

impl<S: Store> Eq for Request<S> {}

/// An HTTP response code.
#[derive(Debug, Clone)]
pub struct Code<S: Store = Bytes> {
    pub(crate) view: View<S, str>,
}

impl<S: Store> Code<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S, str> {
        &self.view
    }

    /// Returns the indices within the store.
    pub fn indices(&self) -> &RangeSet<usize> {
        self.view.indices()
    }

    /// Returns the response code as a string.
    pub fn as_str(&self) -> Cow<'_, str> {
        self.view.as_str()
    }
}

impl<S: Store> PartialEq for Code<S> {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl<S: Store> Eq for Code<S> {}

/// An HTTP response reason phrase.
#[derive(Debug, Clone)]
pub struct Reason<S: Store = Bytes> {
    pub(crate) view: View<S, str>,
}

impl<S: Store> Reason<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S, str> {
        &self.view
    }

    /// Returns the indices within the store.
    pub fn indices(&self) -> &RangeSet<usize> {
        self.view.indices()
    }

    /// Returns the response reason phrase as a string.
    pub fn as_str(&self) -> Cow<'_, str> {
        self.view.as_str()
    }
}

impl<S: Store> PartialEq for Reason<S> {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl<S: Store> Eq for Reason<S> {}

/// An HTTP response status.
#[derive(Debug, Clone)]
pub struct Status<S: Store = Bytes> {
    pub(crate) view: View<S, str>,
    /// The response code.
    pub code: Code<S>,
    /// The reason phrase.
    pub reason: Reason<S>,
}

impl<S: Store> Status<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S, str> {
        &self.view
    }

    /// Returns the indices within the store.
    pub fn indices(&self) -> &RangeSet<usize> {
        self.view.indices()
    }

    /// Returns the status as a string.
    pub fn as_str(&self) -> Cow<'_, str> {
        self.view.as_str()
    }
}

impl<S: Store> PartialEq for Status<S> {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code && self.reason == other.reason
    }
}

impl<S: Store> Eq for Status<S> {}

/// An HTTP response with span tracking.
///
/// # Example
///
/// ```
/// use spansy::http::parse_response;
///
/// let res = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello").unwrap();
///
/// assert_eq!(res.status.code, "200");
/// assert_eq!(&*res.body.unwrap().as_bytes(), b"hello");
/// ```
#[derive(Debug, Clone)]
pub struct Response<S: Store = Bytes> {
    pub(crate) view: View<S>,
    /// The response status.
    pub status: Status<S>,
    /// Response headers.
    pub headers: Vec<Header<S>>,
    /// Response body.
    pub body: Option<Body<S>>,
}

impl<S: Store> Response<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S> {
        &self.view
    }

    /// Returns the indices within the store.
    pub fn indices(&self) -> &RangeSet<usize> {
        self.view.indices()
    }

    /// Returns the response data as bytes.
    pub fn data(&self) -> Cow<'_, [u8]> {
        self.view.as_bytes()
    }

    /// Sets the body of the response.
    pub fn set_body(&mut self, body: Option<Body<S>>) {
        self.body = body;
    }

    /// Returns an iterator of response headers with the given name
    /// (case-insensitive).
    ///
    /// This method returns an iterator because it is valid for HTTP records to
    /// contain duplicate header names.
    pub fn headers_with_name<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Header<S>> {
        self.headers
            .iter()
            .filter(move |h| h.name.as_str().eq_ignore_ascii_case(name))
    }

    /// Returns the indices of the response excluding the headers and body.
    pub fn without_data(&self) -> RangeSet<usize> {
        let mut indices = self.view.indices().clone();
        for header in &self.headers {
            indices = indices.difference(header.indices()).into_set();
        }
        if let Some(body) = &self.body {
            indices = indices.difference(body.indices()).into_set();
        }
        indices
    }
}

impl<S: Store> PartialEq for Response<S> {
    fn eq(&self, other: &Self) -> bool {
        self.status == other.status && self.headers == other.headers && self.body == other.body
    }
}

impl<S: Store> Eq for Response<S> {}

/// A chunk in a chunked transfer-encoded body.
#[derive(Debug, Clone)]
pub struct Chunk<S: Store = Bytes> {
    pub(crate) view: View<S>,
}

impl<S: Store> Chunk<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S> {
        &self.view
    }

    /// Returns the indices within the store.
    pub fn indices(&self) -> &RangeSet<usize> {
        self.view.indices()
    }

    /// Returns the chunk data as bytes.
    pub fn data(&self) -> Cow<'_, [u8]> {
        self.view.as_bytes()
    }
}

impl<S: Store> PartialEq for Chunk<S> {
    fn eq(&self, other: &Self) -> bool {
        self.data() == other.data()
    }
}

impl<S: Store> Eq for Chunk<S> {}

/// An HTTP request or response payload body.
#[derive(Debug, Clone)]
pub struct Body<S: Store = Bytes> {
    pub(crate) view: View<S>,
    /// The body content.
    pub content: BodyContent<S>,
    /// Individual chunks if the body was chunked transfer-encoded.
    pub chunks: Option<Vec<Chunk<S>>>,
    /// Trailing headers sent after a chunked body.
    pub trailers: Vec<Header<S>>,
}

impl<S: Store> Body<S> {
    /// Returns the underlying view.
    pub fn view(&self) -> &View<S> {
        &self.view
    }

    /// Returns the indices within the store.
    pub fn indices(&self) -> &RangeSet<usize> {
        self.view.indices()
    }

    /// Returns the body data as bytes.
    pub fn data(&self) -> Cow<'_, [u8]> {
        self.view.as_bytes()
    }

    /// Returns the body as bytes (alias for data).
    pub fn as_bytes(&self) -> Cow<'_, [u8]> {
        self.data()
    }

    /// Returns the assembled body content.
    ///
    /// For chunked bodies, this returns only the chunk data (no metadata).
    /// For non-chunked bodies, this is equivalent to `data()`.
    pub fn content_data(&self) -> Cow<'_, [u8]> {
        match &self.chunks {
            Some(chunks) => {
                // Concatenate chunk data into owned Vec
                let mut result = Vec::new();
                for chunk in chunks {
                    result.extend_from_slice(&chunk.data());
                }
                Cow::Owned(result)
            }
            None => self.data(),
        }
    }
}

impl<S: Store> PartialEq for Body<S> {
    fn eq(&self, other: &Self) -> bool {
        self.data() == other.data()
    }
}

impl<S: Store> Eq for Body<S> {}

/// An HTTP request or response payload body content.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum BodyContent<S: Store = Bytes> {
    /// Body with an `application/json` content type.
    Json(Document<S>),
    /// Body with an unknown content type.
    Unknown,
}

impl<S: Store> PartialEq for BodyContent<S> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (BodyContent::Json(a), BodyContent::Json(b)) => a == b,
            (BodyContent::Unknown, BodyContent::Unknown) => true,
            _ => false,
        }
    }
}

impl<S: Store> Eq for BodyContent<S> {}

// IntoRangeIterator and Span implementations for HTTP types

macro_rules! impl_span_str {
    ($ty:ident) => {
        impl<S: Store> IntoRangeIterator<usize> for $ty<S> {
            type IntoIter = <RangeSet<usize> as IntoRangeIterator<usize>>::IntoIter;

            fn into_range_iter(self) -> Self::IntoIter {
                self.view.indices().clone().into_range_iter()
            }
        }

        impl<'a, S: Store> IntoRangeIterator<usize> for &'a $ty<S> {
            type IntoIter = RangeIter<'a, usize>;

            fn into_range_iter(self) -> Self::IntoIter {
                self.view.indices().into_range_iter()
            }
        }

        impl<S: Store> Span<str> for $ty<S> {
            fn data(&self) -> Cow<'_, str> {
                self.view.as_str()
            }

            fn len(&self) -> usize {
                self.view.len()
            }

            fn offset(&self) -> usize {
                self.view.offset()
            }

            fn is_empty(&self) -> bool {
                self.view.indices().is_empty()
            }

            fn is_contiguous(&self) -> bool {
                self.view.indices().len_ranges() <= 1
            }
        }

        impl<S: Store> PartialEq<str> for $ty<S> {
            fn eq(&self, other: &str) -> bool {
                self.view.as_str().as_ref() == other
            }
        }

        impl<S: Store> PartialEq<&str> for $ty<S> {
            fn eq(&self, other: &&str) -> bool {
                self.view.as_str().as_ref() == *other
            }
        }

        impl<S: Store> PartialEq<[u8]> for $ty<S> {
            fn eq(&self, other: &[u8]) -> bool {
                self.view.as_str().as_bytes() == other
            }
        }

        impl<S: Store> PartialEq<&[u8]> for $ty<S> {
            fn eq(&self, other: &&[u8]) -> bool {
                self.view.as_str().as_bytes() == *other
            }
        }
    };
}

macro_rules! impl_span_bytes {
    ($ty:ident) => {
        impl<S: Store> IntoRangeIterator<usize> for $ty<S> {
            type IntoIter = <RangeSet<usize> as IntoRangeIterator<usize>>::IntoIter;

            fn into_range_iter(self) -> Self::IntoIter {
                self.view.indices().clone().into_range_iter()
            }
        }

        impl<'a, S: Store> IntoRangeIterator<usize> for &'a $ty<S> {
            type IntoIter = RangeIter<'a, usize>;

            fn into_range_iter(self) -> Self::IntoIter {
                self.view.indices().into_range_iter()
            }
        }

        impl<S: Store> Span<[u8]> for $ty<S> {
            fn data(&self) -> Cow<'_, [u8]> {
                self.view.as_bytes()
            }

            fn len(&self) -> usize {
                self.view.len()
            }

            fn offset(&self) -> usize {
                self.view.offset()
            }

            fn is_empty(&self) -> bool {
                self.view.indices().is_empty()
            }

            fn is_contiguous(&self) -> bool {
                self.view.indices().len_ranges() <= 1
            }
        }

        impl<S: Store> PartialEq<[u8]> for $ty<S> {
            fn eq(&self, other: &[u8]) -> bool {
                self.view.as_bytes().as_ref() == other
            }
        }

        impl<S: Store> PartialEq<&[u8]> for $ty<S> {
            fn eq(&self, other: &&[u8]) -> bool {
                self.view.as_bytes().as_ref() == *other
            }
        }

        impl<S: Store, const N: usize> PartialEq<&[u8; N]> for $ty<S> {
            fn eq(&self, other: &&[u8; N]) -> bool {
                self.view.as_bytes().as_ref() == *other
            }
        }
    };
}

// String-based types
impl_span_str!(HeaderName);
impl_span_str!(Method);
impl_span_str!(Target);
impl_span_str!(RequestLine);
impl_span_str!(Code);
impl_span_str!(Reason);
impl_span_str!(Status);

// Byte-based types
impl_span_bytes!(HeaderValue);
impl_span_bytes!(Header);
impl_span_bytes!(Request);
impl_span_bytes!(Response);
impl_span_bytes!(Chunk);
impl_span_bytes!(Body);

// AsRef<View<S>> implementations

impl<S: Store> AsRef<View<S, str>> for HeaderName<S> {
    fn as_ref(&self) -> &View<S, str> {
        &self.view
    }
}

impl<S: Store> AsRef<View<S>> for HeaderValue<S> {
    fn as_ref(&self) -> &View<S> {
        &self.view
    }
}

impl<S: Store> AsRef<View<S>> for Header<S> {
    fn as_ref(&self) -> &View<S> {
        &self.view
    }
}

impl<S: Store> AsRef<View<S, str>> for Method<S> {
    fn as_ref(&self) -> &View<S, str> {
        &self.view
    }
}

impl<S: Store> AsRef<View<S, str>> for Target<S> {
    fn as_ref(&self) -> &View<S, str> {
        &self.view
    }
}

impl<S: Store> AsRef<View<S, str>> for RequestLine<S> {
    fn as_ref(&self) -> &View<S, str> {
        &self.view
    }
}

impl<S: Store> AsRef<View<S>> for Request<S> {
    fn as_ref(&self) -> &View<S> {
        &self.view
    }
}

impl<S: Store> AsRef<View<S, str>> for Code<S> {
    fn as_ref(&self) -> &View<S, str> {
        &self.view
    }
}

impl<S: Store> AsRef<View<S, str>> for Reason<S> {
    fn as_ref(&self) -> &View<S, str> {
        &self.view
    }
}

impl<S: Store> AsRef<View<S, str>> for Status<S> {
    fn as_ref(&self) -> &View<S, str> {
        &self.view
    }
}

impl<S: Store> AsRef<View<S>> for Response<S> {
    fn as_ref(&self) -> &View<S> {
        &self.view
    }
}

impl<S: Store> AsRef<View<S>> for Chunk<S> {
    fn as_ref(&self) -> &View<S> {
        &self.view
    }
}

impl<S: Store> AsRef<View<S>> for Body<S> {
    fn as_ref(&self) -> &View<S> {
        &self.view
    }
}
