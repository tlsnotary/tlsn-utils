# MUX Protocol Specification

## 1. Overview

MUX is a symmetric stream multiplexing protocol designed to run over reliable, ordered connections such as TCP. It enables multiple logical streams to share a single underlying connection with independent flow control per stream.

### 1.1 Design Goals

- Lightweight framing with minimal overhead (14-byte headers)
- Per-stream flow control with automatic window tuning
- Graceful and abrupt stream termination
- Connection-level resource limits
- Symmetric operation (no client/server distinction)
- Deterministic stream IDs derived from user-defined identifiers

### 1.2 Terminology

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT", "SHOULD", "SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this document are to be interpreted as described in [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119).

## 2. Framing

All data transmitted over a MUX connection is encapsulated in frames. Each frame consists of a fixed 14-byte header followed by an optional payload.

### 2.1 Header Format

| Field | Offset | Size | Encoding | Description |
|-------|--------|------|----------|-------------|
| Type | 0 | 1 byte | unsigned | Frame type (see Section 3). |
| Flags | 1 | 1 byte | unsigned | Bitfield of flags (see Section 4). |
| Length | 2 | 4 bytes | big-endian | Type-specific value (see Section 3). |
| Stream ID | 6 | 8 bytes | raw bytes | Stream identifier (see Section 5.1). |

### 2.2 Byte Order

Multi-byte Length fields MUST be encoded in network byte order (big-endian). Stream IDs are stored as raw bytes.

### 2.3 Header Size

The header size is fixed at 14 bytes. Implementations MUST NOT send or accept headers of any other size.

## 3. Frame Types

The Type field identifies the frame's purpose. The Length field's interpretation varies by type.

| Type | Value | Length Field | Has Payload |
|------|-------|--------------|-------------|
| Data | `0x00` | Payload length in bytes | Yes |
| Window Update | `0x01` | Window increment in bytes | No |
| Ping | `0x02` | Opaque nonce value | No |
| GoAway | `0x03` | Error code | No |

Implementations MUST reject frames with unknown Type values by sending a GoAway frame with error code `0x01` (protocol error).

### 3.1 Data Frame (Type 0x00)

Data frames carry stream payload. The Length field specifies the number of payload bytes immediately following the header.

**Constraints:**
- Length MUST NOT exceed the receiver's available stream receive window.
- Length MUST NOT exceed `1,048,576` bytes (1 MiB).

### 3.2 Window Update Frame (Type 0x01)

Window Update frames adjust the sender's view of the receiver's available receive window. The Length field contains the number of bytes to add to the stream's receive window.

**Constraints:**
- Length of `0` is valid but has no effect.
- The resulting window MUST NOT exceed `2^32 - 1` bytes.

### 3.3 Ping Frame (Type 0x02)

Ping frames measure round-trip time and serve as keep-alives. The Length field contains an opaque 32-bit nonce.

**Behavior:**
- A Ping with the SYN flag set is a request. The receiver MUST respond with a Ping carrying the ACK flag and the same nonce.
- A Ping with the ACK flag set is a response. The nonce MUST match a previously sent request.
- Ping frames MUST use the zero Stream ID.

### 3.4 GoAway Frame (Type 0x03)

GoAway frames signal connection termination. The Length field contains an error code.

| Error Code | Value | Description |
|------------|-------|-------------|
| Normal | `0x00000000` | Graceful shutdown, no error. |
| Protocol Error | `0x00000001` | Peer violated the protocol. |
| Internal Error | `0x00000002` | Internal implementation error. |

**Behavior:**
- GoAway frames MUST use the zero Stream ID.
- After sending GoAway, an implementation MUST NOT open new streams.
- After receiving GoAway, an implementation MUST NOT open new streams.
- Existing streams MAY continue until closed.

## 4. Flags

Flags modify frame behavior. Multiple flags MAY be set simultaneously by combining their values with bitwise OR.

