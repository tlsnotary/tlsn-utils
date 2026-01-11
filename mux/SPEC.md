# MUX Protocol Specification

## 1. Overview

MUX is a stream multiplexing protocol designed to run over reliable, ordered connections such as TCP. It enables multiple logical streams to share a single underlying connection with independent flow control per stream.

### 1.1 Design Goals

- Lightweight framing with minimal overhead (12-byte headers)
- Per-stream flow control with automatic window tuning
- Graceful and abrupt stream termination
- Connection-level resource limits
- Optional synchronized connection shutdown

### 1.2 Relationship to Yamux

MUX is derived from the [yamux protocol](https://github.com/hashicorp/yamux/blob/master/spec.md) and maintains wire compatibility for basic operations. MUX extends yamux with:

- Connection-level receive window limits
- ACK backlog limits for stream creation
- RTT-based automatic window tuning
- Synchronized close mode

### 1.3 Terminology

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT", "SHOULD", "SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this document are to be interpreted as described in [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119).

## 2. Framing

All data transmitted over a MUX connection is encapsulated in frames. Each frame consists of a fixed 12-byte header followed by an optional payload.

### 2.1 Header Format

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|    Version    |     Type      |            Flags              |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                          Stream ID                            |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                           Length                              |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

| Field | Offset | Size | Encoding | Description |
|-------|--------|------|----------|-------------|
| Version | 0 | 1 byte | unsigned | Protocol version. MUST be `0`. |
| Type | 1 | 1 byte | unsigned | Frame type (see Section 3). |
| Flags | 2 | 2 bytes | big-endian | Bitfield of flags (see Section 4). |
| Stream ID | 4 | 4 bytes | big-endian | Stream identifier. |
| Length | 8 | 4 bytes | big-endian | Type-specific value (see Section 3). |

### 2.2 Byte Order

All multi-byte fields MUST be encoded in network byte order (big-endian).

### 2.3 Header Size

The header size is fixed at 12 bytes. Implementations MUST NOT send or accept headers of any other size.

## 3. Frame Types

The Type field identifies the frame's purpose. The Length field's interpretation varies by type.

| Type | Value | Length Field | Has Payload |
|------|-------|--------------|-------------|
| Data | `0x00` | Payload length in bytes | Yes |
| Window Update | `0x01` | Window increment in bytes | No |
| Ping | `0x02` | Opaque nonce value | No |
| GoAway | `0x03` | Error code | No |
| StreamInit | `0x04` | User ID length in bytes | Yes (user ID) |

Implementations MUST reject frames with unknown Type values by sending a GoAway frame with error code `0x01` (protocol error).

### 3.1 Data Frame (Type 0x00)

Data frames carry stream payload. The Length field specifies the number of payload bytes immediately following the header.

**Constraints:**
- Length MUST NOT exceed the receiver's available stream receive window.
- Length MUST NOT exceed `1,048,576` bytes (1 MiB).

**Example: Data frame with 1024-byte payload on stream 5**
```
Version: 0x00
Type:    0x00 (Data)
Flags:   0x0000
Stream:  0x00000005
Length:  0x00000400 (1024)
[1024 bytes of payload follow]
```

### 3.2 Window Update Frame (Type 0x01)

Window Update frames adjust the sender's view of the receiver's available receive window. The Length field contains the number of bytes to add to the stream's receive window.

**Constraints:**
- Length of `0` is valid but has no effect.
- The resulting window MUST NOT exceed `2^32 - 1` bytes.

**Example: Grant 65536 additional bytes to stream 3**
```
Version: 0x00
Type:    0x01 (Window Update)
Flags:   0x0000
Stream:  0x00000003
Length:  0x00010000 (65536)
```

### 3.3 Ping Frame (Type 0x02)

Ping frames measure round-trip time and serve as keep-alives. The Length field contains an opaque 32-bit nonce.

**Behavior:**
- A Ping with the SYN flag set is a request. The receiver MUST respond with a Ping carrying the ACK flag and the same nonce.
- A Ping with the ACK flag set is a response. The nonce MUST match a previously sent request.
- Ping frames MUST use Stream ID `0`.

**Example: Ping request**
```
Version: 0x00
Type:    0x02 (Ping)
Flags:   0x0001 (SYN)
Stream:  0x00000000
Length:  0x12345678 (nonce)
```

**Example: Ping response**
```
Version: 0x00
Type:    0x02 (Ping)
Flags:   0x0002 (ACK)
Stream:  0x00000000
Length:  0x12345678 (same nonce)
```

### 3.4 GoAway Frame (Type 0x03)

GoAway frames signal connection termination. The Length field contains an error code.

| Error Code | Value | Description |
|------------|-------|-------------|
| Normal | `0x00000000` | Graceful shutdown, no error. |
| Protocol Error | `0x00000001` | Peer violated the protocol. |
| Internal Error | `0x00000002` | Internal implementation error. |

**Behavior:**
- GoAway frames MUST use Stream ID `0`.
- After sending GoAway, an implementation MUST NOT open new streams.
- After receiving GoAway, an implementation MUST NOT open new streams.
- Existing streams MAY continue until closed.

**Example: Graceful shutdown**
```
Version: 0x00
Type:    0x03 (GoAway)
Flags:   0x0000
Stream:  0x00000000
Length:  0x00000000 (Normal)
```

### 3.5 StreamInit Frame (Type 0x04)

StreamInit frames initiate a new stream. The Length field specifies the number of bytes in the optional user-defined stream identifier that follows the header.

**User-Defined Stream Identifiers:**
- Length of `0` indicates no user ID (the stream has no user-defined identifier).
- Length MUST NOT exceed 32 bytes.
- Non-empty user IDs MUST be unique within a session. Duplicate non-empty user IDs are a protocol error.
- Multiple streams MAY have no user ID (Length 0); these do not conflict with each other.

**Constraints:**
- Only the client MAY send StreamInit frames.
- StreamInit frames from the server are a protocol error.
- The Stream ID in the header identifies the new stream being created.

**Behavior:**
- Upon receiving a valid StreamInit, the server creates the stream.
- The server SHOULD set the ACK flag on its first response frame to that stream.
- The server MUST NOT acknowledge via the StreamInit frame itself.

**Example: Open stream 1 with no user ID**
```
Version: 0x00
Type:    0x04 (StreamInit)
Flags:   0x0000
Stream:  0x00000001
Length:  0x00000000
```

**Example: Open stream 2 with 8-byte user ID "mystream"**
```
Version: 0x00
Type:    0x04 (StreamInit)
Flags:   0x0000
Stream:  0x00000002
Length:  0x00000008
[8 bytes: "mystream"]
```

## 4. Flags

Flags modify frame behavior. Multiple flags MAY be set simultaneously by combining their values with bitwise OR.

| Flag | Value | Applicable Types | Description |
|------|-------|------------------|-------------|
| SYN | `0x0001` | Ping | Ping request. Reserved on Data/WindowUpdate (MUST NOT be set). |
| ACK | `0x0002` | Data, Window Update, Ping | Acknowledges stream initiation (Data/WU) or ping response (Ping). |
| FIN | `0x0004` | Data, Window Update | Half-closes the stream in the sender's direction. |
| RST | `0x0008` | Data, Window Update | Immediately resets (terminates) the stream. |

### 4.1 Flag Combinations

- FIN and RST MUST NOT be set together.
- RST takes precedence; if RST is set, FIN MUST be ignored.
- SYN on Data or WindowUpdate frames is a protocol error.

## 5. Stream Management

### 5.1 Stream Identifiers

Stream IDs are 32-bit unsigned integers. Stream ID `0` is reserved for connection-level frames (Ping, GoAway) and MUST NOT be used for data streams.

**Allocation:**
- Only the client (connection initiator) MAY open streams.
- The server (connection acceptor) MUST NOT open streams.
- Stream IDs MUST be allocated sequentially starting from 1: 1, 2, 3, ...
- Stream IDs MUST be unique; reuse of a closed Stream ID is a protocol error.

### 5.2 Stream Lifecycle

#### 5.2.1 Opening a Stream

A stream is opened by the client sending a StreamInit frame. The server SHOULD respond with an ACK flag on its first Data or WindowUpdate frame to that stream.

**Client behavior:**
1. Select the next available Stream ID.
2. Send a StreamInit frame with the new Stream ID and optional user-defined identifier.
3. The stream enters `Open(acknowledged=false)` state.
4. Upon receiving ACK on a Data or WindowUpdate frame, transition to `Open(acknowledged=true)`.

**Server behavior:**
1. Receive StreamInit frame.
2. Validate the Stream ID (sequential, not already in use).
3. Validate user ID (if non-empty, must be unique within session).
4. If valid, create the stream.
5. Set ACK flag on the first Data or WindowUpdate frame sent to that stream.
6. If invalid, send GoAway with Protocol Error.

The server MUST NOT send StreamInit frames.

#### 5.2.2 Stream States

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
| Open | Bidirectional data flow. May be acknowledged or pending acknowledgment. |
| SendClosed | Local side sent FIN. Can still receive data. |
| RecvClosed | Remote side sent FIN. Can still send data. |
| Closed | Both directions closed. Stream resources may be released. |

#### 5.2.3 Half-Close (FIN)

Sending FIN indicates the sender will transmit no more data on this stream. The stream transitions:
- `Open` → `SendClosed`
- `RecvClosed` → `Closed`

Receiving FIN transitions:
- `Open` → `RecvClosed`
- `SendClosed` → `Closed`

#### 5.2.4 Reset (RST)

RST immediately terminates a stream from any state. Both sides SHOULD release stream resources upon sending or receiving RST. No further frames SHOULD be sent on a reset stream.

### 5.3 ACK Backlog Limit

To prevent resource exhaustion, implementations SHOULD limit the number of outbound streams awaiting acknowledgment.

**Recommended limit:** 256 streams

When the limit is reached, implementations MUST NOT open new streams until existing streams are acknowledged or closed.

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

**Example flow:**
1. Stream opens with 256 KiB window.
2. Sender transmits 128 KiB of data.
3. Receiver's window is now 128 KiB.
4. Receiver processes data, sends Window Update for 128 KiB.
5. Sender's view of window returns to 256 KiB.

### 6.3 Connection-Level Window Limit

Implementations SHOULD enforce a connection-level limit on the total receive window across all streams.

**Recommended limit:** 1,073,741,824 bytes (1 GiB)

This prevents a peer from opening many streams to exhaust memory. The limit applies to the sum of all streams' maximum receive windows.

### 6.4 Automatic Window Tuning

Implementations MAY automatically increase stream receive windows based on observed throughput and round-trip time.

**Algorithm (RECOMMENDED):**
1. Track the time when the receive window drops below half capacity.
2. If window drops to half within 2 RTTs, double the maximum window size.
3. Respect per-stream and connection-level limits.
4. Never decrease the maximum window size.

This approach is inspired by bandwidth-delay product (BDP) estimation in QUIC.

## 7. Session Management

### 7.1 Connection Initialization

MUX does not require an explicit handshake. The connection is considered established once the underlying transport is connected. The client may immediately begin opening streams.

**Role determination:**
- The side that initiated the underlying connection is the "client" and MAY open streams.
- The side that accepted the connection is the "server" and MUST NOT open streams.

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

This mode ensures both sides agree the connection is terminating, preventing data loss from in-flight frames.

### 7.4 Keep-Alive

Implementations MAY send periodic Ping frames to detect connection liveness and measure RTT. There is no mandatory keep-alive interval; implementations SHOULD make this configurable.

## 8. Error Handling

### 8.1 Protocol Violations

Upon detecting a protocol violation, implementations MUST:
1. Send a GoAway frame with appropriate error code.
2. Close all streams.
3. Close the underlying connection.

**Examples of protocol violations:**
- Unknown protocol version
- Unknown frame type
- Invalid Stream ID (non-sequential, reused)
- Data exceeding receive window
- Duplicate non-empty user-defined stream identifier
- SYN flag on Data or WindowUpdate frame
- StreamInit frame from server

### 8.2 Stream Errors vs Connection Errors

- **Stream errors** (e.g., application-level errors) SHOULD be handled with RST on that stream.
- **Connection errors** (e.g., protocol violations) MUST be handled with GoAway and connection closure.

## 9. Constants Summary

| Constant | Value | Description |
|----------|-------|-------------|
| Header Size | 12 bytes | Fixed frame header size |
| Protocol Version | 0 | Current protocol version |
| Default Receive Window | 262,144 bytes (256 KiB) | Initial per-stream receive window |
| Max Frame Payload | 1,048,576 bytes (1 MiB) | Maximum Data frame payload |
| Max User ID Length | 32 bytes | Maximum user-defined stream identifier |
| Recommended ACK Backlog | 256 | Maximum unacknowledged outbound streams |
| Recommended Connection Window | 1,073,741,824 bytes (1 GiB) | Maximum total receive window |

## 10. Security Considerations

- Implementations MUST validate all frame fields to prevent integer overflows.
- Implementations SHOULD enforce connection-level resource limits to prevent denial of service.
- Stream ID exhaustion (reaching 2^31 streams) SHOULD trigger graceful connection shutdown.

## 11. References

- [Yamux Specification](https://github.com/hashicorp/yamux/blob/master/spec.md)
- [RFC 2119: Key words for use in RFCs](https://www.rfc-editor.org/rfc/rfc2119)
- [QUIC: A UDP-Based Multiplexed and Secure Transport (RFC 9000)](https://www.rfc-editor.org/rfc/rfc9000)
