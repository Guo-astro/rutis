package peer

// A frame over the channel's per-message limit, carried by `chunk` frames
// and reassembled by the far end (#1363; session protocol §2.5).
//
// A `chunk` frame is not a session frame: the reader takes it before the
// frame is handled, so the frame vocabulary and every rule about call ids,
// references and unknown fields stay as they are. The sending side cuts an
// encoded frame's bytes, base64s each slice and writes the slices together;
// the receiving side joins them and hands the bytes to the session as one
// message.

import (
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"strconv"
	"unicode/utf8"

	"github.com/arcships/rutis/go/rutis/internal/values"
)

const (
	// ChunkSize is the most bytes one `chunk` frame carries before base64:
	// 8 MiB, about 10.67 MiB on the wire, inside the 16 MiB a process pipe
	// or a WebSocket takes.
	ChunkSize = 8 << 20
	// DefaultMaxTransfer is the default most a chunked transfer carries once
	// reassembled, and the default most one frame may be on a session:
	// 256 MiB. `Options.MaxTransfer` or `RUTIS_MAX_TRANSFER` raises or
	// lowers it; never unlimited.
	DefaultMaxTransfer = 256 << 20
	// The room a `chunk` frame needs beyond its base64 `data`: the tag, the
	// transfer id, the sequence number, the flag and the JSON syntax.
	frameRoom = 256

	maxTransferEnv = "RUTIS_MAX_TRANSFER"
)

// chunkFields are the fields a `chunk` frame defines: any other is a
// malformed frame, which ends the session.
var chunkFields = map[string]bool{"op": true, "transfer": true, "seq": true, "last": true, "data": true}

// maxTransferFromEnvironment is the deployment's reassembly cap, or the
// default; an invalid value refuses the session naming the variable and
// what it expected.
func maxTransferFromEnvironment() (int, error) {
	value, ok := os.LookupEnv(maxTransferEnv)
	if !ok {
		return DefaultMaxTransfer, nil
	}
	bytes, err := strconv.Atoi(value)
	if err != nil || bytes <= 0 {
		return 0, fmt.Errorf("%s %q: expected a positive number of bytes", maxTransferEnv, value)
	}
	return bytes, nil
}

// chunkSize is the largest slice whose `chunk` frame, base64 included, fits
// `limit` bytes; 0 when not even one byte does. A smaller channel limit
// shrinks the slice, so a `chunk` frame never exceeds the channel itself.
func chunkSize(limit int) int {
	size := (limit - frameRoom) / 4 * 3
	if size < 0 {
		return 0
	}
	return min(size, ChunkSize)
}

// chunkOverLimit is `over the limit of <n> bytes (<n> MiB)` for a transfer
// cap.
func chunkOverLimit(limit int) string {
	const mib = 1 << 20
	text := fmt.Sprintf("over the limit of %d bytes", limit)
	if limit%mib == 0 {
		text += fmt.Sprintf(" (%d MiB)", limit/mib)
	}
	return text
}

// chunkTransfer is the transfer the reading goroutine is rebuilding, if
// any. One at a time: the sender queues a frame's chunks together, so
// another transfer's chunk can only mean the far end broke the rule.
type chunkTransfer struct {
	id    string
	next  uint64
	bytes []byte
}

// chunkingLocked reports whether the far end declared `chunked` and the
// channel leaves room for a `chunk` frame. This side always declares it;
// p.mu is held.
func (p *Peer) chunkingLocked() bool {
	return p.supports("chunked") && chunkSize(p.maxMessage) > 0
}

// sendLimitLocked is the most this side may send in one frame: the
// session's max transfer when the far end takes chunks, otherwise the
// smaller of the channel's limit and the session's; p.mu is held.
func (p *Peer) sendLimitLocked() int {
	if p.chunkingLocked() {
		return p.maxTransfer
	}
	return min(p.maxMessage, p.maxTransfer)
}

