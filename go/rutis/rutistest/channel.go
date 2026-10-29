package rutistest

import (
	"bufio"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"net"
	"os"
	"os/exec"
	"strings"
	"time"
)

const (
	// hostVariable names the rutis-host to use instead of the one on PATH.
	hostVariable = "RUTIS_HOST"
	// handoverVariable set to `loopback` hands the channel over on a
	// loopback connection on every system, as for local runtimes.
	handoverVariable = "RUTIS_LOCAL_HANDOVER"
	tokenVariable    = "RUTIS_CHANNEL_TOKEN"
)

// channel starts `rutis-host test <driver> --channel <channel>` and is the
// runtime's end of the session: on Unix, a socket pair whose other end
// rutis-host inherits as fd 3; on Windows and with
// RUTIS_LOCAL_HANDOVER=loopback, a loopback connection rutis-host makes,
// presenting a one-time token first. `ended` says when rutis-host ended.
func channel(driver string) (conn net.Conn, ended <-chan error, err error) {
	path := os.Getenv(hostVariable)
	if path == "" {
		if path, err = exec.LookPath("rutis-host"); err != nil {
			return nil, nil, fmt.Errorf("rutis-host is not on PATH: install it, or set %s to its path", hostVariable)
		}
	}
	if os.Getenv(handoverVariable) == "loopback" || !socketPairs {
		return loopback(path, driver)
	}
	ours, theirs, err := socketPair()
	if err != nil {
		return nil, nil, err
	}
	command := exec.Command(path, "test", driver, "--channel", "fd:3")
	command.ExtraFiles = []*os.File{theirs}
	exited, err := run(command, path)
	theirs.Close()
	if err != nil {
		ours.Close()
		return nil, nil, err
	}
	conn, err = net.FileConn(ours)
	ours.Close()
	if err != nil {
		return nil, nil, err
	}
	return conn, exited, nil
}

// loopback hands the channel over on a loopback connection.
func loopback(path, driver string) (net.Conn, <-chan error, error) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, nil, err
	}
	defer listener.Close()
	secret := make([]byte, 16)
	if _, err := rand.Read(secret); err != nil {
		return nil, nil, err
	}
	token := hex.EncodeToString(secret)
	command := exec.Command(path, "test", driver, "--channel", "tcp:"+listener.Addr().String())
	command.Env = append(os.Environ(), tokenVariable+"="+token)
	exited, err := run(command, path)
	if err != nil {
		return nil, nil, err
	}
	accepted := make(chan net.Conn, 1)
	go func() {
		for {
			conn, err := listener.Accept()
			if err != nil {
				return
			}
			_ = conn.SetReadDeadline(time.Now().Add(10 * time.Second))
			reader := bufio.NewReader(conn)
			line, err := reader.ReadString('\n')
			if err != nil || strings.TrimSuffix(line, "\n") != token || reader.Buffered() > 0 {
				conn.Close()
				continue
			}
			_ = conn.SetReadDeadline(time.Time{})
			accepted <- conn
			return
		}
	}()
	select {
	case conn := <-accepted:
		return conn, exited, nil
	case err := <-exited:
		if err == nil {
			err = errors.New("it exited before connecting")
		}
		return nil, nil, fmt.Errorf("rutis-host test: %w", err)
	}
}

// run starts `command`, its output on this process's standard error;
// the channel says when it exited.
func run(command *exec.Cmd, path string) (<-chan error, error) {
	command.Stdout = os.Stderr
	command.Stderr = os.Stderr
	if err := command.Start(); err != nil {
		return nil, fmt.Errorf("start %s: %w", path, err)
	}
	exited := make(chan error, 1)
	go func() { exited <- command.Wait() }()
	return exited, nil
}
