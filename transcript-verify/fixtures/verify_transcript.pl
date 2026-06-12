#!/usr/bin/perl
# verify_transcript.pl — sanity-check one raw HTTP/1.1 transcript buffer.
#
# Used by capture.sh (live captures) and gen_synthetic.sh (hand-crafted pairs)
# to make sure every checked-in fixture is a well-formed, fully-framed
# HTTP/1.1 message with byte-exact framing metadata.
#
# Usage:
#   verify_transcript.pl <file> request
#   verify_transcript.pl <file> response [request-method] [expected-status]
#
# Checks (all fatal):
#   - non-empty; responses <= 200 KB
#   - head terminated by CRLFCRLF; no bare LF in head lines
#   - request:  "<METHOD> <target> HTTP/1.1" start line (METHOD = RFC 9110 token)
#   - response: starts with "HTTP/1.1 " + 3-digit status (empty reason allowed);
#               status equals <expected-status> when given
#   - response head carries no Content-Encoding (captures use identity)
#   - no duplicate Content-Length / Transfer-Encoding; not both at once
#   - framing is internally consistent:
#       * Content-Length N  -> body is exactly N bytes, message ends at EOF
#       * chunked           -> full chunk walk (hex sizes, exact CRLFs) ending
#                              precisely at EOF, optional trailers verified
#       * HEAD/1xx/204/304  -> zero body bytes regardless of CL/TE headers
#       * neither (response)-> close-delimited; reported, not rejected
#       * neither (request) -> zero body bytes required
#
# On success prints one line:  <start-line> \t <framing-summary> \t <body-bytes>
# On failure dies with a description and exits non-zero.

use strict;
use warnings;

my ( $file, $mode, $method, $expect_status ) = @ARGV;
defined $file && defined $mode
    or die "usage: verify_transcript.pl <file> <request|response> [method] [expected-status]\n";
$method = defined $method && length $method ? uc $method : 'GET';

open my $fh, '<:raw', $file or die "$file: cannot open: $!\n";
my $buf = do { local $/; <$fh> };
close $fh;
$buf = '' unless defined $buf;
my $len = length $buf;

die "$file: empty capture\n" if $len == 0;
die "$file: too large ($len bytes > 204800)\n" if $mode eq 'response' && $len > 204800;

my $he = index $buf, "\r\n\r\n";
die "$file: no CRLFCRLF head terminator\n" if $he < 0;

my $head  = substr $buf, 0, $he;
my @lines = split /\r\n/, $head, -1;
my $start = shift @lines;
die "$file: start line contains a bare LF\n" if $start =~ /\n/;

