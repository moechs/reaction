package app

import (
	"bufio"
	"encoding/gob"
	"encoding/json"
	"fmt"
	"net"
	"os"
	"regexp"
	"slices"
	"strings"
	"time"

	"framagit.org/ppom/reaction/logger"
	"sigs.k8s.io/yaml"
)

const (
	Info  = 0
	Flush = 1
)

type Request struct {
	Request int
	Flush   PSF
}

type Response struct {
	Err error
	// Config  Conf
	Matches MatchesMap
	Actions ActionsMap
}

func SendAndRetrieve(data Request) Response {
	conn, err := net.Dial("unix", *SocketPath)
	if err != nil {
		logger.Fatalln("Error opening connection top daemon:", err)
	}
	defer conn.Close()

	err = gob.NewEncoder(conn).Encode(data)
	if err != nil {
		logger.Fatalln("Can't send message:", err)
	}

	var response Response
	err = gob.NewDecoder(conn).Decode(&response)
	if err != nil {
		logger.Fatalln("Invalid answer from daemon:", err)
	}
	return response
}

type PatternStatus struct {
	Matches int                 `json:"matches,omitempty"`
	Actions map[string][]string `json:"actions,omitempty"`
}
type MapPatternStatus map[Match]*PatternStatus
type MapPatternStatusFlush MapPatternStatus

type ClientStatus map[string]map[string]MapPatternStatus
type ClientStatusFlush ClientStatus

func (mps MapPatternStatusFlush) MarshalJSON() ([]byte, error) {
	for _, v := range mps {
		return json.Marshal(v)
	}
	return []byte(""), nil
}

func (csf ClientStatusFlush) MarshalJSON() ([]byte, error) {
	ret := make(map[string]map[string]MapPatternStatusFlush)
	for k, v := range csf {
		ret[k] = make(map[string]MapPatternStatusFlush)
		for kk, vv := range v {
			ret[k][kk] = MapPatternStatusFlush(vv)
		}
	}
	return json.Marshal(ret)
}

func pfMatches(streamName string, filterName string, regexes map[string]*regexp.Regexp, match Match, filter *Filter) bool {
	// Check stream and filter match
	if streamName != "" && streamName != filter.Stream.Name {
		return false
	}
	if filterName != "" && filterName != filter.Name {
		return false
	}
	// Check that all user requested patterns are in this filter
	var nbMatched int
	var localMatches = match.Split()
	// For each pattern of this filter
	for i, pattern := range filter.Pattern {
		// Check that this pattern has user requested name
		if reg, ok := regexes[pattern.Name]; ok {
			// Check that the PF.p[i] matches user requested pattern
			if reg.MatchString(localMatches[i]) {
				nbMatched++
			}
		}
	}
	if len(regexes) != nbMatched {
		return false
	}
	// All checks passed
	return true
}

func addMatchToCS(cs ClientStatus, pf PF, times map[time.Time]struct{}) {
	patterns, streamName, filterName := pf.P, pf.F.Stream.Name, pf.F.Name
	if cs[streamName] == nil {
		cs[streamName] = make(map[string]MapPatternStatus)
	}
	if cs[streamName][filterName] == nil {
		cs[streamName][filterName] = make(MapPatternStatus)
	}
	cs[streamName][filterName][patterns] = &PatternStatus{len(times), nil}
}

func addActionToCS(cs ClientStatus, pa PA, times map[time.Time]struct{}) {
	patterns, streamName, filterName, actionName := pa.P, pa.A.Filter.Stream.Name, pa.A.Filter.Name, pa.A.Name
	if cs[streamName] == nil {
		cs[streamName] = make(map[string]MapPatternStatus)
	}
	if cs[streamName][filterName] == nil {
		cs[streamName][filterName] = make(MapPatternStatus)
	}
	if cs[streamName][filterName][patterns] == nil {
		cs[streamName][filterName][patterns] = new(PatternStatus)
	}
	ps := cs[streamName][filterName][patterns]
	if ps.Actions == nil {
		ps.Actions = make(map[string][]string)
	}
	for then := range times {
		ps.Actions[actionName] = append(ps.Actions[actionName], then.Format(time.DateTime))
	}
}

