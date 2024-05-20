package app

import (
	"bytes"
	"encoding/gob"
	"fmt"
	"os"
	"regexp"
	"strings"
	"time"
)

type Conf struct {
	Concurrency int                 `json:"concurrency"`
	Patterns    map[string]*Pattern `json:"patterns"`
	Streams     map[string]*Stream  `json:"streams"`
	Start       [][]string          `json:"start"`
	Stop        [][]string          `json:"stop"`
}

type Pattern struct {
	Regex  string   `json:"regex"`
	Ignore []string `json:"ignore"`

	IgnoreRegex         []string        `json:"ignoreregex"`
	compiledIgnoreRegex []regexp.Regexp `json:"-"`

	Name           string `json:"-"`
	nameWithBraces string `json:"-"`
}

// Stream, Filter & Action structures must never be copied.
// They're always referenced through pointers

type Stream struct {
	Name string `json:"-"`

	Cmd     []string           `json:"cmd"`
	Filters map[string]*Filter `json:"filters"`
}
type LilStream struct {
	Name string
}

func (s *Stream) GobEncode() ([]byte, error) {
	var buf bytes.Buffer
	enc := gob.NewEncoder(&buf)
	err := enc.Encode(LilStream{s.Name})
	return buf.Bytes(), err
}

func (s *Stream) GobDecode(b []byte)(error) {
	var ls LilStream
	dec := gob.NewDecoder(bytes.NewReader(b))
	err := dec.Decode(&ls)
	s.Name = ls.Name
	return err
}

type Filter struct {
	Stream *Stream `json:"-"`
	Name   string  `json:"-"`

	Regex         []string        `json:"regex"`
	compiledRegex []regexp.Regexp `json:"-"`
	Pattern       []*Pattern      `json:"-"`

	Retry         int           `json:"retry"`
	RetryPeriod   string        `json:"retryperiod"`
	retryDuration time.Duration `json:"-"`

	Actions                map[string]*Action `json:"actions"`
	longuestActionDuration *time.Duration
}

// those small versions are needed to prevent infinite recursion in gob because of
// data cycles: Stream <-> Filter, Filter <-> Action
type LilFilter struct {
	Stream  *Stream
	Name    string
	Pattern []*Pattern
}

func (f *Filter) GobDecode(b []byte)(error) {
	var lf LilFilter
	dec := gob.NewDecoder(bytes.NewReader(b))
	err := dec.Decode(&lf)
	f.Stream = lf.Stream
	f.Name = lf.Name
	f.Pattern = lf.Pattern
	return err
}

func (f *Filter) GobEncode() ([]byte, error) {
	var buf bytes.Buffer
	enc := gob.NewEncoder(&buf)
	err := enc.Encode(LilFilter{f.Stream, f.Name, f.Pattern})
	return buf.Bytes(), err
}

type Action struct {
	Filter *Filter `json:"-"`
	Name   string  `json:"-"`

	Cmd []string `json:"cmd"`

	After         string        `json:"after"`
	afterDuration time.Duration `json:"-"`

	OnExit bool `json:"onexit"`
}
type LilAction struct {
	Filter *Filter
	Name   string
}

func (a *Action) GobEncode() ([]byte, error) {
	var buf bytes.Buffer
	enc := gob.NewEncoder(&buf)
	err := enc.Encode(LilAction{a.Filter, a.Name})
	return buf.Bytes(), err
}

func (a *Action) GobDecode(b []byte)(error) {
	var la LilAction
	dec := gob.NewDecoder(bytes.NewReader(b))
	err := dec.Decode(&la)
	a.Filter = la.Filter
	a.Name = la.Name
	return err
}

type LogEntry struct {
	T              time.Time
	S              int64
	Pattern        Match
	Stream, Filter string
	SF             int
	Exec           bool
}

type ReadDB struct {
	file *os.File
	dec  *gob.Decoder
}

type WriteDB struct {
	file *os.File
	enc  *gob.Encoder
}

type MatchesMap map[PF]map[time.Time]struct{}
type ActionsMap map[PA]map[time.Time]struct{}

// This is a "\x00" Joined string
// which contains all matches on a line.
type Match string

func (m *Match) Split() []string {
	return strings.Split(string(*m), "\x00")
}
func JoinMatch(mm []string) Match {
	return Match(strings.Join(mm, "\x00"))
}
func WithBrackets(mm []string) string {
	var b strings.Builder
	for _, match := range mm {
		fmt.Fprintf(&b, "[%s]", match)
	}
	return b.String()
}

// Helper structs made to carry information
// Stream, Filter
type SF struct{ S, F string }

// Pattern, Stream, Filter
type PSF struct {
	P    Match
	S, F string
}

type PF struct {
	P Match
	F *Filter
}
type PFT struct {
	P Match
	F *Filter
	T time.Time
}
type PA struct {
	P Match
	A *Action
}
type PAT struct {
	P Match
	A *Action
	T time.Time
}