my $status = 0;
if ( $mode eq 'response' ) {
    substr( $buf, 0, 9 ) eq 'HTTP/1.1 '
        or die "$file: does not start with 'HTTP/1.1 ' (got '" . printable( substr $buf, 0, 16 ) . "')\n";
    ($status) = $start =~ m{^HTTP/1\.1 (\d{3})(?: |$)}
        or die "$file: malformed status line: '" . printable($start) . "'\n";
    die "$file: expected status $expect_status, got $status\n"
        if defined $expect_status && length $expect_status && $status != $expect_status;
}
elsif ( $mode eq 'request' ) {
    my ($m) = $start =~ m{^([!#\$%&'*+.^_`|~0-9A-Za-z-]+) \S+ HTTP/1\.1$}
        or die "$file: malformed request line: '" . printable($start) . "'\n";
    $method = $m;    # trust the request line itself
}
else {
    die "mode must be 'request' or 'response', got '$mode'\n";
}

my ( $cl, $te );
for my $l (@lines) {
    die "$file: header line contains a bare LF: '" . printable($l) . "'\n" if $l =~ /\n/;
    if ( $l =~ /^content-length:[ \t]*([0-9]+)[ \t]*$/i ) {
        die "$file: duplicate Content-Length\n" if defined $cl;
        $cl = $1;
    }
    elsif ( $l =~ /^content-length:/i ) {
        die "$file: unparseable Content-Length: '" . printable($l) . "'\n";
    }
    elsif ( $l =~ /^transfer-encoding:[ \t]*(.*?)[ \t]*$/i ) {
        die "$file: duplicate Transfer-Encoding\n" if defined $te;
        $te = lc $1;
    }
    elsif ( $mode eq 'response' && $l =~ /^content-encoding:/i ) {
        die "$file: response has Content-Encoding despite 'Accept-Encoding: identity': '"
            . printable($l) . "'\n";
    }
}

my $body_off = $he + 4;
my $blen     = $len - $body_off;
my $framing;

my $bodyless = $mode eq 'response'
    && ( $method eq 'HEAD'
    || $status == 204
    || $status == 304
    || ( $status >= 100 && $status < 200 ) );

if ($bodyless) {
    die "$file: bodyless response (method=$method status=$status) carries $blen body bytes\n"
        if $blen;
    $framing = 'none(' . ( $method eq 'HEAD' ? 'HEAD' : $status )
        . ( defined $cl ? ", CL:$cl hdr" : '' )
        . ( defined $te ? ', TE hdr'     : '' ) . ')';
}
elsif ( defined $te ) {
    die "$file: unsupported Transfer-Encoding '$te'\n" unless $te eq 'chunked';
    die "$file: both Content-Length and Transfer-Encoding present\n" if defined $cl;
    my ( $p, $decoded, $chunks, $trailers ) = ( $body_off, 0, 0, '' );
    while (1) {
        my $eol = index $buf, "\r\n", $p;
        die "$file: chunked: missing CRLF after chunk-size line at offset $p\n" if $eol < 0;
        my $line = substr $buf, $p, $eol - $p;
        my ($hex) = $line =~ /^([0-9A-Fa-f]{1,16})(?:;[^\r\n]*)?$/
            or die "$file: chunked: bad chunk-size line: '" . printable($line) . "'\n";
        my $sz = hex $hex;
        $p = $eol + 2;
        if ( $sz == 0 ) {
            if ( substr( $buf, $p, 2 ) eq "\r\n" ) {
                die "$file: chunked: trailing bytes after final CRLF\n" if $p + 2 != $len;
            }
            else {
                my $tend = index $buf, "\r\n\r\n", $p;
                die "$file: chunked: unterminated trailer section\n" if $tend < 0;
                die "$file: chunked: trailing bytes after trailers\n" if $tend + 4 != $len;
                $trailers = join '+',
                    map { ( split /:/, $_, 2 )[0] }
                    split /\r\n/, substr( $buf, $p, $tend - $p );
            }
            last;
        }
        die "$file: chunked: chunk data (size $sz at offset $p) overruns buffer\n"
            if $p + $sz + 2 > $len;
        substr( $buf, $p + $sz, 2 ) eq "\r\n"
            or die "$file: chunked: missing CRLF after chunk data\n";
        $decoded += $sz;
        $chunks++;
        $p += $sz + 2;
    }
    $framing = "chunked($chunks chunks, $decoded B decoded"
        . ( $trailers ? ", trailers:$trailers" : '' ) . ')';
}
elsif ( defined $cl ) {
    die "$file: Content-Length $cl != actual body bytes $blen\n" if $cl != $blen;
    $framing = "content-length:$cl";
}
else {
    if ( $mode eq 'request' ) {
        die "$file: request has $blen body bytes but no Content-Length/Transfer-Encoding\n"
            if $blen;
        $framing = 'none';
    }
    else {
        $framing = $blen ? "close-delimited($blen B)" : 'none(empty, close)';
    }
}

print join( "\t", $start, $framing, $blen ), "\n";

sub printable {
    my ($s) = @_;
    $s = '' unless defined $s;
    $s =~ s/([^\x20-\x7e])/sprintf('\\x%02x', ord $1)/ge;
    return $s;
}