| Flag | Value | Applicable Types | Description |
|------|-------|------------------|-------------|
| FIN | `0x01` | Data | Half-closes the stream in the sender's direction. |
| RST | `0x02` | Data | Immediately resets (terminates) the stream. |
| SYN | `0x04` | Data, Ping | On Data, opens the stream. On Ping, a ping request. |
| ACK | `0x08` | Ping | Ping response. |

### 4.1 Flag Constraints

- FIN and RST are valid on Data frames only.
- ACK is only valid on Ping frames.
- SYN is valid on Data frames, where it opens a stream, and on Ping frames,
  where it marks a request.
- SYN and FIN MAY be set together: a stream opened and half-closed by one frame.
- A sender MUST NOT set FIN and RST on the same frame, nor SYN and RST. A
  receiver that observes either combination MUST process the frame as a reset
  and ignore the other flag.
- A sender MUST NOT set a flag on a frame type it is not applicable to. A
  receiver MUST ignore such a flag and process the frame as if it were unset,
  rather than treating it as a connection error.

## 5. Stream Management

### 5.1 Stream Identifiers

Stream IDs are 8-byte values derived from user-defined identifiers using the BLAKE3 hash function:

```
StreamID = BLAKE3(user_id)[0..8]
```

The first 8 bytes of the BLAKE3 hash are used as the Stream ID. The zero Stream ID (all bytes zero) is reserved for connection-level frames (Ping, GoAway) and MUST NOT be used for data streams.

**Properties:**
- Stream IDs are deterministic: the same user ID always produces the same Stream ID.
- Either peer MAY open any stream by sending frames to that Stream ID.
- If both peers open the same stream simultaneously, the streams merge automatically.
  Each side sends its own opening frame, and receiving SYN for a Stream ID that
  already exists locally is expected, not an error.
- A Stream ID MUST NOT be opened more than once per connection, even after that
  stream has closed. Implementations are not required to enforce this.

### 5.2 Opening a Stream

A stream is opened by a Data frame carrying the SYN flag. No other frame creates
a stream.

**When sending:**
1. Compute the Stream ID from the user-defined identifier.
2. If the stream does not exist locally, create it and mark its first outbound
   frame as the opening frame.
3. Set SYN on that frame. A stream adopted from a peer that opened it first is
   already open and MUST NOT send SYN.

**When receiving a Data frame with SYN:**
1. If the Stream ID is already known, this is a simultaneous open: process the
   frame normally against the existing stream.
2. Otherwise create the stream. The body of an opening frame MUST NOT exceed the
   default credit, as it precedes any window negotiation; a larger body is a
   protocol violation and MUST be answered with GoAway (Protocol Error).
3. If the stream limit would be exceeded, send GoAway with Internal Error.

**When receiving any other frame for an unknown Stream ID:**

Ignore it. It is not a protocol violation and MUST NOT be answered with GoAway or
a stream reset. Such a frame is ordinarily a leftover from a stream the receiver
has already released while the sender was still flushing frames queued before it
learned of the close. Creating a stream for it would produce an entry no local
handle can claim, which would hold its resources until the connection ends.

### 5.3 Stream Lifecycle

```
                     +-------+
                     | Open  |
                     +-------+
                    /    |    \
              FIN← /     |     \ →FIN
                  /      |      \
           +----------+  |  +----------+
           |RecvClosed|  |  |SendClosed|
           +----------+  |  +----------+
                  \      |      /
              FIN→ \     |     / ←FIN
                    \    |    /
                     +-------+
                     |Closed |
                     +-------+
                          ↑
                      RST (any state)
```

| State | Description |
|-------|-------------|
| Open | Bidirectional data flow. |
| SendClosed | Local side sent FIN. Can still receive data. |
| RecvClosed | Remote side sent FIN. Can still send data. |
| Closed | Both directions closed. Stream resources may be released. |

### 5.4 Half-Close (FIN)

