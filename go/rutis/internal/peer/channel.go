package peer

import (
	"bufio"
	"bytes"
	"errors"
	"fmt"
	"io"
	"net"
	"sync"
	"time"
	"unicode/utf8"
)

// writeTimeout bounds a write: a far end that stops reading ends the
// session instead of blocking the writer forever.
const writeTimeout = 60 * time.Second

// MaxMessage is the largest message a channel carries, as on the WebSocket
// channel.
const MaxMessage = 16 * 1024 * 1024

// Channel carries ordered, reliable messages with their boundaries kept.
// Recv returns io.EOF once the far end finished.
type Channel interface {
	Send(message []byte) error
	Recv() ([]byte, error)
	Close(reason string)
}

// Lines is a stream channel, one message per line.
type Lines struct {
	conn   net.Conn
	reader *bufio.Reader
	mu     sync.Mutex
	once   sync.Once
}

func NewLines(conn net.Conn) *Lines {
	return &Lines{conn: conn, reader: bufio.NewReaderSize(conn, 64*1024)}
}

func (l *Lines) Send(message []byte) error {
	if bytes.IndexByte(message, '\n') >= 0 {
		return errors.New("a message contains a raw newline")
	}
	if len(message) > MaxMessage {
		return fmt.Errorf("message of %d bytes exceeds the limit of %d", len(message), MaxMessage)
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	buffer := make([]byte, 0, len(message)+1)
	buffer = append(append(buffer, message...), '\n')
	_ = l.conn.SetWriteDeadline(time.Now().Add(writeTimeout))
	_, err := l.conn.Write(buffer)
	return err
}

func (l *Lines) Recv() ([]byte, error) {
	var line []byte
	for {
		chunk, err := l.reader.ReadSlice('\n')
		line = append(line, chunk...)
		if len(line) > MaxMessage+1 {
			return nil, fmt.Errorf("a message exceeds the limit of %d bytes", MaxMessage)
		}
		if err == nil {
			line = line[:len(line)-1]
			if !utf8.Valid(line) {
				return nil, errors.New("a message is not UTF-8")
			}
			return line, nil
		}
		if errors.Is(err, bufio.ErrBufferFull) {
			continue
		}
		if errors.Is(err, io.EOF) {
			if len(line) > 0 {
				return nil, errors.New("stream ended inside a message")
			}
			return nil, io.EOF
		}
		return nil, err
	}
}

func (l *Lines) Close(string) {
	l.once.Do(func() { _ = l.conn.Close() })
}
