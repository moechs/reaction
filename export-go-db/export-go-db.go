package main

import (
	"bufio"
	"encoding/gob"
	"encoding/json"
	"io"
	"os"
	"strings"
	"time"
)

func quit(msg string, err error) {
	if err == nil {
		print(msg)
	} else {
		print(msg, err)
	}
	os.Exit(1)
}

type SF struct{ S, F string }

type LogEntry struct {
	T time.Time
	S int64
	// This is a "\x00" Joined string
	// which contains all matches on a line.
	Pattern        string
	Stream, Filter string
	SF             int
	Exec           bool
}

type JsonEntry struct {
	Time   int64    `json:"time"` // unix epoch
	Stream string   `json:"stream"`
	Filter string   `json:"filter"`
	Match  []string `json:"match"`
	Exec   bool     `json:"exec"`
}

func export(oldpath, newpath string) {
	// Read DB
	file, err := os.Open(oldpath)
	if err != nil {
		quit("could not open db: ", err)
	}
	dec := gob.NewDecoder(file)

	// Write export
	fileNew, err := os.Create(newpath)
	if err != nil {
		quit("could not create export: ", err)
	}
	enc := json.NewEncoder(fileNew)

	malformedEntries := 0
	discardedEntries := 0

	readSF2int := make(map[int]SF)
	for {
		var entry LogEntry

		err := dec.Decode(&entry)
		if err != nil {
			if err == io.EOF {
				break
			}
			malformedEntries++
			continue
		}

		if entry.Stream == "" && entry.Filter == "" {
			sf, ok := readSF2int[entry.SF]
			if !ok {
				discardedEntries++
				continue
			}
			entry.Stream = sf.S
			entry.Filter = sf.F
		}

		if entry.SF != 0 {
			readSF2int[entry.SF] = SF{entry.Stream, entry.Filter}
		}

		if entry.T.IsZero() {
			entry.T = time.Unix(entry.S, 0)
		}

		jsonEntry := JsonEntry{
			entry.T.Unix(),
			entry.Stream,
			entry.Filter,
			strings.Split(entry.Pattern, "\x00"),
			entry.Exec,
		}
		enc.Encode(jsonEntry)
	}

	if discardedEntries > 0 {
		println(discardedEntries, "discarded entries")
	}

	if malformedEntries > 0 {
		println(malformedEntries, "malformed entries")
	}
}

func main() {
	println("This export script must run in reaction's runtime directory.")
	println("This usually is /var/lib/reaction.")
	println("It will export the go / reaction-v1.x database files as JSON.")
	println("Do you want to proceed? (y/n)")
	reader := bufio.NewReader(os.Stdin)
	line, err := reader.ReadString('\n')
	if err != nil {
		quit("fatal: could not read user input.", nil)
	} else if line != "y\n" {
		quit("user did not type `y`, quitting.", nil)
	}

	export("./reaction-matches.db", "./reaction-matches.export.json")
	export("./reaction-flushes.db", "./reaction-flushes.export.json")
}
