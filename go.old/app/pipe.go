package app

import (
	"encoding/gob"
	"errors"
	"net"
	"os"
	"path"
	"time"

	"framagit.org/ppom/reaction/logger"
)

func createOpenSocket() net.Listener {
	err := os.MkdirAll(path.Dir(*SocketPath), 0755)
	if err != nil {
		logger.Fatalln("Failed to create socket directory")
	}
	_, err = os.Stat(*SocketPath)
	if err == nil {
		logger.Println(logger.WARN, "socket", SocketPath, "already exists: Is the daemon already running? Deleting.")
		err = os.Remove(*SocketPath)
		if err != nil {
			logger.Fatalln("Failed to remove socket:", err)
		}
	}
	ln, err := net.Listen("unix", *SocketPath)
	if err != nil {
		logger.Fatalln("Failed to create socket:", err)
	}
	return ln
}

// Handle connections
//func SocketManager(streams map[string]*Stream) {
func SocketManager(conf *Conf) {
	ln := createOpenSocket()
	defer ln.Close()
	for {
		conn, err := ln.Accept()
		if err != nil {
			logger.Println(logger.ERROR, "Failed to open connection from cli:", err)
			continue
		}
		go func(conn net.Conn) {
			defer conn.Close()
			var request Request
			var response Response

			err := gob.NewDecoder(conn).Decode(&request)
			if err != nil {
				logger.Println(logger.ERROR, "Invalid Message from cli:", err)
				return
			}

			switch request.Request {
			case Info:
				// response.Config = *conf
				response.Matches = matches
				response.Actions = actions
			case Flush:
				le := LogEntry{time.Now(), 0, request.Flush.P, request.Flush.S, request.Flush.F, 0, false}

				flushToMatchesC <- request.Flush
				flushToActionsC <- request.Flush
				flushToDatabaseC <- le

			default:
				logger.Println(logger.ERROR, "Invalid Message from cli: unrecognised command type")
				response.Err = errors.New("unrecognised command type")
				return
			}

			err = gob.NewEncoder(conn).Encode(response)
			if err != nil {
				logger.Println(logger.ERROR, "Can't respond to cli:", err)
				return
			}
		}(conn)
	}
}
