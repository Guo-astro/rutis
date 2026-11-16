package peer

import (
	"context"
	"fmt"
	"strings"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

// rule: SESS-75 (a write-back queue at its bound ends the session), #1210.
// The reader hands a refused reply's release to the writer and reads on.
// While another frame's write is held, the frames pile up behind it; past
// maxWriteBack the session ends as one whose far end stopped reading, and
// no write-back is dropped.
func TestAReadBackQueuePastItsBoundEndsTheSession(t *testing.T) {
	session, channel, _ := openBlockedSession(t)
	// Replies to calls nobody waits for: each is a late reply whose
	// reference goes back through the reader's write-back queue.
	for i := 1; i <= maxWriteBack+2; i++ {
		session.Notify("x", "m", nil)
		testwait.Within(t, "the call frame", channel.written)
	}
	channel.holdWrites()
	held := make(chan struct{})
	go func() {
		defer close(held)
		_, _ = session.Send(context.Background(), "x", "m2", nil, false)
	}()
	testwait.Within(t, "the held write", channel.blocked)

	for i := 1; i <= maxWriteBack+2; i++ {
		channel.feed(t, strings.Replace(refusedReply, `"id":"go:1"`, fmt.Sprintf(`"id":"go:%d"`, i), 1))
	}
	testwait.Within(t, "the session to end", session.Closed())
	if err := session.Err(); err == nil || !strings.Contains(err.Error(), "the far end is not reading") {
		t.Fatalf("the session ended with %v", err)
	}
	channel.releaseWrites()
	<-held
}