func printClientStatus(cs ClientStatus, format string) {
	var text []byte
	var err error
	if format == "json" {
		text, err = json.MarshalIndent(cs, "", "  ")
	} else {
		text, err = yaml.Marshal(cs)
	}
	if err != nil {
		logger.Fatalln("Failed to convert daemon binary response to text format:", err)
	}

	fmt.Println(strings.ReplaceAll(string(text), "\\0", " "))
}

func compileKVPatterns(kvpatterns []string) map[string]*regexp.Regexp {
	var regexes map[string]*regexp.Regexp
	regexes = make(map[string]*regexp.Regexp)
	for _, p := range kvpatterns {
		// p syntax already checked in Main
		key, value, found := strings.Cut(p, "=")
		if !found {
			logger.Printf(logger.ERROR, "Bad argument: no `=` in %v", p)
			logger.Fatalln("Patterns must be prefixed by their name (e.g. ip=1.1.1.1)")
		}
		if regexes[key] != nil {
			logger.Fatalf("Bad argument: same pattern name provided multiple times: %v", key)
		}
		compiled, err := regexp.Compile(fmt.Sprintf("^%v$", value))
		if err != nil {
			logger.Fatalf("Bad argument: Could not compile: `%v`: %v", value, err)
		}
		regexes[key] = compiled
	}
	return regexes
}

func ClientShow(format, stream, filter string, kvpatterns []string) {
	response := SendAndRetrieve(Request{Info, PSF{}})
	if response.Err != nil {
		logger.Fatalln("Received error from daemon:", response.Err)
	}

	cs := make(ClientStatus)

	var regexes map[string]*regexp.Regexp

	if len(kvpatterns) != 0 {
		regexes = compileKVPatterns(kvpatterns)
	}

	var found bool

	// Painful data manipulation
	for pf, times := range response.Matches {
		// Check this PF is not empty
		if len(times) == 0 {
			continue
		}
		if !pfMatches(stream, filter, regexes, pf.P, pf.F) {
			continue
		}
		addMatchToCS(cs, pf, times)
		found = true
	}

	// Painful data manipulation
	for pa, times := range response.Actions {
		// Check this PF is not empty
		if len(times) == 0 {
			continue
		}
		if !pfMatches(stream, filter, regexes, pa.P, pa.A.Filter) {
			continue
		}
		addActionToCS(cs, pa, times)
		found = true
	}

	if !found {
		logger.Println(logger.WARN, "No matching stream.filter items found. This does not mean it doesn't exist, maybe it just didn't receive any match.")
		os.Exit(1)
	}

	printClientStatus(cs, format)

	os.Exit(0)
}

// TODO : Show values we just flushed - for now we got no details :
/*
 * % ./reaction flush -l ssh.failedlogin login=".*t"
 * ssh:
 *  failedlogin:
 *    actions:
 *      unban:
 *      - "2024-04-30 15:27:28"
 *      - "2024-04-30 15:27:28"
 *      - "2024-04-30 15:27:28"
 *      - "2024-04-30 15:27:28"
 *
 */