// writeChunkedLocked sends `data` as `chunk` frames; p.writeMu is held, and
// `transfer` names the transfer. All chunks go out under that one lock, so
// no frame comes between them.
func (p *Peer) writeChunkedLocked(data []byte, transfer string) error {
	size := chunkSize(p.maxMessage)
	for at, seq := 0, uint64(0); at < len(data); seq++ {
		end := min(at+size, len(data))
		encoded, err := marshal(map[string]any{
			"op":       "chunk",
			"transfer": transfer,
			"seq":      seq,
			"last":     end == len(data),
			"data":     base64.StdEncoding.EncodeToString(data[at:end]),
		})
		if err != nil {
			return err
		}
		if err := p.channel.Send(encoded); err != nil {
			return err
		}
		at = end
	}
	return nil
}

// receiveChunk buffers one `chunk` of a frame over the channel's limit. It
// runs on the reading goroutine. A transfer is taken in order, one at a
// time; nothing reaches the session until its last chunk, and a broken
// transfer ends the session (there is no call to fail).
func (p *Peer) receiveChunk(message []byte) error {
	p.mu.Lock()
	handshake, declared, maxTransfer := p.handshake, p.supports("chunked"), p.maxTransfer
	p.mu.Unlock()
	if !handshake {
		return errors.New("request before protocol handshake")
	}
	if !declared {
		return errors.New("chunk frame from a far end that did not declare chunked")
	}
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(message, &fields); err != nil {
		return fmt.Errorf("invalid chunk frame: %w", err)
	}
	for key := range fields {
		if !chunkFields[key] {
			return fmt.Errorf("invalid chunk frame: unknown field %q", key)
		}
	}
	for key := range chunkFields {
		if _, ok := fields[key]; !ok {
			return fmt.Errorf("invalid chunk frame: missing %s", key)
		}
	}
	var transfer string
	if err := json.Unmarshal(fields["transfer"], &transfer); err != nil || transfer == "" {
		return errors.New("chunk frame without a transfer id")
	}
	var seq uint64
	if err := json.Unmarshal(fields["seq"], &seq); err != nil {
		return errors.New("invalid chunk frame: invalid seq")
	}
	var last bool
	if err := json.Unmarshal(fields["last"], &last); err != nil {
		return errors.New("invalid chunk frame: invalid last")
	}
	var data string
	if err := json.Unmarshal(fields["data"], &data); err != nil {
		return errors.New("invalid chunk frame: invalid data")
	}
	current := p.incoming
	if current != nil && current.id != transfer {
		return fmt.Errorf("chunked transfer %s: transfer %s started before it ended", current.id, transfer)
	}
	expected := uint64(0)
	if current != nil {
		expected = current.next
	}
	if seq != expected {
		return fmt.Errorf("chunked transfer %s: expected chunk %d, got %d", transfer, expected, seq)
	}
	part, err := values.ParseBytes(data)
	if err != nil {
		return fmt.Errorf("chunked transfer %s: %w", transfer, err)
	}
	if current == nil {
		current = &chunkTransfer{id: transfer}
		p.incoming = current
	}
	current.bytes = append(current.bytes, part...)
	if len(current.bytes) > maxTransfer {
		p.incoming = nil
		return fmt.Errorf("chunked transfer %s: %s", transfer, chunkOverLimit(maxTransfer))
	}
	current.next++
	if !last {
		return nil
	}
	complete := current.bytes
	p.incoming = nil
	if !utf8.Valid(complete) {
		return errors.New("received a message that is not valid UTF-8")
	}
	var completeFrame frame
	if err := json.Unmarshal(complete, &completeFrame); err != nil {
		return fmt.Errorf("received a message that is not JSON: %w", err)
	}
	// A chunk wraps one frame's bytes: it may not wrap another chunk.
	if completeFrame.Op == "chunk" {
		return errors.New("chunk frame inside a chunked transfer")
	}
	return p.receive(&completeFrame, unknownFrameFields(complete))
}
