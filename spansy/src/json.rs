//! JSON span parsing.
//!
//! This module provides a JSON parser that can be used to parse span
//! information for each JSON value within a source string.
//!
//! Note that the parser does *not* fully parse values, it simply computes the
//! span of the corresponding characters in the source string. Thus, this parser
//! should not be expected to perform any kind of validation of the JSON.
//!
//! # Example
//!
//! ```
//! use spansy::json;
//!
//! let src = b"{\"foo\": {\"bar\": [42, 14]}}";
//!
//! let value = json::parse(src).unwrap();
//!
//! // We can assert that the value present at the path "foo.bar.1" is the number 14.
//! assert_eq!(value.get("foo.bar.1").unwrap().view().as_str().as_ref(), "14");
//!
//! let bar = value.get("foo.bar").unwrap();
//!
//! // The span of the `bar` array is 16..24 within the source string.
//! assert_eq!(bar.view().indices(), &rangeset::set::RangeSet::from(16usize..24));
//! ```

mod span;
mod types;
mod visit;

pub use span::parse;
pub use types::{
    Array, Bool, Document, JsonKey, JsonValue, KeyValue, Null, Number, Object, String,
};
pub use visit::JsonVisit;