Sending FIN indicates the sender will transmit no more data on this stream. The stream transitions:
- `Open` → `SendClosed`
- `RecvClosed` → `Closed`

Receiving FIN transitions:
- `Open` → `RecvClosed`
- `SendClosed` → `Closed`

### 5.5 Reset (RST)

RST immediately terminates a stream from any state. Both sides SHOULD release stream resources upon sending or receiving RST. No further frames SHOULD be sent on a reset stream.

## 6. Flow Control

MUX implements per-stream flow control using a credit-based window mechanism.

### 6.1 Stream Receive Window

Each stream maintains an independent receive window representing the number of bytes the receiver is willing to buffer.

**Initial window size:** 262,144 bytes (256 KiB)

**Behavior:**
- Senders MUST NOT send more data than the receiver's advertised window.
- Each byte of Data payload consumes one byte of window.
- Window Update frames replenish the window.

### 6.2 Window Updates

Receivers send Window Update frames to grant additional receive capacity.

**Timing:**
- Implementations SHOULD send Window Update when approximately half the window has been consumed.
- Implementations MAY batch window updates for efficiency.

### 6.3 Connection-Level Window Limit

Implementations SHOULD enforce a connection-level limit on the total receive window across all streams.

**Recommended limit:** 1,073,741,824 bytes (1 GiB)

This prevents a peer from opening many streams to exhaust memory.

### 6.4 Automatic Window Tuning

Implementations MAY automatically increase stream receive windows based on observed throughput and round-trip time.

## 7. Session Management

### 7.1 Connection Initialization

MUX does not require an explicit handshake. The connection is considered established once the underlying transport is connected. Either peer may immediately begin opening streams.

### 7.2 Graceful Shutdown

To gracefully close a connection:

1. Send a GoAway frame with error code `0x00` (Normal).
2. Stop opening new streams.
3. Wait for existing streams to close naturally or with timeout.
4. Close the underlying connection.

### 7.3 Synchronized Close Mode

In synchronized close mode, both sides exchange GoAway frames before closing.

**Initiator behavior:**
1. Send GoAway.
2. Wait to receive GoAway from peer (with timeout).
3. Close underlying connection.

**Responder behavior:**
1. Receive GoAway.
2. Send GoAway.
3. Close underlying connection.

### 7.4 Keep-Alive

Implementations MAY send periodic Ping frames to detect connection liveness and measure RTT.

## 8. Error Handling

### 8.1 Protocol Violations

Upon detecting a protocol violation, implementations MUST:
1. Send a GoAway frame with appropriate error code.
2. Close all streams.
3. Close the underlying connection.

**Examples of protocol violations:**
- Unknown frame type
- Data exceeding receive window
- Stream limit exceeded
- Invalid flags for frame type

### 8.2 Stream Errors vs Connection Errors

- **Stream errors** (e.g., application-level errors) SHOULD be handled with RST on that stream.
- **Connection errors** (e.g., protocol violations) MUST be handled with GoAway and connection closure.

## 9. Constants Summary

| Constant | Value | Description |
|----------|-------|-------------|
| Header Size | 14 bytes | Fixed frame header size |
| Default Receive Window | 262,144 bytes (256 KiB) | Initial per-stream receive window |
| Max Frame Payload | 1,048,576 bytes (1 MiB) | Maximum Data frame payload |
| Recommended Connection Window | 1,073,741,824 bytes (1 GiB) | Maximum total receive window |

## 10. Security Considerations

- Implementations MUST validate all frame fields to prevent integer overflows.
- Implementations SHOULD enforce connection-level resource limits to prevent denial of service.
- Stream ID collisions are theoretically possible but extremely unlikely (64-bit hash space).

## 11. References

- [RFC 2119: Key words for use in RFCs](https://www.rfc-editor.org/rfc/rfc2119)
- [BLAKE3 Hash Function](https://github.com/BLAKE3-team/BLAKE3)
