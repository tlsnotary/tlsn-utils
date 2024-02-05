use std::ops::Range;

/// Returns the range within the source string corresponding to the span.
///
/// # Panics
///
/// Panics if the span is not within the source string.
pub(crate) fn get_span_range(src: &[u8], span: &[u8]) -> Range<usize> {
    let src_start = src.as_ptr() as usize;
    let src_end = src_start + src.len();
    let span_start = span.as_ptr() as usize;
    let span_end = span_start + span.len();

    assert!(
        span_start >= src_start && span_end <= src_end,
        "span is not within source string: src={src_start}..{src_end}, span={span_start}..{span_end}"
    );

    span_start - src_start..span_end - src_start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_span_range() {
        let src = b"foobar";

        assert_eq!(get_span_range(src, &src[..]), 0..src.len());
        assert_eq!(get_span_range(src, &src[0..1]), 0..1);
        assert_eq!(get_span_range(src, &src[1..2]), 1..2);
        assert_eq!(get_span_range(src, &src[3..6]), 3..6);
    }

    #[test]
    #[should_panic]
    fn test_get_span_range_outside_src_begin() {
        let src = b"foobar";

        get_span_range(&src[1..3], &src[..3]);
    }

    #[test]
    #[should_panic]
    fn test_get_span_range_outside_src_end() {
        let src = b"foobar";

        get_span_range(&src[1..3], &src[2..]);
    }
}