func ClientFlush(format, streamName, filterName string, patterns []string) {
	requestedPatterns := compileKVPatterns(patterns)

	// Remember which Filters are compatible with the query
	filterCompatibility := make(map[SF]bool)
	isCompatible := func(filter *Filter) bool {
		sf := SF{filter.Stream.Name, filter.Name}
		compatible, ok := filterCompatibility[sf]

		// already tested
		if ok {
			return compatible
		}

		for k := range requestedPatterns {
			if -1 == slices.IndexFunc(filter.Pattern, func(pattern *Pattern) bool {
				return pattern.Name == k
			}) {
				filterCompatibility[sf] = false
				return false
			}
		}
		filterCompatibility[sf] = true
		return true
	}

	// match functions
	kvMatch := func(filter *Filter, filterPatterns []string) bool {
		// For each user requested pattern
		for k, v := range requestedPatterns {
			// Find its index on the Filter.Pattern
			for i, pattern := range filter.Pattern {
				if k == pattern.Name {
					// Test the match
					if !v.MatchString(filterPatterns[i]) {
						return false
					}
				}
			}
		}
		return true
	}

	var found bool
	fullMatch := func(filter *Filter, match Match) bool {
		// Test if we limit by stream
		if streamName == "" || filter.Stream.Name == streamName {
			// Test if we limit by filter
			if filterName == "" || filter.Name == filterName {
				found = true
				filterPatterns := match.Split()
				return isCompatible(filter) && kvMatch(filter, filterPatterns)
			}
		}
		return false
	}

	response := SendAndRetrieve(Request{Info, PSF{}})
	if response.Err != nil {
		logger.Fatalln("Received error from daemon:", response.Err)
	}

	commands := make([]PSF, 0)

	cs := make(ClientStatus)

	for pf, times := range response.Matches {
		if fullMatch(pf.F, pf.P) {
			commands = append(commands, PSF{pf.P, pf.F.Stream.Name, pf.F.Name})
			addMatchToCS(cs, pf, times)
		}
	}

	for pa, times := range response.Actions {
		if fullMatch(pa.A.Filter, pa.P) {
			commands = append(commands, PSF{pa.P, pa.A.Filter.Stream.Name, pa.A.Filter.Name})
			addActionToCS(cs, pa, times)
		}
	}

	if !found {
		logger.Println(logger.WARN, "No matching stream.filter items found. This does not mean it doesn't exist, maybe it just didn't receive any match.")
		os.Exit(1)
	}

	for _, psf := range commands {
		response := SendAndRetrieve(Request{Flush, psf})
		if response.Err != nil {
			logger.Fatalln("Received error from daemon:", response.Err)
		}
	}

	printClientStatus(cs, format)
	os.Exit(0)
}

func TestRegex(confFilename, regex, line string) {
	conf := parseConf(confFilename)

	// Code close to app/startup.go
	var usedPatterns []*Pattern
	for _, pattern := range conf.Patterns {
		if strings.Contains(regex, pattern.nameWithBraces) {
			usedPatterns = append(usedPatterns, pattern)
			regex = strings.Replace(regex, pattern.nameWithBraces, pattern.Regex, 1)
		}
	}
	reg, err := regexp.Compile(regex)
	if err != nil {
		logger.Fatalln("ERROR the specified regex is invalid: %v", err)
		os.Exit(1)
	}

	// Code close to app/daemon.go
	match := func(line string) {
		var ignored bool
		if matches := reg.FindStringSubmatch(line); matches != nil {
			if usedPatterns != nil {
				var result []string
				for _, p := range usedPatterns {
					match := matches[reg.SubexpIndex(p.Name)]
					result = append(result, match)
					if !p.notAnIgnore(&match) {
						ignored = true
					}
				}
				if !ignored {
					fmt.Printf("\033[32mmatching\033[0m %v: %v\n", WithBrackets(result), line)
				} else {
					fmt.Printf("\033[33mignore matching\033[0m %v: %v\n", WithBrackets(result), line)
				}
			} else {
				fmt.Printf("\033[32mmatching\033[0m [%v]:\n", line)
			}
		} else {
			fmt.Printf("\033[31mno match\033[0m: %v\n", line)
		}
	}

	if line != "" {
		match(line)
	} else {
		logger.Println(logger.INFO, "no second argument: reading from stdin")
		scanner := bufio.NewScanner(os.Stdin)
		for scanner.Scan() {
			match(scanner.Text())
		}
	}
}
