//go:build unix

package rutistest

import (
	"os"
	"syscall"
)

const socketPairs = true

// socketPair is a connected pair of Unix sockets, close-on-exec.
func socketPair() (ours, theirs *os.File, err error) {
	syscall.ForkLock.RLock()
	fds, err := syscall.Socketpair(syscall.AF_UNIX, syscall.SOCK_STREAM, 0)
	if err == nil {
		syscall.CloseOnExec(fds[0])
		syscall.CloseOnExec(fds[1])
	}
	syscall.ForkLock.RUnlock()
	if err != nil {
		return nil, nil, os.NewSyscallError("socketpair", err)
	}
	return os.NewFile(uintptr(fds[0]), "rutistest"), os.NewFile(uintptr(fds[1]), "rutis-host"), nil
}
