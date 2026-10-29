//go:build !unix

package rutistest

import (
	"errors"
	"os"
)

// No socket pairs: the channel goes over a loopback connection.
const socketPairs = false

func socketPair() (ours, theirs *os.File, err error) {
	return nil, nil, errors.New("no socket pairs on this system")
}
